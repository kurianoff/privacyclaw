use crate::dashboard::WsEvent;
use crate::parser::{self, Provider};
use crate::parser::sse::SseParser;
use crate::pii::{self, PiiCtx, PiiMode, PiiPipeline};
use crate::pii::buffer::ReplacementBuffer;
use crate::pii::vault::VaultHandle;
use crate::storage::{Conversation, Message, Store};
use crate::util::{new_uuid, now_iso8601};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot};

const MAX_SSE_BUFFER: usize = 10 * 1024 * 1024; // 10 MB
const READ_BUF: usize = 65536;

/// How long u_to_c waits for any upstream data before giving up.
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How long c_to_u waits for a single write to upstream to complete.
const UPSTREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle a fully decrypted bidirectional stream between client and upstream.
///
/// `pii` is `Some(...)` when PII protection mode is active, `None` for Phase 1
/// passthrough behaviour (byte-identical forwarding).
pub async fn run(
    client_reader: impl AsyncRead + Unpin + Send + 'static,
    client_writer: impl AsyncWrite + Unpin + Send + 'static,
    upstream_reader: impl AsyncRead + Unpin + Send + 'static,
    upstream_writer: impl AsyncWrite + Unpin + Send + 'static,
    host: String,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    pii: PiiCtx,
) -> Result<()> {
    let provider = Provider::from_host(&host);
    tracing::warn!(host = %host, provider = provider.as_str(), "intercept: session started");

    let shared_conv_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // Vault handle populated by c_to_u after PII pipeline runs; read by u_to_c.
    let shared_vault: Arc<Mutex<Option<VaultHandle>>> = Arc::new(Mutex::new(None));

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let c_to_u = tokio::spawn({
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let shared_conv_id = Arc::clone(&shared_conv_id);
        let shared_vault = Arc::clone(&shared_vault);
        let pii = pii.clone();
        async move {
            let result = handle_c2u(
                client_reader, upstream_writer, provider,
                store, ws_tx, host, shared_conv_id, shared_vault, pii,
            ).await;
            let _ = shutdown_tx.send(());
            if let Err(e) = result { tracing::debug!("c→u closed: {}", e); }
        }
    });

    let u_to_c = tokio::spawn(async move {
        if let Err(e) = handle_u2c(
            upstream_reader, client_writer, provider,
            store, ws_tx, shared_conv_id, shared_vault, shutdown_rx, pii,
        ).await {
            tracing::debug!("u→c closed: {}", e);
        }
    });

    let _ = tokio::join!(c_to_u, u_to_c);
    Ok(())
}

// ─── Client → Upstream ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_c2u(
    reader: impl AsyncRead + Unpin,
    writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    host: String,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    shared_vault: Arc<Mutex<Option<VaultHandle>>>,
    pii: PiiCtx,
) -> Result<()> {
    let pii_active = pii.as_ref()
        .map(|p| p.mode != PiiMode::Off)
        .unwrap_or(false);

    if pii_active {
        handle_c2u_pii(reader, writer, provider, store, ws_tx, host, shared_conv_id, shared_vault, pii).await
    } else {
        handle_c2u_passthrough(reader, writer, provider, store, ws_tx, host, shared_conv_id).await
    }
}

/// Phase 1 outbound: forward bytes immediately, parse off-path.
async fn handle_c2u_passthrough(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    host: String,
    shared_conv_id: Arc<Mutex<Option<String>>>,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut content_length: Option<usize> = None;
    let mut body_start: usize = 0;
    let mut body_received: usize = 0;
    let mut parse_attempted = false;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 { break; }

        let chunk = &buf[..n];
        match tokio::time::timeout(UPSTREAM_WRITE_TIMEOUT, writer.write_all(chunk)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => anyhow::bail!("upstream write stalled for {}s", UPSTREAM_WRITE_TIMEOUT.as_secs()),
        }
        raw.extend_from_slice(chunk);

        if !header_done {
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                body_start = hdr_end;
                content_length = parse_content_length(&raw[..hdr_end]);
                body_received = raw.len() - body_start;
            }
        } else {
            body_received += chunk.len();
        }

        if header_done && !parse_attempted {
            if let Some(len) = content_length {
                let body = &raw[body_start..];
                if body.len() >= len {
                    parse_attempted = true;
                    log_request(body, provider, &host, &store, &ws_tx, &shared_conv_id).await;
                    raw.clear();
                    raw.shrink_to_fit();
                }
            }
        }

        if header_done {
            if let Some(len) = content_length {
                if body_received >= len {
                    header_done = false;
                    content_length = None;
                    body_start = 0;
                    body_received = 0;
                    parse_attempted = false;
                    raw.clear();
                }
            }
        }
    }

    if header_done && content_length.is_none() && !parse_attempted {
        let body = &raw[body_start..];
        if !body.is_empty() {
            log_request(body, provider, &host, &store, &ws_tx, &shared_conv_id).await;
        }
    }

    Ok(())
}

/// PII mode outbound: buffer complete request, run pipeline, forward modified.
#[allow(clippy::too_many_arguments)]
async fn handle_c2u_pii(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    host: String,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    shared_vault: Arc<Mutex<Option<VaultHandle>>>,
    pii: PiiCtx,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut content_length: Option<usize> = None;
    let mut body_start: usize = 0;
    let mut body_received: usize = 0;

    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 { break; }
        raw.extend_from_slice(&buf[..n]);

        if !header_done {
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                body_start = hdr_end;
                content_length = parse_content_length(&raw[..hdr_end]);
                body_received = raw.len() - body_start;
            }
        } else {
            body_received += buf[..n].len();
        }

        let body_done = header_done && content_length.map_or(false, |cl| body_received >= cl);
        if !body_done { continue; }

        let cl = content_length.unwrap_or(body_received);
        let original_body = raw[body_start..body_start + cl].to_vec();

        // Log original body (dashboard shows real PII locally).
        log_request(&original_body, provider, &host, &store, &ws_tx, &shared_conv_id).await;

        // Get or create vault keyed by conversation_id.
        let vault_handle = if let Some(ref pii_ctx) = pii {
            let cid = shared_conv_id.lock().unwrap().clone()
                .unwrap_or_else(new_uuid);
            Some(pii_ctx.registry.get_or_create(&cid))
        } else {
            None
        };

        // Run PII pipeline (replace mode) to get modified body.
        let forward_request = if let (Some(ref pii_ctx), Some(ref vh)) = (&pii, &vault_handle) {
            if pii_ctx.mode == PiiMode::Replace {
                let mut vault = vh.write().unwrap();
                match PiiPipeline::process_request_body(&original_body, &mut vault, provider, &pii_ctx.locale) {
                    Some(new_body) => pii::rebuild_request(&raw, body_start, &new_body),
                    None => raw.clone(),
                }
            } else {
                raw.clone() // detect-only
            }
        } else {
            raw.clone()
        };

        // Share vault with u_to_c before forwarding request.
        if let Some(vh) = vault_handle {
            *shared_vault.lock().unwrap() = Some(vh);
        }

        // Forward (possibly modified) request.
        match tokio::time::timeout(UPSTREAM_WRITE_TIMEOUT, writer.write_all(&forward_request)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => anyhow::bail!("upstream write stalled"),
        }

        // Reset for next request on keep-alive connection.
        raw.clear();
        header_done = false;
        content_length = None;
        body_start = 0;
        body_received = 0;
    }

    Ok(())
}

// ─── Upstream → Client ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_u2c(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    shared_vault: Arc<Mutex<Option<VaultHandle>>>,
    mut shutdown: oneshot::Receiver<()>,
    pii: PiiCtx,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let pii_replace = pii.as_ref()
        .map(|p| p.mode == PiiMode::Replace)
        .unwrap_or(false);

    // Per-response state.
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut is_sse = false;
    let mut sse_parser = SseParser::new();
    let mut accumulated = String::new();
    let mut accumulation_stopped = false;
    let mut tokens_in: Option<i64> = None;
    let mut tokens_out: Option<i64> = None;
    let mut body_buf: Vec<u8> = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut body_received: usize = 0;
    let mut rep_buf: Option<ReplacementBuffer> = None;

    loop {
        let n = tokio::select! {
            result = tokio::time::timeout(UPSTREAM_READ_TIMEOUT, reader.read(&mut buf)) => {
                match result {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => { tracing::debug!("u2c: upstream read error: {}", e); break; }
                    Err(_) => { tracing::warn!(timeout_secs = UPSTREAM_READ_TIMEOUT.as_secs(), "u2c: idle timeout"); break; }
                }
            }
            _ = &mut shutdown => { tracing::debug!("u2c: client closed"); break; }
        };
        if n == 0 { break; }
        let chunk = &buf[..n];

        let sse_done = if !header_done {
            raw.extend_from_slice(chunk);
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                let headers_text = String::from_utf8_lossy(&raw[..hdr_end]);
                is_sse = headers_text.contains("text/event-stream");
                content_length = parse_content_length(&raw[..hdr_end]);
                tracing::debug!(is_sse, "u2c: response headers done");

                // Initialize ReplacementBuffer when PII replace + SSE.
                if pii_replace && is_sse {
                    let vault = get_vault_with_backoff(&shared_vault).await;
                    if let Some(vh) = vault {
                        rep_buf = Some(ReplacementBuffer::new(vh));
                    }
                }

                let done = if hdr_end < raw.len() {
                    body_received += raw.len() - hdr_end;
                    let body_chunk = raw[hdr_end..].to_vec();
                    raw.clear(); raw.shrink_to_fit();
                    if pii_replace && is_sse {
                        process_sse_chunk_pii(
                            &body_chunk, &mut sse_parser, &mut rep_buf,
                            &mut accumulated, &mut accumulation_stopped,
                            provider, &ws_tx, &shared_conv_id,
                            &mut tokens_in, &mut tokens_out, &mut writer,
                        ).await?
                    } else {
                        writer.write_all(&body_chunk).await?;
                        process_response_chunk(
                            &body_chunk, is_sse, &mut sse_parser, &mut accumulated,
                            &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                            &mut tokens_in, &mut tokens_out, &mut body_buf,
                        )
                    }
                } else {
                    raw.clear(); raw.shrink_to_fit();
                    false
                };
                done
            } else {
                false
            }
        } else {
            body_received += chunk.len();
            if pii_replace && is_sse {
                process_sse_chunk_pii(
                    chunk, &mut sse_parser, &mut rep_buf,
                    &mut accumulated, &mut accumulation_stopped,
                    provider, &ws_tx, &shared_conv_id,
                    &mut tokens_in, &mut tokens_out, &mut writer,
                ).await?
            } else {
                writer.write_all(chunk).await?;
                process_response_chunk(
                    chunk, is_sse, &mut sse_parser, &mut accumulated,
                    &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                    &mut tokens_in, &mut tokens_out, &mut body_buf,
                )
            }
        };

        let response_complete = sse_done
            || (!is_sse && content_length.map_or(false, |cl| body_received >= cl));

        if response_complete && header_done {
            flush_rep_buf_and_finalize(
                &mut rep_buf, &mut accumulated, &mut accumulation_stopped,
                provider, &ws_tx, &shared_conv_id, &shared_vault,
                &store, tokens_in, tokens_out, pii_replace, &mut writer,
            ).await?;

            header_done = false;
            is_sse = false;
            sse_parser = SseParser::new();
            accumulated = String::new();
            accumulation_stopped = false;
            tokens_in = None;
            tokens_out = None;
            body_buf = Vec::new();
            content_length = None;
            body_received = 0;
            rep_buf = None;
        }
    }

    flush_rep_buf_and_finalize(
        &mut rep_buf, &mut accumulated, &mut accumulation_stopped,
        provider, &ws_tx, &shared_conv_id, &shared_vault,
        &store, tokens_in, tokens_out, pii_replace, &mut writer,
    ).await?;

    Ok(())
}

/// Flush ReplacementBuffer then finalize the response.
#[allow(clippy::too_many_arguments)]
async fn flush_rep_buf_and_finalize(
    rep_buf: &mut Option<ReplacementBuffer>,
    accumulated: &mut String,
    accumulation_stopped: &mut bool,
    provider: Provider,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    shared_vault: &Arc<Mutex<Option<VaultHandle>>>,
    store: &Store,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    pii_replace: bool,
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<()> {
    if let Some(ref mut rb) = rep_buf {
        let remaining = rb.flush_remaining();
        if !remaining.is_empty() {
            if let Some(event_bytes) = make_trailing_text_event(provider, &remaining) {
                writer.write_all(&event_bytes).await?;
            }
            if !*accumulation_stopped {
                accumulated.push_str(&remaining);
                let cid = shared_conv_id.lock().unwrap().clone().unwrap_or_else(new_uuid);
                let _ = ws_tx.send(WsEvent::TextDelta {
                    conversation_id: cid,
                    text: remaining,
                    timestamp: now_iso8601(),
                });
            }
        }
    }

    finalize_response(
        shared_conv_id, shared_vault, store, ws_tx, provider,
        accumulated, tokens_in, tokens_out, pii_replace,
    ).await;
    Ok(())
}

/// Wait up to 50ms for the shared vault to be set by c_to_u.
async fn get_vault_with_backoff(shared_vault: &Arc<Mutex<Option<VaultHandle>>>) -> Option<VaultHandle> {
    for _ in 0..5 {
        {
            let g = shared_vault.lock().unwrap();
            if g.is_some() { return g.clone(); }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    shared_vault.lock().unwrap().clone()
}

// ─── PII SSE chunk processing ─────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn process_sse_chunk_pii(
    chunk: &[u8],
    sse_parser: &mut SseParser,
    rep_buf: &mut Option<ReplacementBuffer>,
    accumulated: &mut String,
    accumulation_stopped: &mut bool,
    provider: Provider,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    tokens_in: &mut Option<i64>,
    tokens_out: &mut Option<i64>,
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<bool> {
    for event in sse_parser.push(chunk) {
        if SseParser::is_done_sentinel(&event) {
            writer.write_all(&reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
            return Ok(true);
        }

        if provider == Provider::Anthropic {
            if event.event_type.as_deref() == Some("message_stop") {
                writer.write_all(&reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
                return Ok(true);
            }
            let (ti, to) = crate::parser::anthropic::extract_message_start_tokens(&event);
            if ti.is_some() { *tokens_in = ti; }
            if to.is_some() { *tokens_out = to; }
        }

        let maybe_text = extract_text_delta(provider, &event.data);

        let event_bytes = if let (Some(text), Some(ref mut rb)) = (maybe_text.as_ref(), &mut *rep_buf) {
            let flushed = rb.process_delta(text);

            if !flushed.is_empty() {
                if !*accumulation_stopped {
                    accumulated.push_str(&flushed);
                    if accumulated.len() > MAX_SSE_BUFFER {
                        *accumulation_stopped = true;
                        tracing::warn!("SSE accumulation buffer limit reached");
                    }
                }
                let cid = shared_conv_id.lock().unwrap().clone().unwrap_or_else(new_uuid);
                let _ = ws_tx.send(WsEvent::TextDelta {
                    conversation_id: cid,
                    text: flushed.clone(),
                    timestamp: now_iso8601(),
                });
                let new_data = replace_text_delta(provider, &event.data, &flushed);
                reconstruct_sse_event_bytes(event.event_type.as_deref(), &new_data)
            } else {
                // Buffer holding: emit empty text delta to keep SSE stream alive.
                let new_data = replace_text_delta(provider, &event.data, "");
                reconstruct_sse_event_bytes(event.event_type.as_deref(), &new_data)
            }
        } else {
            // Non-text event or no rep_buf: forward reconstructed.
            if let Some(delta) = parser::extract_sse_delta(provider, &event) {
                if !delta.is_empty() && !*accumulation_stopped {
                    accumulated.push_str(&delta);
                    let cid = shared_conv_id.lock().unwrap().clone().unwrap_or_else(new_uuid);
                    let _ = ws_tx.send(WsEvent::TextDelta {
                        conversation_id: cid,
                        text: delta,
                        timestamp: now_iso8601(),
                    });
                }
            }
            reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)
        };

        writer.write_all(&event_bytes).await?;
    }
    Ok(false)
}

// ─── SSE helpers ─────────────────────────────────────────────────────────────

fn reconstruct_sse_event_bytes(event_type: Option<&str>, data: &str) -> Vec<u8> {
    let mut b = Vec::new();
    if let Some(et) = event_type {
        b.extend_from_slice(b"event: ");
        b.extend_from_slice(et.as_bytes());
        b.push(b'\n');
    }
    b.extend_from_slice(b"data: ");
    b.extend_from_slice(data.as_bytes());
    b.push(b'\n');
    b.push(b'\n');
    b
}

fn extract_text_delta(provider: Provider, data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    match provider {
        Provider::Anthropic => {
            if v["type"].as_str() == Some("content_block_delta")
                && v["delta"]["type"].as_str() == Some("text_delta")
            {
                v["delta"]["text"].as_str().map(|s| s.to_string())
            } else {
                None
            }
        }
        Provider::OpenAI => v["choices"][0]["delta"]["content"].as_str().map(|s| s.to_string()),
        Provider::Google => {
            v["candidates"][0]["content"]["parts"][0]["text"].as_str().map(|s| s.to_string())
        }
        Provider::Unknown => None,
    }
}

fn replace_text_delta(provider: Provider, data: &str, new_text: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };
    match provider {
        Provider::Anthropic => {
            if let Some(t) = v.pointer_mut("/delta/text") {
                *t = serde_json::Value::String(new_text.to_string());
            }
        }
        Provider::OpenAI => {
            if let Some(t) = v.pointer_mut("/choices/0/delta/content") {
                *t = serde_json::Value::String(new_text.to_string());
            }
        }
        Provider::Google => {
            if let Some(t) = v.pointer_mut("/candidates/0/content/parts/0/text") {
                *t = serde_json::Value::String(new_text.to_string());
            }
        }
        Provider::Unknown => {}
    }
    serde_json::to_string(&v).unwrap_or_else(|_| data.to_string())
}

fn make_trailing_text_event(provider: Provider, text: &str) -> Option<Vec<u8>> {
    if text.is_empty() { return None; }
    let data = match provider {
        Provider::Anthropic => serde_json::json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": text }
        }).to_string(),
        Provider::OpenAI => serde_json::json!({
            "id": "trailing", "object": "chat.completion.chunk",
            "choices": [{ "delta": { "content": text }, "index": 0, "finish_reason": null }]
        }).to_string(),
        _ => return None,
    };
    Some(reconstruct_sse_event_bytes(Some("content_block_delta"), &data))
}

// ─── finalize_response ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn finalize_response(
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    shared_vault: &Arc<Mutex<Option<VaultHandle>>>,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    provider: Provider,
    accumulated: &mut String,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    pii_replace: bool,
) {
    let conv_id = shared_conv_id.lock().unwrap().clone();
    let Some(cid) = conv_id else { return; };

    let content = std::mem::take(accumulated);
    if content.is_empty() {
        let _ = ws_tx.send(WsEvent::ResponseComplete { conversation_id: cid, tokens_in, tokens_out });
        return;
    }

    let ts = now_iso8601();
    let msg = Message {
        id: new_uuid(),
        conversation_id: cid.clone(),
        direction: "response".to_string(),
        timestamp: ts,
        role: Some("assistant".to_string()),
        content,
        tokens_in,
        tokens_out,
    };

    let store_clone = store.clone();
    let msg_clone = msg.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || store_clone.insert_message(&msg_clone)).await {
        tracing::warn!("Failed to store response: {}", e);
    }

    // Persist vault after stream completion.
    if pii_replace {
        if let Some(vh) = shared_vault.lock().unwrap().clone() {
            let vault = vh.read().unwrap();
            if !vault.is_empty() {
                let records: Vec<(String, String)> = vault
                    .pairs()
                    .map(|(o, s)| (o.to_string(), s.to_string()))
                    .collect();
                let seed = vault.rng_seed;
                drop(vault);
                let store_clone = store.clone();
                let cid_clone = cid.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    if let Err(e) = store_clone.save_vault(&cid_clone, seed, &records) {
                        tracing::warn!("Failed to save vault: {}", e);
                    }
                });
            }
        }
    }

    let _ = ws_tx.send(WsEvent::ResponseComplete { conversation_id: cid, tokens_in, tokens_out });
}

// ─── Phase 1 process_response_chunk ──────────────────────────────────────────

fn process_response_chunk(
    chunk: &[u8],
    is_sse: bool,
    sse_parser: &mut SseParser,
    accumulated: &mut String,
    accumulation_stopped: &mut bool,
    provider: Provider,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    tokens_in: &mut Option<i64>,
    tokens_out: &mut Option<i64>,
    body_buf: &mut Vec<u8>,
) -> bool {
    if !is_sse {
        body_buf.extend_from_slice(chunk);
        return false;
    }

    let cid_opt = shared_conv_id.lock().unwrap().clone();

    for event in sse_parser.push(chunk) {
        if SseParser::is_done_sentinel(&event) { return true; }

        if provider == Provider::Anthropic {
            if event.event_type.as_deref() == Some("message_stop") { return true; }
            let (ti, to) = crate::parser::anthropic::extract_message_start_tokens(&event);
            if ti.is_some() { *tokens_in = ti; }
            if to.is_some() { *tokens_out = to; }
        }

        if let Some(delta) = parser::extract_sse_delta(provider, &event) {
            if delta.is_empty() { continue; }
            if !*accumulation_stopped {
                accumulated.push_str(&delta);
                if accumulated.len() > MAX_SSE_BUFFER {
                    *accumulation_stopped = true;
                    tracing::warn!("SSE accumulation buffer limit reached");
                }
            }
            let cid = cid_opt.clone().unwrap_or_else(new_uuid);
            let _ = ws_tx.send(WsEvent::TextDelta {
                conversation_id: cid,
                text: delta,
                timestamp: now_iso8601(),
            });
        }
    }

    false
}

// ─── log_request ─────────────────────────────────────────────────────────────

async fn log_request(
    body: &[u8],
    provider: Provider,
    host: &str,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
) {
    let Some(parsed) = parser::parse_request(provider, body) else {
        tracing::debug!("Could not parse request body for {} ({} bytes)", host, body.len());
        return;
    };

    let fingerprint = conversation_fingerprint(&parsed.messages);
    let store_clone = store.clone();
    let provider_str = provider.as_str().to_string();
    let model = parsed.model.clone();
    let messages = parsed.messages.clone();

    let result = tokio::task::spawn_blocking(move || {
        let (conv_id, msg_offset, is_new) =
            match store_clone.find_conversation_by_fingerprint(&provider_str, &fingerprint) {
                Some(existing_id) => {
                    let stored = store_clone.count_request_messages(&existing_id);
                    tracing::debug!(conv_id = %existing_id, stored_msgs = stored, total_msgs = messages.len(), "Continuing conversation");
                    (existing_id, stored, false)
                }
                None => {
                    let new_id = new_uuid();
                    let ts = now_iso8601();
                    let conv = Conversation {
                        id: new_id.clone(),
                        started_at: ts,
                        provider: provider_str.clone(),
                        model: Some(model.clone()),
                        client_hint: Some(fingerprint),
                    };
                    if let Err(e) = store_clone.insert_conversation(&conv) {
                        tracing::warn!("Failed to store conversation: {}", e);
                    }
                    tracing::info!(conv_id = %new_id, provider = %provider_str, model = %model, "New conversation");
                    (new_id, 0, true)
                }
            };

        let new_messages = messages.get(msg_offset..).unwrap_or(&[]);
        if new_messages.is_empty() {
            return Ok::<_, anyhow::Error>((conv_id, is_new, model, vec![]));
        }

        let ts = now_iso8601();
        let stored_msgs: Vec<Message> = new_messages.iter().map(|msg| Message {
            id: new_uuid(),
            conversation_id: conv_id.clone(),
            direction: "request".to_string(),
            timestamp: ts.clone(),
            role: Some(msg.role.clone()),
            content: msg.content.clone(),
            tokens_in: None,
            tokens_out: None,
        }).collect();

        if let Err(e) = store_clone.batch_insert_messages(&stored_msgs) {
            tracing::warn!("Failed to store request messages: {}", e);
        }

        Ok::<_, anyhow::Error>((conv_id, is_new, model, stored_msgs))
    }).await;

    let Ok(Ok((conv_id, is_new, model, stored_msgs))) = result else { return; };

    *shared_conv_id.lock().unwrap() = Some(conv_id.clone());

    if is_new {
        let ts = now_iso8601();
        let _ = ws_tx.send(WsEvent::ConversationStart {
            id: conv_id.clone(),
            provider: provider.as_str().to_string(),
            model,
            timestamp: ts,
        });
    }

    for msg in stored_msgs {
        let _ = ws_tx.send(WsEvent::Message {
            conversation_id: conv_id.clone(),
            direction: "request".to_string(),
            role: msg.role,
            content: msg.content,
            timestamp: msg.timestamp,
        });
    }
}

fn conversation_fingerprint(messages: &[crate::parser::Message]) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    if let Some(first) = messages.first() {
        h.update(first.role.as_bytes());
        h.update(b":");
        h.update(first.content.as_bytes());
    }
    format!("{:x}", h.finalize())
}

// ─── HTTP helpers ─────────────────────────────────────────────────────────────

fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i+1] == b'\n' && data[i+2] == b'\r' && data[i+3] == b'\n' {
            return Some(i + 4);
        }
    }
    None
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(headers).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.to_lowercase().strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}
