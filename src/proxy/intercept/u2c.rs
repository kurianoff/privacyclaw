use crate::dashboard::WsEvent;
use crate::parser::{self, Provider};
use crate::parser::sse::SseParser;
use crate::pii::{PiiCtx, PiiMode};
use crate::pii::buffer::ReplacementBuffer;
use crate::pii::vault::VaultHandle;
use crate::storage::{Message, Store};
use crate::util::{new_uuid, now_iso8601};
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot};

use super::{MAX_SSE_BUFFER, READ_BUF, UPSTREAM_READ_TIMEOUT};
use super::backoff::{get_vault_with_backoff, wait_for_conv_id};
use super::framing::{find_header_end, is_chunked_encoding, parse_content_length, write_http_chunk};
use super::framing::{ChunkedDecoder, find_chunked_body_end};
use super::pii_sse::{make_trailing_text_event, process_sse_chunk_pii};

// ─── Upstream → Client ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_u2c(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    shared_vault: Arc<Mutex<Option<VaultHandle>>>,
    mut shutdown: oneshot::Receiver<()>,
    upstream_gone: Arc<AtomicBool>,
    pii: PiiCtx,
    upstream_shutdown_tx: oneshot::Sender<()>,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let pii_replace = pii.as_ref()
        .map(|p| p.mode == PiiMode::Replace)
        .unwrap_or(false);

    // Per-response state.
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut is_sse = false;
    let mut is_chunked_resp = false;
    let mut sse_parser = SseParser::new();
    let mut accumulated = String::new();
    let mut synthetic_accumulated = String::new(); // raw LLM text before PII restoration
    let mut accumulation_stopped = false;
    let mut tokens_in: Option<i64> = None;
    let mut tokens_out: Option<i64> = None;
    let mut body_buf: Vec<u8> = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut body_received: usize = 0;
    let mut rep_buf: Option<ReplacementBuffer> = None;
    let mut chunk_decoder: Option<ChunkedDecoder> = None;

    loop {
        let n = tokio::select! {
            result = tokio::time::timeout(UPSTREAM_READ_TIMEOUT, reader.read(&mut buf)) => {
                match result {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => {
                        tracing::debug!("u2c: upstream read error: {}", e);
                        upstream_gone.store(true, Ordering::Release);
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(timeout_secs = UPSTREAM_READ_TIMEOUT.as_secs(), "u2c: idle timeout");
                        upstream_gone.store(true, Ordering::Release);
                        break;
                    }
                }
            }
            _ = &mut shutdown => { tracing::debug!("u2c: client closed"); break; }
        };
        if n == 0 {
            upstream_gone.store(true, Ordering::Release);
            break;
        }
        let chunk = &buf[..n];

        let sse_done = if !header_done {
            raw.extend_from_slice(chunk);
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                let headers_text = String::from_utf8_lossy(&raw[..hdr_end]);
                is_sse = headers_text.contains("text/event-stream");
                content_length = parse_content_length(&raw[..hdr_end]);
                is_chunked_resp = content_length.is_none()
                    && is_chunked_encoding(&raw[..hdr_end]);
                // If Anthropic signals it will close the connection, mark it so
                // c2u doesn't attempt to send another request on the dead socket.
                let connection_close = headers_text.lines().any(|line| {
                    let lo = line.to_lowercase();
                    lo.starts_with("connection:") && lo.contains("close")
                });
                if connection_close {
                    upstream_gone.store(true, Ordering::Release);
                    tracing::debug!("u2c: upstream signaled Connection: close");
                }
                tracing::debug!(
                    is_sse,
                    content_length = ?content_length,
                    is_chunked_resp,
                    "u2c: response headers done"
                );
                tracing::trace!(
                    headers = %String::from_utf8_lossy(&raw[..hdr_end]),
                    "u2c: response headers raw"
                );

                // Initialize ReplacementBuffer when PII replace + SSE.
                tracing::debug!(pii_replace, is_sse, "u2c_pii: response headers parsed");
                if pii_replace && is_sse {
                    let vault = get_vault_with_backoff(&shared_vault).await;
                    if let Some(vh) = vault {
                        let mapping_count = vh.read().map(|v| v.mapping_count()).unwrap_or(0);
                        tracing::debug!(mapping_count, "u2c_pii: ReplacementBuffer initialized with vault");
                        rep_buf = Some(ReplacementBuffer::new(vh));
                    } else {
                        tracing::warn!("u2c_pii: vault is None — ReplacementBuffer NOT initialized, inbound PII will NOT be decoded");
                    }
                    if is_chunked_resp {
                        chunk_decoder = Some(ChunkedDecoder::new());
                        tracing::debug!("u2c_pii: ChunkedDecoder initialized for SSE+chunked response");
                    }
                }

                // Always forward the headers to the client first.
                writer.write_all(&raw[..hdr_end]).await?;
                writer.flush().await?;

                if hdr_end < raw.len() {
                    body_received += raw.len() - hdr_end;
                    let body_chunk = raw[hdr_end..].to_vec();
                    raw.clear(); raw.shrink_to_fit();
                    if pii_replace && is_sse {
                        let decoded = if let Some(ref mut cd) = chunk_decoder {
                            cd.push(&body_chunk)
                        } else {
                            body_chunk
                        };
                        if decoded.is_empty() {
                            false
                        } else {
                            process_sse_chunk_pii(
                                &decoded, &mut sse_parser, &mut rep_buf,
                                &mut accumulated, &mut synthetic_accumulated,
                                &mut accumulation_stopped,
                                provider, &ws_tx, &shared_conv_id,
                                &mut tokens_in, &mut tokens_out, &mut writer,
                            ).await?
                        }
                    } else {
                        writer.write_all(&body_chunk).await?;
                        writer.flush().await?;
                        process_response_chunk(
                            &body_chunk, is_sse, &mut sse_parser, &mut accumulated,
                            &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                            &mut tokens_in, &mut tokens_out, &mut body_buf,
                        )
                    }
                } else {
                    raw.clear(); raw.shrink_to_fit();
                    false
                }
            } else {
                false
            }
        } else {
            body_received += chunk.len();
            tracing::trace!(
                chunk_len = chunk.len(),
                is_sse,
                pii_replace,
                body_received,
                hex = %crate::util::fmt_chunk_hex(chunk, 256),
                "u2c: upstream body chunk"
            );
            if pii_replace && is_sse {
                let decoded = if let Some(ref mut cd) = chunk_decoder {
                    cd.push(chunk)
                } else {
                    chunk.to_vec()
                };
                if decoded.is_empty() {
                    false
                } else {
                    process_sse_chunk_pii(
                        &decoded, &mut sse_parser, &mut rep_buf,
                        &mut accumulated, &mut synthetic_accumulated,
                        &mut accumulation_stopped,
                        provider, &ws_tx, &shared_conv_id,
                        &mut tokens_in, &mut tokens_out, &mut writer,
                    ).await?
                }
            } else {
                writer.write_all(chunk).await?;
                writer.flush().await?;
                process_response_chunk(
                    chunk, is_sse, &mut sse_parser, &mut accumulated,
                    &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                    &mut tokens_in, &mut tokens_out, &mut body_buf,
                )
            }
        };

        let chunked_resp_done = is_chunked_resp && !is_sse
            && find_chunked_body_end(&body_buf).is_some();
        let response_complete = sse_done
            || (!is_sse && content_length.is_some_and(|cl| body_received >= cl))
            || chunked_resp_done;

        if response_complete && header_done {
            let was_sse = is_sse;
            flush_rep_buf_and_finalize(
                &mut rep_buf, &mut accumulated, &mut synthetic_accumulated,
                &mut accumulation_stopped,
                provider, &ws_tx, &shared_conv_id, &shared_vault,
                &store, tokens_in, tokens_out, pii_replace, &mut writer,
            ).await?;
            // Close the chunked response when we rewrote SSE events as chunks.
            if pii_replace && was_sse {
                writer.write_all(b"0\r\n\r\n").await?;
                writer.flush().await?;
            }

            header_done = false;
            is_sse = false;
            is_chunked_resp = false;
            sse_parser = SseParser::new();
            accumulated = String::new();
            synthetic_accumulated = String::new();
            accumulation_stopped = false;
            tokens_in = None;
            tokens_out = None;
            body_buf = Vec::new();
            content_length = None;
            body_received = 0;
            rep_buf = None;
            chunk_decoder = None;
        }
    }

    flush_rep_buf_and_finalize(
        &mut rep_buf, &mut accumulated, &mut synthetic_accumulated,
        &mut accumulation_stopped,
        provider, &ws_tx, &shared_conv_id, &shared_vault,
        &store, tokens_in, tokens_out, pii_replace, &mut writer,
    ).await?;

    // Signal c_to_u that the upstream is gone so it can abort waiting for new data.
    let _ = upstream_shutdown_tx.send(());
    // Send TLS close_notify to the client so it gets a clean session termination.
    let _ = writer.shutdown().await;

    Ok(())
}

/// Flush ReplacementBuffer then finalize the response.
#[allow(clippy::too_many_arguments)]
pub(super) async fn flush_rep_buf_and_finalize(
    rep_buf: &mut Option<ReplacementBuffer>,
    accumulated: &mut String,
    synthetic_accumulated: &mut String,
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
                write_http_chunk(writer, &event_bytes).await?;
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
        accumulated, synthetic_accumulated, tokens_in, tokens_out, pii_replace,
    ).await;
    Ok(())
}

// ─── Phase 1 process_response_chunk ──────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
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

// ─── finalize_response ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn finalize_response(
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    shared_vault: &Arc<Mutex<Option<VaultHandle>>>,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    _provider: Provider,
    accumulated: &mut String,
    synthetic_accumulated: &mut String,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    pii_replace: bool,
) {
    let conv_id = wait_for_conv_id(shared_conv_id).await;
    let Some(cid) = conv_id else { return; };

    let content = std::mem::take(accumulated);
    if content.is_empty() {
        let _ = ws_tx.send(WsEvent::ResponseComplete { conversation_id: cid, tokens_in, tokens_out });
        return;
    }

    let content_masked = {
        let s = std::mem::take(synthetic_accumulated);
        if s.is_empty() { None } else { Some(s) }
    };

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
        content_masked,
        pii_processed: None,
    };

    let store_clone = store.clone();
    let msg_clone = msg.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || store_clone.insert_message(&msg_clone)).await {
        tracing::warn!("Failed to store response: {}", e);
    }

    // Persist vault after stream completion.
    if pii_replace {
        let vh_opt = shared_vault.lock().unwrap().clone();
        if let Some(vh) = vh_opt {
            let vault_data = {
                let vault = vh.read().unwrap();
                if vault.is_empty() {
                    None
                } else {
                    let records: Vec<(String, String, String, u8, f32)> = vault
                        .quints()
                        .map(|(o, s, t, tier, conf)| (o.to_string(), s.to_string(), t.to_string(), tier, conf))
                        .collect();
                    Some((vault.rng_seed, records))
                }
            };
            if let Some((seed, records)) = vault_data {
                let store_clone = store.clone();
                let cid_clone = cid.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    if let Err(e) = store_clone.save_vault(&cid_clone, seed, &records) {
                        tracing::warn!("Failed to save vault: {}", e);
                    }
                }).await {
                    tracing::warn!("save_vault task panicked: {}", e);
                }
            }
        }
    }

    let _ = ws_tx.send(WsEvent::ResponseComplete { conversation_id: cid, tokens_in, tokens_out });
}
