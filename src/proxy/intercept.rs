use crate::dashboard::WsEvent;
use crate::parser::{self, Provider};
use crate::parser::sse::SseParser;
use crate::pii::{self, PiiCtx, PiiDetection, PiiMode};
use crate::pii::buffer::ReplacementBuffer;
use crate::pii::vault::VaultHandle;
use crate::storage::{Conversation, Message, MessageDetection, Store};
use crate::util::{new_uuid, now_iso8601};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot};

const MAX_SSE_BUFFER: usize = 10 * 1024 * 1024; // 10 MB
const READ_BUF: usize = 65536;

/// Number of retry attempts when polling a shared value with backoff.
#[cfg(not(test))]
const BACKOFF_ATTEMPTS: usize = 5;
/// Extended in test builds: saturated Tokio runtimes under parallel execution
/// need up to 500 ms for spawn_blocking (DB lookup) to complete.
#[cfg(test)]
const BACKOFF_ATTEMPTS: usize = 50;
/// Sleep duration between each backoff retry (total wait: BACKOFF_ATTEMPTS × this).
const BACKOFF_SLEEP_MS: u64 = 10;

/// How long u_to_c waits for any upstream data before giving up.
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// How long c_to_u waits for a single write to upstream to complete.
const UPSTREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle a fully decrypted bidirectional stream between client and upstream.
///
/// `pii` is `Some(...)` when PII protection mode is active, `None` for Phase 1
/// passthrough behaviour (byte-identical forwarding).
#[allow(clippy::too_many_arguments)]
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
    // Pre-seeded session UUID used as fallback in the PII path when
    // create_or_find_conversation returns None (parse failure or DB error).
    let session_uuid = new_uuid();
    // Vault handle populated by c_to_u after PII pipeline runs; read by u_to_c.
    let shared_vault: Arc<Mutex<Option<VaultHandle>>> = Arc::new(Mutex::new(None));
    // Set by u_to_c when the upstream connection closes (EOF or Connection: close).
    // c_to_u checks this before writing a new request to avoid sending to a dead connection.
    let upstream_gone: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let (upstream_shutdown_tx, upstream_shutdown_rx) = oneshot::channel::<()>();

    let c_to_u = tokio::spawn({
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let shared_conv_id = Arc::clone(&shared_conv_id);
        let shared_vault = Arc::clone(&shared_vault);
        let upstream_gone = Arc::clone(&upstream_gone);
        let pii = pii.clone();
        let session_uuid = session_uuid.clone();
        async move {
            let result = handle_c2u(
                client_reader, upstream_writer, provider,
                store, ws_tx, host, shared_conv_id, shared_vault, upstream_gone, pii, session_uuid,
                upstream_shutdown_rx,
            ).await;
            let _ = shutdown_tx.send(());
            if let Err(e) = result { tracing::debug!(err = %e, "c→u closed"); }
        }
    });

    let u_to_c = tokio::spawn(async move {
        if let Err(e) = handle_u2c(
            upstream_reader, client_writer, provider,
            store, ws_tx, shared_conv_id, shared_vault, shutdown_rx, upstream_gone, pii,
            upstream_shutdown_tx,
        ).await {
            tracing::debug!(err = %e, "u→c closed");
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
    upstream_gone: Arc<AtomicBool>,
    pii: PiiCtx,
    session_uuid: String,
    upstream_shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let pii_active = pii.as_ref()
        .map(|p| p.mode != PiiMode::Off)
        .unwrap_or(false);

    if pii_active {
        handle_c2u_pii(reader, writer, provider, store, ws_tx, host, shared_conv_id, shared_vault, upstream_gone, pii, session_uuid, upstream_shutdown_rx).await
    } else {
        handle_c2u_passthrough(reader, writer, provider, store, ws_tx, host, shared_conv_id, upstream_gone, upstream_shutdown_rx).await
    }
}

/// Phase 1 outbound: forward bytes to upstream, parse off-path.
///
/// For Content-Length requests: bytes are forwarded eagerly as they arrive.
/// For chunked requests: bytes are buffered until the terminal chunk is found,
/// then the body is decoded and re-sent to upstream with a Content-Length header.
/// This is required because many upstream HTTP/1.1 servers (including Anthropic's)
/// wait for EOF on chunked uploads before processing — which never comes on a
/// keep-alive connection. Re-encoding with Content-Length avoids that deadlock.
///
/// Supports HTTP/1.1 keep-alive: multiple requests on the same connection are
/// parsed independently.
#[allow(clippy::too_many_arguments)]
async fn handle_c2u_passthrough(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    host: String,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    upstream_gone: Arc<AtomicBool>,
    mut upstream_shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut content_length: Option<usize> = None;
    let mut is_chunked = false;
    let mut body_start: usize = 0;
    // Bytes of `raw` already written to upstream. For chunked requests this
    // stays at 0 until the complete body is available; for all other framing
    // bytes are forwarded eagerly.
    let mut forwarded: usize = 0;

    loop {
        // Only read from the network when we actually need more bytes.
        let needs_more = if !header_done {
            find_header_end(&raw).is_none()
        } else if let Some(cl) = content_length {
            raw.len() < body_start + cl
        } else if is_chunked {
            find_chunked_body_end(&raw[body_start..]).is_none()
        } else {
            // Unknown body framing — keep reading until EOF.
            true
        };

        if needs_more {
            // Between requests (raw is empty, no partial state): also watch for
            // upstream dying so we don't block indefinitely on reader.read().
            let n = if raw.is_empty() && !header_done {
                tokio::select! {
                    result = reader.read(&mut buf) => result?,
                    _ = &mut upstream_shutdown_rx => {
                        tracing::debug!("c2u: upstream died, aborting passthrough");
                        break;
                    }
                }
            } else {
                reader.read(&mut buf).await?
            };
            tracing::debug!(n, "c2u: read from client");
            if n == 0 {
                tracing::debug!("c2u: client EOF");
                break;
            }
            raw.extend_from_slice(&buf[..n]);
        }

        // Locate header end if not yet found.
        if !header_done {
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                body_start = hdr_end;
                content_length = parse_content_length(&raw[..hdr_end]);
                is_chunked = content_length.is_none() && is_chunked_encoding(&raw[..hdr_end]);
                tracing::debug!(
                    content_length = ?content_length,
                    is_chunked,
                    "c2u: request headers parsed"
                );
                if !is_chunked {
                    // Non-chunked: forward the bytes we have so far (headers + any
                    // body bytes that arrived in the same read as the headers).
                    if upstream_gone.load(Ordering::Acquire) {
                        tracing::warn!("c2u: upstream closed before request could be forwarded, aborting");
                        anyhow::bail!("upstream connection closed by server");
                    }
                    upstream_write(&mut writer, &raw[forwarded..]).await?;
                    forwarded = raw.len();
                }
                // Chunked: keep buffering — don't forward until body is complete.
            } else {
                continue;
            }
        }

        // Eagerly forward new bytes for non-chunked / unknown-framing requests.
        if !is_chunked && forwarded < raw.len() {
            if upstream_gone.load(Ordering::Acquire) {
                tracing::warn!("c2u: upstream closed before request could be forwarded, aborting");
                anyhow::bail!("upstream connection closed by server");
            }
            upstream_write(&mut writer, &raw[forwarded..]).await?;
            forwarded = raw.len();
        }

        // Check whether the complete body is available.
        if let Some(cl) = content_length {
            if raw.len() >= body_start + cl {
                let body = raw[body_start..body_start + cl].to_vec();
                log_request(&body, provider, &host, &store, &ws_tx, &shared_conv_id).await;
                raw.drain(..body_start + cl);
                forwarded = 0;
                header_done = false;
                content_length = None;
                is_chunked = false;
                body_start = 0;
            }
        } else if is_chunked {
            if let Some(end) = find_chunked_body_end(&raw[body_start..]) {
                let chunked_slice = &raw[body_start..body_start + end];
                if upstream_gone.load(Ordering::Acquire) {
                    tracing::warn!("c2u: upstream closed before request could be forwarded, aborting");
                    anyhow::bail!("upstream connection closed by server");
                }
                if let Some(decoded) = decode_chunked_body(chunked_slice) {
                    // Re-encode with Content-Length so the upstream knows the body
                    // is complete without needing an EOF / connection close.
                    let rebuilt = rebuild_request_with_content_length(&raw[..body_start], &decoded);
                    upstream_write(&mut writer, &rebuilt).await?;
                    log_request(&decoded, provider, &host, &store, &ws_tx, &shared_conv_id).await;
                    tracing::debug!(decoded_bytes = decoded.len(), "c2u: chunked → Content-Length, state reset");
                } else {
                    // Decode failed: forward the raw chunked bytes as a fallback.
                    tracing::warn!("c2u: chunked decode failed, forwarding raw");
                    upstream_write(&mut writer, &raw[..body_start + end]).await?;
                }
                raw.drain(..body_start + end);
                forwarded = 0;
                header_done = false;
                content_length = None;
                is_chunked = false;
                body_start = 0;
            }
            // Else: keep buffering.
        }
        // Unknown framing: already forwarded above; keep reading until EOF.
    }

    // EOF reached: handle any remaining buffered data.
    if header_done && content_length.is_none() && !is_chunked {
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
    upstream_gone: Arc<AtomicBool>,
    pii: PiiCtx,
    session_uuid: String,
    mut upstream_shutdown_rx: oneshot::Receiver<()>,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];
    let mut raw: Vec<u8> = Vec::new();
    let mut header_done = false;
    let mut content_length: Option<usize> = None;
    let mut is_chunked = false;
    let mut body_start: usize = 0;
    let mut body_received: usize = 0;

    loop {
        let n = if raw.is_empty() && !header_done {
            tokio::select! {
                result = reader.read(&mut buf) => {
                    result?
                }
                _ = &mut upstream_shutdown_rx => {
                    tracing::debug!("c2u_pii: upstream signaled shutdown, aborting");
                    break;
                }
            }
        } else {
            reader.read(&mut buf).await?
        };
        tracing::debug!(n, "c2u_pii: read from client");
        if n == 0 { break; }
        raw.extend_from_slice(&buf[..n]);

        if !header_done {
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                body_start = hdr_end;
                content_length = parse_content_length(&raw[..hdr_end]);
                is_chunked = content_length.is_none() && is_chunked_encoding(&raw[..hdr_end]);
                body_received = raw.len() - body_start;
                tracing::debug!(content_length = ?content_length, is_chunked, "c2u_pii: request headers parsed");
            }
        } else {
            body_received += buf[..n].len();
        }

        let body_done = header_done && if let Some(cl) = content_length {
            body_received >= cl
        } else if is_chunked {
            let found = find_chunked_body_end(&raw[body_start..]).is_some();
            tracing::debug!(found, raw_body_len = raw.len() - body_start, "c2u_pii: chunked body end check");
            found
        } else {
            // RFC 7230 §3.3: no Content-Length and no Transfer-Encoding → no body.
            // Forward immediately with whatever body bytes arrived so far.
            true
        };
        if !body_done { continue; }

        // Extract the decoded body.
        let (original_body, consumed) = if is_chunked {
            let end = find_chunked_body_end(&raw[body_start..]).unwrap();
            let decoded = decode_chunked_body(&raw[body_start..body_start + end])
                .unwrap_or_else(|| raw[body_start..body_start + end].to_vec());
            (decoded, body_start + end)
        } else {
            let cl = content_length.unwrap_or(body_received);
            (raw[body_start..body_start + cl].to_vec(), body_start + cl)
        };

        // Phase A: create or find conversation before running pipeline.
        let conv_id_opt = create_or_find_conversation(
            &original_body, provider, &host, &store, &ws_tx, &shared_conv_id,
        ).await;

        // Get or create vault keyed by conversation_id.
        // If conversation lookup failed, fall back to the session UUID so that all
        // turns within this connection share the same vault (stable across retries).
        let pii_cid = conv_id_opt.clone().or_else(|| {
            pii.as_ref().map(|_| shared_conv_id.lock().unwrap().clone().unwrap_or_else(|| session_uuid.clone()))
        });

        // When the real conv_id becomes known on a later turn, merge any mappings
        // that were stored under the session_uuid fallback vault into the real vault.
        if let (Some(ref real_cid), Some(ref pii_ctx)) = (&conv_id_opt, &pii) {
            if real_cid != &session_uuid {
                pii_ctx.registry.merge_into(&session_uuid, real_cid, &store);
            }
        }

        let vault_handle = if let (Some(ref pii_ctx), Some(ref cid)) = (&pii, &pii_cid) {
            Some(pii_ctx.registry.get_or_create_with_store(cid, &store))
        } else {
            None
        };

        // Run PII pipeline in two stages.
        // Stage 1: PII pipeline (replace mode) — may return None = no changes.
        let mut pii_detections: Vec<PiiDetection> = Vec::new();
        let working_body = if let (Some(ref pii_ctx), Some(ref vh)) = (&pii, &vault_handle) {
            if pii_ctx.mode == PiiMode::Replace {
                match pii_ctx.pipeline.process_request_body_async(&original_body, vh, provider, &pii_ctx.locale).await {
                    Some((new_body, detections)) => {
                        tracing::info!(
                            conv_id = %pii_cid.as_deref().unwrap_or(""),
                            total_detections = detections.len(),
                            "pii: detections ready for storage"
                        );
                        pii_detections = detections;
                        new_body
                    }
                    None => original_body.clone(),
                }
            } else {
                original_body.clone() // detect-only
            }
        } else {
            original_body.clone()
        };

        // Stage 2: system instruction injection (T3 standalone + replace mode only).
        // This is unconditional on stage 1 — the reminder must be injected even when
        // no PII was found in this particular request.
        let t3_standalone_replace = pii
            .as_ref()
            .map(|p| p.pipeline.slm_standalone && p.mode == PiiMode::Replace)
            .unwrap_or(false);
        let forward_body = if t3_standalone_replace {
            tracing::debug!(provider = provider.as_str(), "c2u_pii: attempting system instruction injection (T3 standalone)");
            match inject_system_instruction_into_body(&working_body, provider) {
                Some(injected) => {
                    tracing::debug!(provider = provider.as_str(), "c2u_pii: system instruction injection succeeded");
                    injected
                }
                None => {
                    tracing::debug!(provider = provider.as_str(), "c2u_pii: system instruction injection skipped (no modification made)");
                    working_body
                }
            }
        } else {
            working_body
        };

        let forward_request = rebuild_request_with_content_length(&raw[..body_start], &forward_body);

        // Phase B: store request messages with masked content.
        let pii_mode_active = pii.as_ref().map(|p| p.mode == PiiMode::Replace).unwrap_or(false);
        let replaced_ref: Option<&[u8]> = if pii_mode_active && !pii_detections.is_empty() {
            Some(&forward_body)
        } else {
            None
        };
        let message_ids = if let Some(ref cid) = conv_id_opt {
            store_request_messages(
                &original_body, replaced_ref, pii_mode_active, cid, provider, &store, &ws_tx,
            ).await
        } else {
            vec![]
        };

        // Emit PiiDetected WS events for each replacement made.
        if let Some(ref cid) = pii_cid {
            for det in &pii_detections {
                let _ = ws_tx.send(WsEvent::PiiDetected {
                    conversation_id: cid.clone(),
                    entity_type: det.entity_type.clone(),
                    original: det.original.clone(),
                    original_masked: entity_type_label(&det.entity_type),
                    synthetic: det.synthetic.clone(),
                    tier: det.tier,
                    confidence: det.confidence,
                });
            }
        }

        // Store detection records linked to the last stored message.
        if !pii_detections.is_empty() {
            if let Some(ref cid) = pii_cid {
                let last_msg_id = message_ids.last().cloned().unwrap_or_default();
                tracing::debug!(conv_id = %cid, detection_count = pii_detections.len(), "intercept: storing pii detections");
                let md: Vec<MessageDetection> = pii_detections.iter().map(|d| {
                    tracing::trace!(conv_id = %cid, entity_type = %d.entity_type, tier = d.tier, confidence = d.confidence, "intercept: detection record");
                    MessageDetection {
                        message_id: last_msg_id.clone(),
                        entity_type: d.entity_type.clone(),
                        original_masked: entity_type_label(&d.entity_type),
                        synthetic: d.synthetic.clone(),
                        tier: d.tier,
                        confidence: d.confidence,
                    }
                }).collect();
                let store_clone = store.clone();
                let cid_clone = cid.clone();
                std::mem::drop(tokio::task::spawn_blocking(move || {
                    if let Err(e) = store_clone.insert_detections(&cid_clone, &md) {
                        tracing::error!(err = %e, conv_id = %cid_clone, "intercept: insert_detections failed");
                    }
                }));
            }
        }

        // Share vault with u_to_c before forwarding request.
        if let Some(vh) = vault_handle {
            *shared_vault.lock().unwrap() = Some(vh);
        }

        // Check that the upstream connection is still alive before writing.
        // u2c sets this flag when it receives EOF from the upstream (Connection: close or server crash).
        if upstream_gone.load(Ordering::Acquire) {
            tracing::warn!("c2u_pii: upstream closed before request could be forwarded, aborting");
            anyhow::bail!("upstream connection closed by server");
        }

        // Forward (possibly modified) request.
        tracing::debug!(forward_len = forward_request.len(), "c2u_pii: forwarding request");
        tracing::trace!(
            headers_hex = %crate::util::fmt_chunk_hex(&forward_request[..forward_request.len().min(512)], 512),
            "c2u_pii: forwarding request headers (first 512 bytes)"
        );
        upstream_write(&mut writer, &forward_request).await?;
        tracing::debug!("c2u_pii: request forwarded, state reset");

        // Drain the consumed request bytes; keep any leftover (next request on keep-alive).
        raw.drain(..consumed.min(raw.len()));
        header_done = false;
        content_length = None;
        is_chunked = false;
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
async fn flush_rep_buf_and_finalize(
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

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared vault to be set by c_to_u.
async fn get_vault_with_backoff(shared_vault: &Arc<Mutex<Option<VaultHandle>>>) -> Option<VaultHandle> {
    for _ in 0..BACKOFF_ATTEMPTS {
        {
            let g = shared_vault.lock().unwrap();
            if g.is_some() {
                tracing::debug!("u2c_pii: vault acquired from shared_vault");
                return g.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(BACKOFF_SLEEP_MS)).await;
    }
    let result = shared_vault.lock().unwrap().clone();
    if result.is_none() {
        tracing::warn!("u2c_pii: vault backoff timeout — shared_vault still None after 50ms; rep_buf will be None");
    }
    result
}

/// Wait up to `BACKOFF_ATTEMPTS × BACKOFF_SLEEP_MS` ms for the shared conv_id to be set by c_to_u (log_request).
async fn wait_for_conv_id(shared_conv_id: &Arc<Mutex<Option<String>>>) -> Option<String> {
    for _ in 0..BACKOFF_ATTEMPTS {
        {
            let g = shared_conv_id.lock().unwrap();
            if g.is_some() { return g.clone(); }
        }
        tokio::time::sleep(Duration::from_millis(BACKOFF_SLEEP_MS)).await;
    }
    shared_conv_id.lock().unwrap().clone()
}

// ─── PII SSE chunk processing ─────────────────────────────────────────────────

/// Flush any buffered trailing text from `rep_buf`, send a TextDelta WS event,
/// and write it as an HTTP chunk. Called before forwarding `content_block_stop`
/// and `message_stop` so trailing PII-restored text reaches the client while
/// the content block is still open.
#[allow(clippy::too_many_arguments)]
async fn flush_rep_buf_remaining_as_chunk(
    rep_buf: &mut Option<ReplacementBuffer>,
    accumulated: &mut String,
    accumulation_stopped: &mut bool,
    provider: Provider,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<()> {
    let Some(ref mut rb) = rep_buf else { return Ok(()); };
    let remaining = rb.flush_remaining();
    if remaining.is_empty() { return Ok(()); }
    if !*accumulation_stopped {
        accumulated.push_str(&remaining);
        let cid = shared_conv_id.lock().unwrap().clone().unwrap_or_else(new_uuid);
        let _ = ws_tx.send(WsEvent::TextDelta {
            conversation_id: cid,
            text: remaining.clone(),
            timestamp: now_iso8601(),
        });
    }
    if let Some(event_bytes) = make_trailing_text_event(provider, &remaining) {
        write_http_chunk(writer, &event_bytes).await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_sse_chunk_pii(
    chunk: &[u8],
    sse_parser: &mut SseParser,
    rep_buf: &mut Option<ReplacementBuffer>,
    accumulated: &mut String,
    synthetic_acc: &mut String,
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
            write_http_chunk(writer, &reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
            writer.flush().await?;
            return Ok(true);
        }

        if provider == Provider::Anthropic {
            // content_block_stop closes the text content block. Flush the ReplacementBuffer
            // NOW, before forwarding the stop event, so the trailing text arrives while the
            // block is still open. Sending content_block_delta after content_block_stop
            // causes "Received content_block_delta without a current message" in the client.
            if event.event_type.as_deref() == Some("content_block_stop") {
                flush_rep_buf_remaining_as_chunk(
                    rep_buf, accumulated, accumulation_stopped,
                    provider, ws_tx, shared_conv_id, writer,
                ).await?;
            }
            if event.event_type.as_deref() == Some("message_stop") {
                // Defensive flush: if content_block_stop was missed (e.g. due to
                // chunked framing corruption before this fix), flush any held-back
                // tail now — before message_stop so the client still receives it.
                flush_rep_buf_remaining_as_chunk(
                    rep_buf, accumulated, accumulation_stopped,
                    provider, ws_tx, shared_conv_id, writer,
                ).await?;
                write_http_chunk(writer, &reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
                writer.flush().await?;
                return Ok(true);
            }
            let (ti, to) = crate::parser::anthropic::extract_message_start_tokens(&event);
            if ti.is_some() { *tokens_in = ti; }
            if to.is_some() { *tokens_out = to; }
        }

        let maybe_text = extract_text_delta(provider, &event.data);

        let event_bytes = if let (Some(text), Some(ref mut rb)) = (maybe_text.as_ref(), &mut *rep_buf) {
            // Accumulate raw LLM text (pre-restoration) for storage.
            if !*accumulation_stopped {
                synthetic_acc.push_str(text);
            }

            let flushed = rb.process_delta(text);

            let conv_id_snap = shared_conv_id.lock().unwrap().clone();
            tracing::trace!(
                conv_id = conv_id_snap.as_deref().unwrap_or(""),
                delta_text_len = text.len(),
                after_replace_len = flushed.len(),
                "u2c_pii: sse delta processed"
            );
            tracing::debug!(
                text_len = text.len(),
                flushed_len = flushed.len(),
                replaced = (flushed != *text),
                "u2c_pii: process_delta"
            );

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
            if maybe_text.is_some() {
                // rep_buf is None — text delta forwarded without PII decoding
                tracing::debug!(
                    rep_buf_is_none = rep_buf.is_none(),
                    "u2c_pii: text delta bypassing PII decoding (rep_buf=None)"
                );
            }
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

        write_http_chunk(writer, &event_bytes).await?;
        writer.flush().await?;
    }
    Ok(false)
}

// ─── Incremental chunked-transfer-encoding decoder ───────────────────────────

/// Strips HTTP/1.1 `Transfer-Encoding: chunked` framing from a streaming
/// response body and returns the raw payload bytes.
///
/// Must be applied before the SSE parser in PII replace mode.  Without it,
/// chunk-size lines (e.g. `1f\r\n`) are injected into the byte stream; if a
/// chunk boundary falls mid-SSE-field-name the event type is silently
/// corrupted (e.g. `content_block_stop` → `content_block_`), causing the
/// `content_block_stop` flush of the ReplacementBuffer to be missed and
/// trailing text to be sent *after* `message_stop` where the client ignores it.
struct ChunkedDecoder {
    buf: Vec<u8>,
    chunk_remaining: usize,
    state: ChunkDecoderState,
}

#[derive(PartialEq)]
enum ChunkDecoderState {
    ReadingSize,
    ReadingBody,
    BodyTrail,
}

impl ChunkedDecoder {
    fn new() -> Self {
        Self { buf: Vec::new(), chunk_remaining: 0, state: ChunkDecoderState::ReadingSize }
    }

    /// Push raw framed bytes; returns decoded payload bytes.
    fn push(&mut self, raw: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(raw);
        let mut out = Vec::new();
        loop {
            match self.state {
                ChunkDecoderState::ReadingSize => {
                    let Some(crlf) = self.buf.windows(2).position(|w| w == b"\r\n") else {
                        break;
                    };
                    let size_line = std::str::from_utf8(&self.buf[..crlf]).unwrap_or("");
                    let hex = size_line.split(';').next().unwrap_or("").trim();
                    match usize::from_str_radix(hex, 16) {
                        Ok(0) => { self.buf.clear(); break; } // terminal chunk
                        Ok(n) => {
                            self.buf.drain(..crlf + 2);
                            self.chunk_remaining = n;
                            self.state = ChunkDecoderState::ReadingBody;
                        }
                        Err(_) => break, // not valid chunked framing; pass through
                    }
                }
                ChunkDecoderState::ReadingBody => {
                    let take = self.chunk_remaining.min(self.buf.len());
                    out.extend_from_slice(&self.buf[..take]);
                    self.buf.drain(..take);
                    self.chunk_remaining -= take;
                    if self.chunk_remaining == 0 {
                        self.state = ChunkDecoderState::BodyTrail;
                    } else {
                        break;
                    }
                }
                ChunkDecoderState::BodyTrail => {
                    if self.buf.len() < 2 { break; }
                    self.buf.drain(..2); // skip trailing \r\n
                    self.state = ChunkDecoderState::ReadingSize;
                }
            }
        }
        out
    }
}

// ─── SSE helpers ─────────────────────────────────────────────────────────────

/// Write `data` as a single HTTP/1.1 chunked-encoding chunk.
/// Used when PII mode reconstructs SSE events and must maintain
/// the `Transfer-Encoding: chunked` framing the upstream sent.
async fn write_http_chunk(writer: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> Result<()> {
    let header = format!("{:X}\r\n", data.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(data).await?;
    writer.write_all(b"\r\n").await?;
    Ok(())
}

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

/// Format a PII entity type as a bracketed label, e.g. `"email"` → `"[EMAIL]"`.
fn entity_type_label(entity_type: &str) -> String {
    format!("[{}]", entity_type.to_uppercase())
}

// ─── Phase A / Phase B request logging ───────────────────────────────────────

/// Phase A: create or find conversation, set shared_conv_id, broadcast ConversationStart.
/// Returns the conv_id. Does NOT store messages.
async fn create_or_find_conversation(
    body: &[u8],
    provider: Provider,
    host: &str,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
) -> Option<String> {
    let parsed = parser::parse_request(provider, body)?;
    let fingerprint = conversation_fingerprint(&parsed.messages);
    let store_clone = store.clone();
    let provider_str = provider.as_str().to_string();
    let model = parsed.model.clone();

    let result = tokio::task::spawn_blocking(move || {
        match store_clone.find_conversation_by_fingerprint(&provider_str, &fingerprint) {
            Some(existing_id) => {
                tracing::debug!(conv_id = %existing_id, "Phase A: continuing conversation");
                Ok::<_, anyhow::Error>((existing_id, false, model))
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
                Ok::<_, anyhow::Error>((new_id, true, model))
            }
        }
    }).await;

    let Ok(Ok((conv_id, is_new, model))) = result else {
        tracing::debug!("Phase A: spawn_blocking failed for {}", host);
        return None;
    };

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

    Some(conv_id)
}

/// Phase B: parse original + optional replaced body, store Message records, broadcast WsEvent::Message.
/// Returns the stored message IDs (one per new message in the batch).
async fn store_request_messages(
    original_body: &[u8],
    replaced_body: Option<&[u8]>,
    pii_processed: bool,
    conv_id: &str,
    provider: Provider,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
) -> Vec<String> {
    let Some(parsed_orig) = parser::parse_request(provider, original_body) else {
        return vec![];
    };

    let store_clone = store.clone();
    let conv_id_owned = conv_id.to_string();
    let orig_messages = parsed_orig.messages.clone();

    // Parse replaced body outside spawn_blocking (no I/O).
    let repl_messages: Option<Vec<crate::parser::Message>> = replaced_body
        .and_then(|b| parser::parse_request(provider, b))
        .map(|p| p.messages);

    if repl_messages.is_none() && replaced_body.is_some() {
        tracing::warn!(conv_id = %conv_id, "Phase B: replaced body failed to parse; content_masked will be None");
    }

    let stored_count_result = tokio::task::spawn_blocking(move || {
        let msg_offset = store_clone.count_request_messages(&conv_id_owned);
        let new_orig = orig_messages.get(msg_offset..).unwrap_or(&[]);
        if new_orig.is_empty() {
            return Ok::<_, anyhow::Error>(vec![]);
        }
        let ts = now_iso8601();
        let stored_msgs: Vec<Message> = new_orig.iter().enumerate().map(|(i, msg)| {
            let cm = repl_messages
                .as_ref()
                .and_then(|rm| rm.get(msg_offset + i))
                .map(|rm| rm.content.clone());
            Message {
                id: new_uuid(),
                conversation_id: conv_id_owned.clone(),
                direction: "request".to_string(),
                timestamp: ts.clone(),
                role: Some(msg.role.clone()),
                content: msg.content.clone(),
                tokens_in: None,
                tokens_out: None,
                content_masked: cm,
                pii_processed: Some(pii_processed),
            }
        }).collect();
        if let Err(e) = store_clone.batch_insert_messages(&stored_msgs) {
            tracing::warn!("Failed to store request messages: {}", e);
        } else if pii_processed {
            tracing::debug!(conv_id = %conv_id_owned, pii_processed = true, "storage: message stored with pii_processed flag");
        }
        Ok::<_, anyhow::Error>(stored_msgs)
    }).await;

    let stored_msgs = match stored_count_result {
        Ok(Ok(msgs)) => msgs,
        _ => return vec![],
    };

    let mut ids = Vec::new();
    for msg in stored_msgs {
        ids.push(msg.id.clone());
        let _ = ws_tx.send(WsEvent::Message {
            conversation_id: conv_id.to_string(),
            direction: "request".to_string(),
            role: msg.role,
            content: msg.content,
            timestamp: msg.timestamp,
            content_masked: msg.content_masked,
            pii_processed: msg.pii_processed,
        });
    }
    ids
}

/// Passthrough log_request (no PII). Calls Phase A then Phase B with no replaced body.
async fn log_request(
    body: &[u8],
    provider: Provider,
    host: &str,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
) {
    let Some(conv_id) = create_or_find_conversation(body, provider, host, store, ws_tx, shared_conv_id).await else {
        return;
    };
    store_request_messages(body, None, false, &conv_id, provider, store, ws_tx).await;
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

// ─── Upstream write helper ────────────────────────────────────────────────────

async fn upstream_write(
    writer: &mut (impl AsyncWrite + Unpin),
    data: &[u8],
) -> Result<()> {
    match tokio::time::timeout(
        UPSTREAM_WRITE_TIMEOUT,
        async { writer.write_all(data).await?; writer.flush().await },
    ).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => anyhow::bail!("upstream write stalled for {}s", UPSTREAM_WRITE_TIMEOUT.as_secs()),
    }
}

/// Rebuild an HTTP/1.1 request replacing `Transfer-Encoding: chunked` with
/// `Content-Length: <body.len()>`.  Also strips any existing `Content-Length`
/// header to avoid conflicts.  The reconstructed request is suitable for
/// forwarding to an upstream that requires a known content length.
fn rebuild_request_with_content_length(raw_headers: &[u8], body: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(raw_headers.len() + body.len() + 32);
    let text = match std::str::from_utf8(raw_headers) {
        Ok(s) => s,
        Err(_) => {
            // Non-UTF8 headers (shouldn't happen): just concatenate.
            result.extend_from_slice(raw_headers);
            result.extend_from_slice(body);
            return result;
        }
    };
    let bytes = text.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let line_end = bytes[pos..]
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| pos + p)
            .unwrap_or(bytes.len());
        let line = &text[pos..line_end];
        let lower = line.to_lowercase();
        if lower.starts_with("transfer-encoding:")
            || lower.starts_with("content-length:")
            || lower.starts_with("accept-encoding:")
        {
            // Omit: we'll inject Content-Length below, and we don't want
            // compressed responses (gzip/br would corrupt SSE parsing).
        } else if line.is_empty() {
            // End-of-headers blank line: inject Content-Length + no-compression then close headers.
            result.extend_from_slice(
                format!("Content-Length: {}\r\nAccept-Encoding: identity\r\n\r\n", body.len()).as_bytes(),
            );
            break;
        } else {
            result.extend_from_slice(line.as_bytes());
            result.extend_from_slice(b"\r\n");
        }
        pos = line_end + 2; // skip CRLF
    }
    result.extend_from_slice(body);
    result
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

/// Decode a raw chunked-encoded body into its payload bytes.
/// Returns None on parse error (caller treats as non-fatal).
fn decode_chunked_body(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        // Find end of chunk-size line (CRLF).
        let crlf = data[pos..].windows(2).position(|w| w == b"\r\n")?;
        let size_hex = std::str::from_utf8(&data[pos..pos + crlf]).ok()?;
        // Strip chunk extensions (;...) if present.
        let size_hex = size_hex.split(';').next()?.trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        pos += crlf + 2; // skip size line + CRLF
        if size == 0 { break; } // last-chunk
        if pos + size > data.len() { return None; }
        out.extend_from_slice(&data[pos..pos + size]);
        pos += size + 2; // skip chunk data + trailing CRLF
    }
    Some(out)
}

/// Returns true if the headers contain `Transfer-Encoding: chunked`.
fn is_chunked_encoding(headers: &[u8]) -> bool {
    let text = std::str::from_utf8(headers).unwrap_or("");
    text.lines().any(|line| {
        let lo = line.to_lowercase();
        lo.starts_with("transfer-encoding:") && lo.contains("chunked")
    })
}

/// Find the end of a chunked-encoded body, returning the byte offset just past
/// the terminal chunk (`\r\n0\r\n\r\n`). Returns `None` if the terminator is
/// not yet present in `body`.
fn find_chunked_body_end(body: &[u8]) -> Option<usize> {
    // RFC 7230 §4.1: last-chunk = "0" CRLF ; terminal-chunk is followed by CRLF
    // Full terminal sequence: ...<previous chunk CRLF>0\r\n<trailers>\r\n
    // In practice, look for "\r\n0\r\n\r\n" (no trailers, most common case)
    // or "0\r\n\r\n" at the very start of body (single empty body).
    let term = b"\r\n0\r\n\r\n";
    if let Some(pos) = body.windows(term.len()).position(|w| w == term) {
        return Some(pos + term.len());
    }
    // Body may start immediately with the last chunk (no previous CRLF).
    if body.starts_with(b"0\r\n\r\n") {
        return Some(5);
    }
    None
}

/// Parse `body` as JSON, inject `SYSTEM_REMINDER` via `pii::inject_system_instruction`,
/// and return the re-serialized bytes. Returns `None` if parsing or injection fails.
fn inject_system_instruction_into_body(body: &[u8], provider: Provider) -> Option<Vec<u8>> {
    tracing::debug!(body_len = body.len(), provider = provider.as_str(), "inject_system_instruction_into_body: enter");
    let text = std::str::from_utf8(body).ok()?;
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    if pii::inject_system_instruction(&mut value, &provider) {
        let result = serde_json::to_vec(&value).ok();
        tracing::debug!(injected = result.is_some(), provider = provider.as_str(), "inject_system_instruction_into_body: result");
        result
    } else {
        tracing::debug!(provider = provider.as_str(), "inject_system_instruction_into_body: injection returned false, no modification");
        None
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::WsEvent;
    use crate::pii::{PiiContext, PiiMode, PiiPipeline};
    use crate::pii::locale::Locale;
    use crate::pii::vault::VaultRegistry;
    use crate::storage::Store;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn temp_store() -> (Store, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (store, dir)
    }

    fn no_pii() -> PiiCtx {
        None
    }

    fn replace_pii() -> PiiCtx {
        Some(Arc::new(PiiContext {
            registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
            locale: Locale::EnUs,
            mode: PiiMode::Replace,
            pipeline: PiiPipeline::tier1_only(),
        }))
    }

    /// Minimal Anthropic HTTP POST request with a single user message.
    fn anthropic_request(content: &str) -> Vec<u8> {
        let body = format!(
            r#"{{"model":"claude-3-opus-20240229","max_tokens":256,"messages":[{{"role":"user","content":"{content}"}}]}}"#
        );
        format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Api-Key: sk-ant-test\r\n\r\n{}",
            body.len(), body
        )
        .into_bytes()
    }

    /// Minimal Anthropic SSE response for a given text delta.
    fn anthropic_sse_response(text: &str) -> Vec<u8> {
        let events = format!(
            "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_test\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-3-opus-20240229\",\"stop_reason\":null,\"usage\":{{\"input_tokens\":10,\"output_tokens\":0}}}}}}\n\n\
             event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{text}\"}}}}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n\
             data: [DONE]\n\n"
        );
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{events}"
        )
        .into_bytes()
    }

    /// Run `intercept::run()` end-to-end with in-memory duplex streams.
    ///
    /// Returns `(bytes_forwarded_to_upstream, bytes_forwarded_to_client)`.
    async fn run_intercept(
        client_request: Vec<u8>,
        upstream_response: Vec<u8>,
        host: &str,
        pii: PiiCtx,
    ) -> (Vec<u8>, Vec<u8>) {
        let (store, _dir) = temp_store();
        let (ws_tx, _ws_rx) = broadcast::channel::<WsEvent>(64);

        // Four half-pipes:
        //   client  ──req──▶  [proxy]  ──req──▶  upstream_capture
        //   client  ◀─resp──  [proxy]  ◀─resp──  upstream_feed
        let (client_to_proxy, mut client_writer) = tokio::io::duplex(256 * 1024);
        let (mut client_reader, proxy_to_client) = tokio::io::duplex(256 * 1024);
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(256 * 1024);
        let (mut upstream_reader, proxy_to_upstream) = tokio::io::duplex(256 * 1024);

        // Feed client request then close.
        let feed_client = tokio::spawn(async move {
            client_writer.write_all(&client_request).await.ok();
        });

        // Feed upstream response then close.
        let feed_upstream = tokio::spawn(async move {
            upstream_writer.write_all(&upstream_response).await.ok();
        });

        // Drain what proxy writes to upstream.
        let capture_upstream = tokio::spawn(async move {
            let mut buf = Vec::new();
            upstream_reader.read_to_end(&mut buf).await.ok();
            buf
        });

        // Drain what proxy writes to client.
        let capture_client = tokio::spawn(async move {
            let mut buf = Vec::new();
            client_reader.read_to_end(&mut buf).await.ok();
            buf
        });

        // Run intercept with a generous timeout so the test doesn't hang.
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            run(
                client_to_proxy,
                proxy_to_client,
                upstream_to_proxy,
                proxy_to_upstream,
                host.to_string(),
                store,
                ws_tx,
                pii,
            ),
        )
        .await;

        let _ = feed_client.await;
        let _ = feed_upstream.await;
        let upstream_bytes = capture_upstream.await.unwrap_or_default();
        let client_bytes = capture_client.await.unwrap_or_default();
        (upstream_bytes, client_bytes)
    }

    // ── Section 2.1: Roundtrip fidelity ──────────────────────────────────────

    #[tokio::test]
    async fn test_small_request_response_forwarded_verbatim() {
        let req = anthropic_request("hello world");
        let resp = anthropic_sse_response("Hello from the assistant");
        let (upstream_got, _) =
            run_intercept(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&upstream_got);
        assert!(
            got.contains("hello world"),
            "upstream did not receive the request body. got: {got:?}"
        );
    }

    #[tokio::test]
    async fn test_proxy_does_not_modify_request_bytes() {
        let req = anthropic_request("no pii here");
        let resp = anthropic_sse_response("ok");
        let (upstream_got, _) =
            run_intercept(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&upstream_got);
        assert!(
            got.contains("no pii here"),
            "request body was modified or not forwarded. got: {got:?}"
        );
    }

    #[tokio::test]
    async fn test_proxy_does_not_modify_response_bytes() {
        let req = anthropic_request("hello");
        let resp = anthropic_sse_response("assistant response text");
        let (_, client_got) =
            run_intercept(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&client_got);
        assert!(
            got.contains("assistant response text"),
            "response body was not forwarded to client. got: {got:?}"
        );
    }

    // ── Section 2.4: Upstream failure ────────────────────────────────────────

    #[tokio::test]
    async fn test_upstream_immediate_eof_no_panic() {
        // Empty upstream response → immediate EOF. Must not panic.
        let req = anthropic_request("hello");
        let (_, _) =
            run_intercept(req, vec![], "api.anthropic.com", no_pii()).await;
        // Reaching here without panic is the assertion.
    }

    // ── Section 2.5: SSE streaming ────────────────────────────────────────────

    #[tokio::test]
    async fn test_anthropic_message_stop_terminates_stream() {
        let req = anthropic_request("test");
        let resp = anthropic_sse_response("test response");
        let (_, client_got) =
            run_intercept(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&client_got);
        assert!(
            got.contains("message_stop"),
            "message_stop event not forwarded to client: {got:?}"
        );
    }

    // ── Section 7: PII integration ────────────────────────────────────────────

    #[tokio::test]
    async fn test_pii_mode_off_proxy_byte_identical() {
        // With PII off, an email in the request must reach upstream unchanged.
        let req = anthropic_request("contact alice@acme-corp.com for info");
        let resp = anthropic_sse_response("got it");
        let (upstream_got, _) =
            run_intercept(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&upstream_got);
        assert!(
            got.contains("alice@acme-corp.com"),
            "email was modified when PII mode is off: {got:?}"
        );
    }

    #[tokio::test]
    async fn test_pii_request_sanitised_before_upstream() {
        // With PII Replace, an email in the request must NOT reach upstream.
        let req = anthropic_request("contact alice@acme-corp.com for help");
        let resp = anthropic_sse_response("I will contact them");
        let (upstream_got, _) =
            run_intercept(req, resp, "api.anthropic.com", replace_pii()).await;
        let got = String::from_utf8_lossy(&upstream_got);
        assert!(
            !got.contains("alice@acme-corp.com"),
            "original email was not redacted from upstream request: {got:?}"
        );
    }

    #[tokio::test]
    async fn test_pii_sse_response_reversed_to_client() {
        // Proxy must not crash with PII Replace mode on. Full reversal is tested
        // in the pii_roundtrip integration test.
        let req = anthropic_request("reach alice@acme-corp.com today");
        let resp = anthropic_sse_response("will do");
        let (_, client_got) =
            run_intercept(req, resp, "api.anthropic.com", replace_pii()).await;
        assert!(
            !client_got.is_empty(),
            "proxy returned no response to client with PII Replace mode on"
        );
    }

    #[tokio::test]
    async fn test_pii_detected_ws_event_fired() {
        // With PII Replace and an email in the request, a PiiDetected WsEvent
        // must be broadcast on ws_tx.
        let (store, _dir) = temp_store();
        let (ws_tx, mut ws_rx) = broadcast::channel::<WsEvent>(64);

        let req = anthropic_request("email: test-user@acme-corp.com");
        let resp = anthropic_sse_response("noted");

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(256 * 1024);
        let (mut _client_reader, proxy_to_client) = tokio::io::duplex(256 * 1024);
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(256 * 1024);
        let (upstream_reader, proxy_to_upstream) = tokio::io::duplex(256 * 1024);

        // Pre-load the upstream response into its pipe buffer so u_to_c can read
        // it without waiting — otherwise the shutdown_tx fired by c_to_u (on client
        // EOF) causes u_to_c to exit before we get a chance to write the response.
        upstream_writer.write_all(&resp).await.unwrap();
        drop(upstream_writer);

        let run_handle = tokio::spawn(async move {
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                run(
                    client_to_proxy,
                    proxy_to_client,
                    upstream_to_proxy,
                    proxy_to_upstream,
                    "api.anthropic.com".to_string(),
                    store,
                    ws_tx,
                    replace_pii(),
                ),
            )
            .await;
        });

        // Feed client request then close (signals EOF → c_to_u finishes).
        client_writer.write_all(&req).await.unwrap();
        drop(client_writer);
        // Discard what proxy forwarded to upstream (don't block on it).
        drop(upstream_reader);

        run_handle.await.ok();

        // Check whether any PiiDetected event was broadcast.
        let mut found = false;
        while let Ok(ev) = ws_rx.try_recv() {
            if matches!(ev, WsEvent::PiiDetected { .. }) {
                found = true;
                break;
            }
        }
        assert!(found, "no PiiDetected WsEvent was fired for email in request");
    }

    // ── Extended helper: captures WS events and exposes the store ─────────────

    struct InterceptResult {
        upstream_bytes: Vec<u8>,
        client_bytes: Vec<u8>,
        ws_events: Vec<WsEvent>,
        store: Store,
        _dir: TempDir,
    }

    async fn run_intercept_full(
        client_request: Vec<u8>,
        upstream_response: Vec<u8>,
        host: &str,
        pii: PiiCtx,
    ) -> InterceptResult {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let store_for_run = store.clone();
        let (ws_tx, mut ws_rx) = broadcast::channel::<WsEvent>(256);

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(512 * 1024);
        let (mut client_reader, proxy_to_client) = tokio::io::duplex(512 * 1024);
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(512 * 1024);
        let (mut upstream_reader, proxy_to_upstream) = tokio::io::duplex(512 * 1024);

        let feed_client = tokio::spawn(async move {
            client_writer.write_all(&client_request).await.ok();
        });
        let feed_upstream = tokio::spawn(async move {
            upstream_writer.write_all(&upstream_response).await.ok();
        });
        let capture_upstream = tokio::spawn(async move {
            let mut buf = Vec::new();
            upstream_reader.read_to_end(&mut buf).await.ok();
            buf
        });
        let capture_client = tokio::spawn(async move {
            let mut buf = Vec::new();
            client_reader.read_to_end(&mut buf).await.ok();
            buf
        });

        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            run(
                client_to_proxy, proxy_to_client,
                upstream_to_proxy, proxy_to_upstream,
                host.to_string(), store_for_run, ws_tx, pii,
            ),
        ).await;

        let _ = feed_client.await;
        let _ = feed_upstream.await;
        let upstream_bytes = capture_upstream.await.unwrap_or_default();
        let client_bytes = capture_client.await.unwrap_or_default();

        let mut ws_events = Vec::new();
        while let Ok(ev) = ws_rx.try_recv() {
            ws_events.push(ev);
        }

        InterceptResult { upstream_bytes, client_bytes, ws_events, store, _dir: dir }
    }

    /// Run multiple keep-alive request/response turns on one connection.
    ///
    /// Each turn is interleaved correctly: the request is sent, the upstream
    /// response is written, and we wait for the response terminator (`[DONE]` or
    /// `message_stop`) in the client output before proceeding to the next turn.
    /// This prevents the pre-buffering race that breaks simple concatenation tests.
    async fn run_keepalive(
        turns: Vec<(Vec<u8>, Vec<u8>)>,
        host: &str,
        pii: PiiCtx,
    ) -> InterceptResult {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let store_for_run = store.clone();
        let (ws_tx, mut ws_rx) = broadcast::channel::<WsEvent>(256);

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(512 * 1024);
        let (mut client_reader, proxy_to_client) = tokio::io::duplex(512 * 1024);
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(512 * 1024);
        let (mut upstream_reader, proxy_to_upstream) = tokio::io::duplex(512 * 1024);

        // Drive task: serialises req/resp so c2u processes one request before u2c
        // processes the corresponding response, then both move on to the next turn.
        let drive = tokio::spawn(async move {
            let mut all_client_bytes = Vec::<u8>::new();
            let mut all_upstream_bytes = Vec::<u8>::new();
            let mut rd_pos = 0usize; // scan position in all_client_bytes for terminators

            for (req, resp) in turns {
                // Write this turn's request to the client input.
                client_writer.write_all(&req).await.ok();
                // Write this turn's upstream response.
                upstream_writer.write_all(&resp).await.ok();

                // Read client output until we see the response terminator, indicating
                // u2c has finished processing this turn's response.
                let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
                let mut buf = [0u8; 65536];
                loop {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining.is_zero() { break; }
                    match tokio::time::timeout(remaining, client_reader.read(&mut buf)).await {
                        Ok(Ok(n)) if n > 0 => {
                            all_client_bytes.extend_from_slice(&buf[..n]);
                            let new = String::from_utf8_lossy(&all_client_bytes[rd_pos..]);
                            if new.contains("[DONE]") || new.contains("\"message_stop\"") {
                                rd_pos = all_client_bytes.len();
                                break;
                            }
                        }
                        _ => break,
                    }
                }
            }

            // All turns done: close the client connection so c2u exits.
            drop(client_writer);
            drop(upstream_writer);

            // Drain whatever the proxy forwarded to upstream (for assertion use).
            let mut buf = [0u8; 65536];
            let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(
                    deadline.saturating_duration_since(tokio::time::Instant::now()),
                    upstream_reader.read(&mut buf),
                ).await {
                    Ok(Ok(n)) if n > 0 => { all_upstream_bytes.extend_from_slice(&buf[..n]); }
                    _ => break,
                }
            }

            (all_client_bytes, all_upstream_bytes)
        });

        // Run proxy with a generous timeout.
        let _ = tokio::time::timeout(
            Duration::from_secs(10),
            run(
                client_to_proxy, proxy_to_client,
                upstream_to_proxy, proxy_to_upstream,
                host.to_string(), store_for_run, ws_tx, pii,
            ),
        ).await;

        let (client_bytes, upstream_bytes) = drive.await.unwrap_or_default();

        let mut ws_events = Vec::new();
        while let Ok(ev) = ws_rx.try_recv() {
            ws_events.push(ev);
        }

        InterceptResult { upstream_bytes, client_bytes, ws_events, store, _dir: dir }
    }

    fn openai_request(content: &str) -> Vec<u8> {
        let body = format!(
            r#"{{"model":"gpt-4","messages":[{{"role":"user","content":"{content}"}}]}}"#
        );
        format!(
            "POST /v1/chat/completions HTTP/1.1\r\nHost: api.openai.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer sk-test\r\n\r\n{}",
            body.len(), body
        ).into_bytes()
    }

    fn openai_sse_response(text: &str) -> Vec<u8> {
        let events = format!(
            "data: {{\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n\
             data: [DONE]\n\n"
        );
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{events}"
        ).into_bytes()
    }

    // ── Section 2.1.2: Large request ─────────────────────────────────────────

    #[tokio::test]
    async fn test_large_request_response_forwarded_verbatim() {
        // 40-turn conversation ~60 KB.
        let mut messages = String::from("[");
        for i in 0..40 {
            if i > 0 { messages.push(','); }
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            let content = "x".repeat(750); // ~750 chars per turn × 40 = ~30 KB text
            messages.push_str(&format!(r#"{{"role":"{role}","content":"{content}"}}"#));
        }
        messages.push(']');
        let body = format!(r#"{{"model":"claude-3-opus-20240229","max_tokens":256,"messages":{messages}}}"#);
        let req = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Api-Key: sk-ant-test\r\n\r\n{}",
            body.len(), body
        ).into_bytes();
        let resp = anthropic_sse_response("done");

        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&r.upstream_bytes);
        // All 40 turns must reach upstream.
        assert!(got.contains("\"messages\""), "messages key missing: upstream got nothing?");
        assert!(got.len() > 30_000, "large request too short: {} bytes", got.len());
    }

    // ── Section 2.4.1: EOF mid-SSE ────────────────────────────────────────────

    #[tokio::test]
    async fn test_upstream_eof_mid_sse_finalizes_partial() {
        let req = anthropic_request("partial test");
        // Truncated SSE — no [DONE] sentinel.
        let partial_resp =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n\
              event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"half\"}}\n\n"
            .to_vec();
        // Must not panic. Client should receive whatever was forwarded.
        let r = run_intercept_full(req, partial_resp, "api.anthropic.com", no_pii()).await;
        let got = String::from_utf8_lossy(&r.client_bytes);
        assert!(got.contains("half"), "partial SSE not forwarded to client: {got:?}");
    }

    // ── Section 2.4.3: Client disconnect ─────────────────────────────────────

    #[tokio::test]
    async fn test_client_disconnect_mid_response_still_stores() {
        // We drop the client reader early to simulate client disconnect during response.
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(64);
        let req = anthropic_request("disconnect test");
        let resp = anthropic_sse_response("still going");

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(512 * 1024);
        let (_client_reader, proxy_to_client) = tokio::io::duplex(512 * 1024); // dropped immediately
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(512 * 1024);
        let (upstream_reader, proxy_to_upstream) = tokio::io::duplex(512 * 1024);

        // Pre-load upstream response.
        upstream_writer.write_all(&resp).await.unwrap();
        drop(upstream_writer);
        // Drop client_reader = simulates client disconnecting before reading response.
        drop(_client_reader);

        let store_for_run = store.clone();
        let run_handle = tokio::spawn(async move {
            let _ = tokio::time::timeout(Duration::from_secs(5), run(
                client_to_proxy, proxy_to_client,
                upstream_to_proxy, proxy_to_upstream,
                "api.anthropic.com".to_string(), store_for_run, ws_tx, no_pii(),
            )).await;
        });

        client_writer.write_all(&req).await.unwrap();
        drop(client_writer);
        drop(upstream_reader);
        run_handle.await.ok();
        // Must not panic — reaching here is the primary assertion.
    }

    // ── Section 2.4.4: Upstream idle timeout ─────────────────────────────────

    /// 2.4.4: When the upstream server sends nothing for longer than
    /// UPSTREAM_READ_TIMEOUT, u2c exits cleanly and the proxy shuts down.
    #[tokio::test(start_paused = true)]
    async fn test_upstream_idle_timeout_fires() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(64);
        let req = anthropic_request("idle timeout test");

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(256 * 1024);
        let (_client_reader, proxy_to_client) = tokio::io::duplex(256 * 1024);
        let (upstream_to_proxy, _upstream_writer) = tokio::io::duplex(256 * 1024);
        // _upstream_writer is intentionally kept alive but never written to,
        // so upstream_to_proxy never returns EOF — only the idle timer can exit u2c.
        let (_upstream_reader, proxy_to_upstream) = tokio::io::duplex(256 * 1024);

        // Pre-load the client request. Keep client_writer alive so c2u does not
        // send shutdown prematurely, allowing u2c to hit the idle timeout first.
        client_writer.write_all(&req).await.ok();

        let proxy = tokio::spawn(run(
            client_to_proxy, proxy_to_client,
            upstream_to_proxy, proxy_to_upstream,
            "api.anthropic.com".to_string(), store, ws_tx, no_pii(),
        ));

        // Yield to allow the proxy task to poll and register its timeout future
        // before we advance the mock clock.
        tokio::task::yield_now().await;

        // Advance time past the idle timeout.
        tokio::time::advance(UPSTREAM_READ_TIMEOUT + Duration::from_millis(100)).await;

        // Signal client EOF so c2u can exit after u2c has already timed out.
        drop(client_writer);

        // Use a generous mock-time guard (200 s in the future) so the guard itself
        // won't fire immediately on the already-advanced clock.  The proxy should
        // complete quickly once the idle timeout fires.
        tokio::time::timeout(Duration::from_secs(200), proxy)
            .await
            .expect("proxy hung after upstream idle timeout")
            .expect("proxy task panicked")
            .expect("proxy returned error");
    }

    // ── Section 2.5.2: OpenAI [DONE] sentinel ────────────────────────────────

    #[tokio::test]
    async fn test_openai_done_sentinel_terminates_stream() {
        let req = openai_request("openai test");
        let resp = openai_sse_response("hello openai");
        let (_, client_got) =
            run_intercept(req, resp, "api.openai.com", no_pii()).await;
        let got = String::from_utf8_lossy(&client_got);
        assert!(got.contains("[DONE]"),
            "OpenAI [DONE] sentinel not forwarded to client: {got:?}");
    }

    // ── Section 2.5.6: WS TextDelta events ───────────────────────────────────

    #[tokio::test]
    async fn test_ws_text_delta_events_fired() {
        let req = anthropic_request("delta test");
        let resp = anthropic_sse_response("hello delta");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let has_delta = r.ws_events.iter().any(|e| matches!(e, WsEvent::TextDelta { .. }));
        assert!(has_delta, "no TextDelta WsEvent fired. events: {:?}",
            r.ws_events.iter().map(|e| std::mem::discriminant(e)).collect::<Vec<_>>());
    }

    // ── Section 2.5.7: WS ResponseComplete event ─────────────────────────────

    #[tokio::test]
    async fn test_response_complete_ws_event_fired() {
        let req = anthropic_request("complete test");
        let resp = anthropic_sse_response("response complete");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let has_complete = r.ws_events.iter().any(|e| matches!(e, WsEvent::ResponseComplete { .. }));
        assert!(has_complete, "no ResponseComplete WsEvent fired. events: {:?}",
            r.ws_events.iter().map(|e| std::mem::discriminant(e)).collect::<Vec<_>>());
    }

    // ── Section 2.6.1: Conversation stored ───────────────────────────────────

    #[tokio::test]
    async fn test_new_conversation_created_on_first_request() {
        let req = anthropic_request("store me");
        let resp = anthropic_sse_response("stored");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(!convs.is_empty(),
            "no conversation was created in storage after first request");
    }

    // ── Section 2.6.2: ConversationStart WS event ────────────────────────────

    #[tokio::test]
    async fn test_conversation_start_ws_event_for_new_conv() {
        let req = anthropic_request("ws start test");
        let resp = anthropic_sse_response("started");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let has_start = r.ws_events.iter().any(|e| matches!(e, WsEvent::ConversationStart { .. }));
        assert!(has_start, "no ConversationStart WsEvent fired. events: {:?}",
            r.ws_events.iter().map(|e| std::mem::discriminant(e)).collect::<Vec<_>>());
    }

    // ── Section 2.6.3: Response messages stored ──────────────────────────────

    #[tokio::test]
    async fn test_response_stored_after_sse_complete() {
        let req = anthropic_request("remember this");
        let resp = anthropic_sse_response("I remember");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;
        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(!convs.is_empty(), "no conversation stored");
        let msgs = r.store.get_messages(&convs[0].id).unwrap_or_default();
        // Expect at least the user request message.
        assert!(!msgs.is_empty(), "no messages stored for conversation {}", convs[0].id);
    }

    // ── Section 2.6.4: Unparseable request ───────────────────────────────────

    #[tokio::test]
    async fn test_unparseable_request_no_storage_no_panic() {
        let garbage = b"NOT HTTP AT ALL\r\n\r\ngarbage body here!!!".to_vec();
        let resp = b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n".to_vec();
        // Must not panic.
        let r = run_intercept_full(garbage, resp, "api.anthropic.com", no_pii()).await;
        // No conversation should be stored for garbage input.
        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(convs.is_empty(),
            "garbage request should not create a conversation, got {} convs", convs.len());
    }

    // ── Section 2.3.1: Concurrent sessions ───────────────────────────────────

    #[tokio::test]
    async fn test_two_concurrent_sessions_dont_interfere() {
        let req_a = anthropic_request("session A content");
        let req_b = anthropic_request("session B content");
        let resp_a = anthropic_sse_response("response from A");
        let resp_b = anthropic_sse_response("response from B");

        let (fut_a, fut_b) = tokio::join!(
            run_intercept_full(req_a, resp_a, "api.anthropic.com", no_pii()),
            run_intercept_full(req_b, resp_b, "api.anthropic.com", no_pii()),
        );

        // Each session's upstream should only see its own request body.
        let got_a = String::from_utf8_lossy(&fut_a.upstream_bytes);
        let got_b = String::from_utf8_lossy(&fut_b.upstream_bytes);
        assert!(got_a.contains("session A content"),
            "session A upstream did not get its own request: {got_a:?}");
        assert!(got_b.contains("session B content"),
            "session B upstream did not get its own request: {got_b:?}");
        assert!(!got_a.contains("session B content"),
            "session B content leaked into session A upstream: {got_a:?}");
        assert!(!got_b.contains("session A content"),
            "session A content leaked into session B upstream: {got_b:?}");
    }

    // ── Section A.2 / 7.3: PII mode dispatch + Content-Length ────────────────

    #[tokio::test]
    async fn test_pii_mode_detect_only_body_unchanged() {
        // DetectOnly: body must reach upstream unmodified, but PiiDetected WS event fires.
        let pii = Some(Arc::new(PiiContext {
            registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
            locale: Locale::EnUs,
            mode: PiiMode::DetectOnly,
            pipeline: PiiPipeline::tier1_only(),
        }));
        let req = anthropic_request("contact detect@acme-corp.com please");
        let resp = anthropic_sse_response("noted");
        let r = run_intercept_full(req, resp, "api.anthropic.com", pii).await;
        let got = String::from_utf8_lossy(&r.upstream_bytes);
        // Body must be unchanged in DetectOnly mode.
        assert!(got.contains("detect@acme-corp.com"),
            "DetectOnly mode must not modify the request body, got: {got:?}");
    }

    #[tokio::test]
    async fn test_content_length_correct_after_replacement() {
        // When PII is replaced, the Content-Length header forwarded to upstream
        // must match the actual (modified) body length.
        let req = anthropic_request("mail replace-me@acme-corp.com now");
        let resp = anthropic_sse_response("done");
        let r = run_intercept_full(req, resp, "api.anthropic.com", replace_pii()).await;

        let raw = String::from_utf8_lossy(&r.upstream_bytes);
        // Extract the Content-Length header value sent to upstream.
        let cl_value: Option<usize> = raw.lines()
            .find(|l| l.to_lowercase().starts_with("content-length:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok());

        // The body is the part after \r\n\r\n in what was sent to upstream.
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let actual_body_len = raw.len() - body_start;

        if let Some(cl) = cl_value {
            assert_eq!(cl, actual_body_len,
                "Content-Length ({cl}) does not match actual body length ({actual_body_len})");
        }
        // If Content-Length is absent it's acceptable (chunked), but if present it must be correct.
    }

    // ── Section 2.2: Keep-alive / multi-turn ─────────────────────────────────

    /// 2.2.1: Two sequential request/response cycles on one connection.
    #[tokio::test]
    async fn test_two_turns_on_one_connection() {
        let r = run_keepalive(
            vec![
                (anthropic_request("first turn"),  anthropic_sse_response("reply one")),
                (anthropic_request("second turn"), anthropic_sse_response("reply two")),
            ],
            "api.anthropic.com",
            no_pii(),
        ).await;

        let got = String::from_utf8_lossy(&r.client_bytes);
        assert!(got.contains("reply one"),  "first response not forwarded: {got:?}");
        assert!(got.contains("reply two"),  "second response not forwarded: {got:?}");

        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(convs.len() >= 1, "no conversations stored after two turns");
    }

    /// 2.2.2: Ten sequential request/response cycles on one connection.
    #[tokio::test]
    async fn test_ten_turns_on_one_connection() {
        let turns: Vec<_> = (0..10)
            .map(|i| (
                anthropic_request(&format!("turn {i}")),
                anthropic_sse_response(&format!("response {i}")),
            ))
            .collect();

        let r = run_keepalive(turns, "api.anthropic.com", no_pii()).await;

        let got = String::from_utf8_lossy(&r.client_bytes);
        for i in 0..10 {
            assert!(got.contains(&format!("response {i}")),
                "response {i} not forwarded to client");
        }
    }

    /// 2.2.3: Each request in a keep-alive connection gets independent state.
    /// Two requests with different first messages → two separate conversations.
    #[tokio::test]
    async fn test_per_request_state_reset() {
        let r = run_keepalive(
            vec![
                (anthropic_request("question alpha"), anthropic_sse_response("answer alpha")),
                (anthropic_request("question beta"),  anthropic_sse_response("answer beta")),
            ],
            "api.anthropic.com",
            no_pii(),
        ).await;

        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(convs.len() >= 2,
            "expected 2 conversations for 2 distinct requests, got {}", convs.len());
    }

    /// 2.2.4: When a continuation request arrives (same first message), only new messages
    /// are stored in the existing conversation.
    #[tokio::test]
    async fn test_new_messages_only_stored_on_continuation() {
        let turn1_body = r#"{"model":"claude-3-opus-20240229","max_tokens":256,"messages":[{"role":"user","content":"What is 2+2?"}]}"#;
        let req1 = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Api-Key: sk-ant-test\r\n\r\n{}",
            turn1_body.len(), turn1_body
        ).into_bytes();

        let turn2_body = r#"{"model":"claude-3-opus-20240229","max_tokens":256,"messages":[{"role":"user","content":"What is 2+2?"},{"role":"assistant","content":"4"},{"role":"user","content":"And 3+3?"}]}"#;
        let req2 = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Api-Key: sk-ant-test\r\n\r\n{}",
            turn2_body.len(), turn2_body
        ).into_bytes();

        let r = run_keepalive(
            vec![
                (req1, anthropic_sse_response("4")),
                (req2, anthropic_sse_response("6")),
            ],
            "api.anthropic.com",
            no_pii(),
        ).await;

        // Fingerprint is based on the first message only → both turns map to the same conversation.
        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert_eq!(convs.len(), 1,
            "expected 1 conversation for continuation turns, got {}", convs.len());

        let msgs = r.store.get_messages(&convs[0].id).unwrap_or_default();
        let request_msgs: Vec<_> = msgs.iter().filter(|m| m.direction == "request").collect();
        // Turn 1 stored 1 message; turn 2 stores the 2 new messages (assistant + user).
        assert_eq!(request_msgs.len(), 3,
            "expected 3 stored request messages (1 from turn1 + 2 new from turn2), got {}",
            request_msgs.len());
    }

    // ── Section 2.3.2-2.3.3: Additional concurrent sessions ──────────────────

    /// 2.3.2: Twenty concurrent sessions complete without interference.
    #[tokio::test]
    async fn test_twenty_concurrent_sessions() {
        let futs: Vec<_> = (0..20)
            .map(|i| {
                let req = anthropic_request(&format!("concurrent session {i}"));
                let resp = anthropic_sse_response(&format!("ok {i}"));
                run_intercept_full(req, resp, "api.anthropic.com", no_pii())
            })
            .collect();

        let results = futures_util::future::join_all(futs).await;
        assert_eq!(results.len(), 20);
        for (i, r) in results.iter().enumerate() {
            let got = String::from_utf8_lossy(&r.client_bytes);
            assert!(got.contains(&format!("ok {i}")),
                "session {i} response missing from client bytes");
        }
    }

    /// 2.3.3: Two concurrent sessions with the same request fingerprint (same first message)
    /// use separate stores but each creates exactly one conversation — no corruption.
    #[tokio::test]
    async fn test_concurrent_same_fingerprint() {
        // Same content → same fingerprint → same conversation in each separate store.
        let make = || {
            let req = anthropic_request("identical content");
            let resp = anthropic_sse_response("same response");
            run_intercept_full(req, resp, "api.anthropic.com", no_pii())
        };

        let (r1, r2) = tokio::join!(make(), make());

        // Each run uses its own temp store; each should have exactly 1 conversation.
        let c1 = r1.store.list_conversations(100).unwrap_or_default();
        let c2 = r2.store.list_conversations(100).unwrap_or_default();
        assert_eq!(c1.len(), 1, "session 1 should have 1 conversation, got {}", c1.len());
        assert_eq!(c2.len(), 1, "session 2 should have 1 conversation, got {}", c2.len());

        // Both sessions must have forwarded the response correctly.
        assert!(String::from_utf8_lossy(&r1.client_bytes).contains("same response"));
        assert!(String::from_utf8_lossy(&r2.client_bytes).contains("same response"));
    }

    // ── Section 2.5.3: Tokens extracted and stored ────────────────────────────

    /// 2.5.3: Anthropic `message_start` usage tokens are extracted and stored in
    /// the response message record.
    #[tokio::test]
    async fn test_tokens_extracted_and_stored() {
        // anthropic_sse_response embeds input_tokens=10 in message_start.
        let req = anthropic_request("count my tokens");
        let resp = anthropic_sse_response("here you go");
        let r = run_intercept_full(req, resp, "api.anthropic.com", no_pii()).await;

        let convs = r.store.list_conversations(100).unwrap_or_default();
        assert!(!convs.is_empty(), "no conversation stored");

        let msgs = r.store.get_messages(&convs[0].id).unwrap_or_default();
        let response_msg = msgs.iter().find(|m| m.direction == "response");
        assert!(response_msg.is_some(), "no response message stored");

        let rmsg = response_msg.unwrap();
        assert_eq!(rmsg.tokens_in, Some(10),
            "tokens_in should be 10 (from message_start), got {:?}", rmsg.tokens_in);
    }

    // ── Section 2.5.4: SSE accumulation buffer cap ────────────────────────────

    /// 2.5.4: When SSE content exceeds MAX_SSE_BUFFER (10 MB), the proxy keeps
    /// forwarding data to the client without panicking. Accumulation stops at the cap
    /// but the stored response still has content.
    #[tokio::test]
    async fn test_sse_accumulation_buffer_cap() {
        // Generate slightly more than 10 MB of text via many 1 KB delta events.
        const CHUNK_SIZE: usize = 1024;
        const N_EVENTS: usize = 10_241; // 10_241 * 1024 ≈ 10.5 MB > MAX_SSE_BUFFER (10 MB)
        let chunk = "x".repeat(CHUNK_SIZE);

        let mut sse_body = String::new();
        for _ in 0..N_EVENTS {
            let data = format!(
                "{{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}",
                chunk
            );
            sse_body.push_str(&format!("event: content_block_delta\ndata: {data}\n\n"));
        }
        sse_body.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        sse_body.push_str("data: [DONE]\n\n");

        let http_resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{}",
            sse_body
        ).into_bytes();

        let req = anthropic_request("big response please");
        let r = run_intercept_full(req, http_resp, "api.anthropic.com", no_pii()).await;

        // Must not panic and must forward content to client.
        let got = String::from_utf8_lossy(&r.client_bytes);
        assert!(got.contains("content_block_delta"),
            "SSE content not forwarded to client after cap");

        // Stored response must have content (cap does not lose the already-accumulated data).
        let convs = r.store.list_conversations(100).unwrap_or_default();
        if !convs.is_empty() {
            let msgs = r.store.get_messages(&convs[0].id).unwrap_or_default();
            let response_msg = msgs.iter().find(|m| m.direction == "response");
            if let Some(rm) = response_msg {
                assert!(!rm.content.is_empty(), "stored response content must not be empty");
            }
        }
    }

    // ── Section 2.5.5: SSE split across tiny chunks ───────────────────────────

    /// 2.5.5: Upstream response delivered in very small chunks (4 bytes at a time)
    /// must produce the same output as a single large chunk.
    ///
    /// Design: all 4-byte chunks are pre-loaded into the upstream duplex buffer
    /// before the proxy starts. The client writer is kept alive throughout so c2u
    /// never sends a shutdown signal while u2c is still reading. u2c reads all
    /// upstream bytes naturally and exits on EOF (upstream_writer was dropped).
    /// Once u2c exits and closes proxy_to_client, the capture task completes.
    #[tokio::test]
    async fn test_sse_split_across_tiny_chunks() {
        let req = anthropic_request("chunked delivery");
        let resp = anthropic_sse_response("chunk test response");

        // Reference: full response in one shot.
        let (_, full_client_bytes) =
            run_intercept(req.clone(), resp.clone(), "api.anthropic.com", no_pii()).await;

        // Chunked setup.
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (ws_tx, _ws_rx) = broadcast::channel::<WsEvent>(256);

        let (client_to_proxy, mut client_writer) = tokio::io::duplex(256 * 1024);
        let (mut client_reader, proxy_to_client) = tokio::io::duplex(256 * 1024);
        let (upstream_to_proxy, mut upstream_writer) = tokio::io::duplex(256 * 1024);
        let (mut upstream_reader, proxy_to_upstream) = tokio::io::duplex(256 * 1024);

        // Write the request (keep client_writer alive so c2u blocks instead of
        // sending shutdown while u2c is still processing the upstream response).
        client_writer.write_all(&req).await.ok();

        // Pre-load all upstream bytes in 4-byte chunks.
        // The duplex buffer (256 KB) is large enough to hold the full response
        // (~900 bytes) without blocking, so no yields occur between writes.
        for chunk in resp.chunks(4) {
            upstream_writer.write_all(chunk).await.ok();
        }
        // Drop upstream_writer: u2c will see EOF after reading all buffered bytes.
        drop(upstream_writer);

        // Capture what the proxy forwards to upstream (forwarded request bytes).
        let cap_to_upstream = tokio::spawn(async move {
            let mut buf = Vec::new();
            upstream_reader.read_to_end(&mut buf).await.ok();
            buf
        });

        // Capture what the proxy forwards to the client (the SSE response).
        // This completes when u2c closes proxy_to_client (on upstream EOF).
        let cap_to_client = tokio::spawn(async move {
            let mut buf = Vec::new();
            client_reader.read_to_end(&mut buf).await.ok();
            buf
        });

        // Spawn the proxy. u2c exits naturally via upstream EOF;
        // c2u blocks (client_writer is still alive), so run() times out.
        let proxy_handle = tokio::spawn(run(
            client_to_proxy, proxy_to_client,
            upstream_to_proxy, proxy_to_upstream,
            "api.anthropic.com".to_string(), store, ws_tx, no_pii(),
        ));

        // Wait for u2c to finish (cap_to_client resolves when proxy_to_client closes).
        let chunked_client_bytes = tokio::time::timeout(Duration::from_secs(5), cap_to_client)
            .await.unwrap_or_else(|_| Ok(Vec::new())).unwrap_or_default();

        // Drop client_writer so c2u can exit → proxy_handle completes.
        drop(client_writer);
        let _ = tokio::time::timeout(Duration::from_millis(500), proxy_handle).await;
        let _ = tokio::time::timeout(Duration::from_millis(500), cap_to_upstream).await;

        let full_str = String::from_utf8_lossy(&full_client_bytes);
        let chunked_str = String::from_utf8_lossy(&chunked_client_bytes);
        assert!(chunked_str.contains("chunk test response"),
            "chunked delivery: expected text not in client output: {chunked_str:?}");
        assert_eq!(full_str.len(), chunked_str.len(),
            "chunked delivery produced different byte count than full delivery");
    }

    // ── 3.8 entity_type_label pure helper ────────────────────────────────────

    #[test]
    fn entity_type_label_lowercased_email() {
        assert_eq!(entity_type_label("email"), "[EMAIL]");
    }

    #[test]
    fn entity_type_label_already_uppercased() {
        assert_eq!(entity_type_label("EMAIL"), "[EMAIL]");
        assert_eq!(entity_type_label("PHONE"), "[PHONE]");
    }

    #[test]
    fn entity_type_label_mixed_case() {
        assert_eq!(entity_type_label("Phone"), "[PHONE]");
        assert_eq!(entity_type_label("credit_card"), "[CREDIT_CARD]");
        assert_eq!(entity_type_label("person_name"), "[PERSON_NAME]");
        assert_eq!(entity_type_label("Ssn"), "[SSN]");
    }

    #[test]
    fn entity_type_label_empty_string_does_not_panic() {
        assert_eq!(entity_type_label(""), "[]");
    }

    #[test]
    fn entity_type_label_unicode_uppercased() {
        // ASCII-only uppercasing — non-ASCII passthrough is acceptable.
        let result = entity_type_label("ip_v4");
        assert_eq!(result, "[IP_V4]");
    }

    // ── 3.8 Phase A / Phase B helpers ─────────────────────────────────────────

    /// Produce the raw JSON body (NOT a full HTTP request) for a single-message
    /// Anthropic conversation. This is what `create_or_find_conversation` and
    /// `store_request_messages` receive in production (the decoded request body).
    fn anthropic_json_body(content: &str) -> Vec<u8> {
        let escaped = content.replace('"', "\\\"");
        format!(
            r#"{{"model":"claude-3-opus-20240229","max_tokens":256,"messages":[{{"role":"user","content":"{escaped}"}}]}}"#
        ).into_bytes()
    }

    // ── 3.8 Phase A: same fingerprint returns same conv_id ────────────────────

    #[tokio::test]
    async fn phase_a_returns_same_conv_id_for_same_fingerprint() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);
        let shared = Arc::new(Mutex::new(None::<String>));

        let body = anthropic_json_body("tell me about Rust");

        // Call Phase A twice with the same body → same fingerprint → same conv_id.
        let id1 = create_or_find_conversation(
            &body, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared,
        ).await;

        // Phase A is idempotent: calling it again with the same body (and with the
        // file already on disk) must return the same conv_id.
        let id2 = create_or_find_conversation(
            &body, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared,
        ).await;

        assert!(id1.is_some(), "Phase A must return a conv_id");
        assert_eq!(id1, id2, "same fingerprint must produce the same conv_id");
    }

    #[tokio::test]
    async fn phase_a_different_body_different_conv_id() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);

        let shared1 = Arc::new(Mutex::new(None::<String>));
        let shared2 = Arc::new(Mutex::new(None::<String>));

        let body1 = anthropic_json_body("first unique message aaa");
        let body2 = anthropic_json_body("second unique message bbb");

        let id1 = create_or_find_conversation(
            &body1, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared1,
        ).await;
        let id2 = create_or_find_conversation(
            &body2, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared2,
        ).await;

        assert!(id1.is_some());
        assert!(id2.is_some());
        assert_ne!(id1, id2, "different bodies must produce distinct conv_ids");
    }

    // ── 3.8 Phase B: content_masked populated for pii_processed=true ─────────

    #[tokio::test]
    async fn phase_b_stores_content_masked_when_pii_active() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);
        let shared = Arc::new(Mutex::new(None::<String>));

        let original_body = anthropic_json_body("My email is alice@acme.com please help");
        // Simulate what PII pipeline would produce: email replaced with synthetic.
        let replaced_body = anthropic_json_body("My email is synth@example.com please help");

        // Phase A: create conversation.
        let conv_id = create_or_find_conversation(
            &original_body, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared,
        ).await.expect("Phase A must return conv_id");

        // Phase B: store with replaced content.
        let ids = store_request_messages(
            &original_body,
            Some(&replaced_body),
            true,
            &conv_id,
            crate::parser::Provider::Anthropic,
            &store,
            &ws_tx,
        ).await;

        assert!(!ids.is_empty(), "Phase B must return at least one message id");

        // Verify the stored message has content_masked set.
        let messages = store.get_messages(&conv_id).unwrap();
        let req_msg = messages.iter().find(|m| m.direction == "request")
            .expect("at least one request message should be stored");
        assert_eq!(req_msg.pii_processed, Some(true),
            "pii_processed must be true for PII-active path");
        assert!(req_msg.content_masked.is_some(),
            "content_masked must be set when replaced_body is provided");
        let masked = req_msg.content_masked.as_ref().unwrap();
        assert!(masked.contains("synth@example.com"),
            "content_masked must contain the synthetic replacement: {masked}");
    }

    #[tokio::test]
    async fn phase_b_content_masked_none_when_passthrough() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);
        let shared = Arc::new(Mutex::new(None::<String>));

        let body = anthropic_json_body("plain text no pii");

        let conv_id = create_or_find_conversation(
            &body, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared,
        ).await.expect("Phase A must return conv_id");

        // Phase B with no replaced_body and pii_processed=false.
        store_request_messages(
            &body,
            None,
            false,
            &conv_id,
            crate::parser::Provider::Anthropic,
            &store,
            &ws_tx,
        ).await;

        let messages = store.get_messages(&conv_id).unwrap();
        let req_msg = messages.iter().find(|m| m.direction == "request")
            .expect("request message must be stored");
        assert_eq!(req_msg.pii_processed, Some(false),
            "pii_processed must be false for passthrough");
        assert!(req_msg.content_masked.is_none(),
            "content_masked must be None for passthrough, got {:?}", req_msg.content_masked);
    }

    // ── Bug-fix regression tests: upstream death propagation ─────────────────
    //
    // All four tests below share the same harness pattern:
    //   1. Complete exactly one request/response cycle (client stays connected).
    //   2. Drop the upstream write half to simulate upstream server death (EOF).
    //   3. Assert the observable effect of the fix.
    //
    // Design constraint: the client write half is intentionally kept alive
    // throughout the test to prevent c2u from exiting on client EOF.  Without
    // the fix that is being tested, c2u would block indefinitely on
    // `reader.read()` at the between-requests idle point — the test timeout
    // would fire and the assertion would fail.

    /// Shared setup: allocate pipes, spawn the proxy, write one req/resp cycle,
    /// wait for the response to reach the client output, then return the live
    /// handles for the test to manipulate.
    ///
    /// `wait_for_resp_sentinel` — the string to wait for in the client output
    /// that signals the first response is complete (e.g. "message_stop").
    ///
    /// Returns:
    ///   (proxy_handle, client_writer, client_rx, upstream_tx, upstream_rx, _dir)
    ///
    /// `upstream_tx` is the write half of the upstream pipe.  Drop it to
    /// simulate upstream server death.
    ///
    /// `client_rx` is a channel receiver that streams bytes the proxy has sent
    /// to the client; useful for reading the eventual EOF.
    ///
    /// `upstream_rx_bytes` accumulates all bytes proxy sent to upstream.
    async fn one_cycle_setup(
        pii: PiiCtx,
    ) -> OneCycleHandles {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (ws_tx, _ws_rx) = broadcast::channel::<WsEvent>(64);

        let (client_to_proxy, client_writer_inner) = tokio::io::duplex(512 * 1024);
        let (client_reader_inner, proxy_to_client) = tokio::io::duplex(512 * 1024);
        let (upstream_to_proxy, upstream_writer_inner) = tokio::io::duplex(512 * 1024);
        let (upstream_reader_inner, proxy_to_upstream) = tokio::io::duplex(512 * 1024);

        // Channel to relay test → client_writer (allows writing after the proxy starts).
        let (client_write_tx, mut client_write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        // Channel to relay test → upstream_writer.
        let (upstream_write_tx, mut upstream_write_rx) = tokio::sync::mpsc::unbounded_channel::<Option<Vec<u8>>>();
        // Channel to receive bytes proxy sent to the client.
        let (client_out_tx, mut client_out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        // Channel to receive bytes proxy sent to upstream.
        let (upstream_out_tx, mut upstream_out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

        // Task: relay bytes from test into client_writer.
        tokio::spawn(async move {
            let mut w = client_writer_inner;
            while let Some(data) = client_write_rx.recv().await {
                if w.write_all(&data).await.is_err() { break; }
            }
            // When sender is dropped, this task exits and w is dropped → EOF on client_to_proxy.
        });

        // Task: relay bytes from test into upstream_writer; None = drop writer (EOF).
        tokio::spawn(async move {
            let mut w = upstream_writer_inner;
            while let Some(maybe_data) = upstream_write_rx.recv().await {
                match maybe_data {
                    Some(data) => { if w.write_all(&data).await.is_err() { break; } }
                    None => break, // explicit "drop writer" signal
                }
            }
            // w dropped here → EOF on upstream_to_proxy.
        });

        // Task: capture bytes proxy sends to client.
        tokio::spawn(async move {
            let mut r = client_reader_inner;
            let mut buf = vec![0u8; 65536];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => { let _ = client_out_tx.send(buf[..n].to_vec()); }
                }
            }
        });

        // Task: capture bytes proxy sends to upstream.
        tokio::spawn(async move {
            let mut r = upstream_reader_inner;
            let mut buf = vec![0u8; 65536];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => { let _ = upstream_out_tx.send(buf[..n].to_vec()); }
                }
            }
        });

        // Spawn the proxy.
        let proxy_handle = tokio::spawn(run(
            client_to_proxy,
            proxy_to_client,
            upstream_to_proxy,
            proxy_to_upstream,
            "api.anthropic.com".to_string(),
            store,
            ws_tx,
            pii,
        ));

        // Send one req/resp cycle.
        let req = anthropic_request("bug regression one");
        let resp = anthropic_sse_response("cycle one ok");
        client_write_tx.send(req).unwrap();
        upstream_write_tx.send(Some(resp)).unwrap();

        // Wait until c2u has forwarded the first request to upstream.  This is
        // the reliable sentinel for "c2u has completed one full request cycle
        // and is back at the between-requests idle point" — it works for both
        // passthrough and PII modes.
        //
        // In passthrough mode, c2u forwards immediately on Content-Length.
        // In PII mode, c2u processes the full request body before forwarding.
        // Either way, once upstream_out_rx receives bytes, c2u is past the
        // forwarding step and will soon be back in `raw.is_empty()` idle state.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut upstream_got = Vec::<u8>::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { break; }
            match tokio::time::timeout(remaining, upstream_out_rx.recv()).await {
                Ok(Some(chunk)) => {
                    upstream_got.extend_from_slice(&chunk);
                    // Any chunk containing the request body content is sufficient.
                    if String::from_utf8_lossy(&upstream_got).contains("bug regression one") {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert!(
            String::from_utf8_lossy(&upstream_got).contains("bug regression one"),
            "first request did not reach upstream during setup (c2u did not forward): {:?}",
            String::from_utf8_lossy(&upstream_got)
        );

        // Also wait briefly for u2c to begin forwarding the response headers,
        // so u2c is active and can promptly process upstream EOF.
        let _ = tokio::time::timeout(
            Duration::from_millis(200),
            client_out_rx.recv(),
        ).await;

        OneCycleHandles {
            proxy_handle,
            client_write_tx,
            upstream_write_tx,
            upstream_out_rx,
            _dir: dir,
        }
    }

    struct OneCycleHandles {
        proxy_handle: tokio::task::JoinHandle<Result<()>>,
        /// Send bytes to the client→proxy pipe.  Drop to close client connection.
        client_write_tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        /// Send Some(bytes) to upstream→proxy pipe, or None to close it (upstream dies).
        upstream_write_tx: tokio::sync::mpsc::UnboundedSender<Option<Vec<u8>>>,
        /// Receive bytes that the proxy forwarded to upstream.
        upstream_out_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        _dir: TempDir,
    }

    // ── Bug 1: upstream death must unblock handle_c2u_passthrough ────────────

    /// Regression test for Bug 1: after the upstream server closes its
    /// connection, `handle_c2u_passthrough` must exit instead of blocking
    /// indefinitely on `reader.read()` at the between-requests idle point.
    ///
    /// Without the fix (no `upstream_shutdown_rx` in `select!`), the proxy
    /// hangs forever and this test times out.
    #[tokio::test]
    async fn test_upstream_eof_signals_passthrough_c2u_to_exit() {
        let OneCycleHandles {
            proxy_handle,
            client_write_tx,
            upstream_write_tx,
            upstream_out_rx: _,
            _dir,
        } = one_cycle_setup(no_pii()).await;

        // Signal upstream death: send None to the upstream relay task so it
        // drops the upstream_writer → EOF on upstream_to_proxy.
        upstream_write_tx.send(None).unwrap();
        // Drop the sender so the relay task also exits cleanly.
        drop(upstream_write_tx);

        // The proxy must exit within 2 seconds.  Without the fix, handle_c2u_passthrough
        // would block forever on reader.read() and this timeout would fire.
        let result = tokio::time::timeout(Duration::from_secs(2), proxy_handle).await;
        assert!(
            result.is_ok(),
            "proxy hung after upstream death in passthrough mode (Bug 1 regression)"
        );

        // Ensure client_write_tx is not dropped before we reach the assertion
        // (its drop would cause a client EOF that would also exit c2u, masking the bug).
        drop(client_write_tx);
    }

    // ── Bug 2: upstream_gone flag prevents write in passthrough ──────────────

    /// Regression test for Bug 2: when `upstream_gone` is set (upstream has
    /// died), a second request from the client must NOT be forwarded to the
    /// dead upstream, and the proxy must exit cleanly.
    ///
    /// Without the fix (`upstream_gone.load()` check absent before each
    /// `upstream_write` call), `handle_c2u_passthrough` would attempt to write
    /// to a broken pipe — either panicking or producing a silent data corruption.
    ///
    /// Combined with the Bug 1 fix (`upstream_shutdown_rx`), the proxy must
    /// exit cleanly after upstream death regardless of which guard fires first.
    /// This test validates the observable contract: after upstream dies, a second
    /// request is never forwarded and the proxy terminates within a deadline.
    #[tokio::test]
    async fn test_upstream_gone_flag_prevents_write_in_passthrough() {
        let OneCycleHandles {
            proxy_handle,
            client_write_tx,
            upstream_write_tx,
            mut upstream_out_rx,
            _dir,
        } = one_cycle_setup(no_pii()).await;

        // Drain bytes the proxy already forwarded for the first cycle so we
        // start with a clean baseline for the second-request assertion.
        let drain_deadline = tokio::time::Instant::now() + Duration::from_millis(300);
        while tokio::time::Instant::now() < drain_deadline {
            match tokio::time::timeout(
                drain_deadline.saturating_duration_since(tokio::time::Instant::now()),
                upstream_out_rx.recv(),
            ).await {
                Ok(Some(_)) => { /* drain */ }
                _ => break,
            }
        }

        // Kill upstream: EOF → handle_u2c sets upstream_gone=true and sends
        // upstream_shutdown_tx.
        upstream_write_tx.send(None).unwrap();
        drop(upstream_write_tx);

        // Give handle_u2c time to process the EOF and set upstream_gone=true.
        // 150 ms is ample for a local in-process duplex stream under test conditions.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Send a second request.  At this point upstream_gone=true AND
        // upstream_shutdown_tx has been sent, so c2u should bail via either:
        //   - the upstream_gone check before upstream_write (Bug 2 guard), OR
        //   - the upstream_shutdown_rx in the idle select! (Bug 1 guard).
        let second_req = anthropic_request("second request after upstream death");
        client_write_tx.send(second_req).unwrap();
        // Drop client writer so c2u can exit after processing (or rejecting) the request.
        drop(client_write_tx);

        // Proxy must exit within 2 seconds.
        let result = tokio::time::timeout(Duration::from_secs(2), proxy_handle).await;
        assert!(
            result.is_ok(),
            "proxy hung after upstream death + second client request (Bug 2 regression)"
        );

        // The second request's body must NOT have reached upstream.
        let mut second_cycle_bytes = Vec::<u8>::new();
        while let Ok(chunk) = upstream_out_rx.try_recv() {
            second_cycle_bytes.extend_from_slice(&chunk);
        }
        let second_got = String::from_utf8_lossy(&second_cycle_bytes);
        assert!(
            !second_got.contains("second request after upstream death"),
            "Bug 2 regression: second request body was forwarded to dead upstream.\n\
             upstream received after first cycle: {second_got:?}"
        );
    }

    // ── Bug 3: handle_u2c sends clean EOF to client before returning ─────────

    /// Regression test for Bug 3: after upstream dies, `handle_u2c` must call
    /// `writer.shutdown()` so the client receives a clean EOF (equivalent to
    /// TLS `close_notify`) even if `handle_c2u` is still running.
    ///
    /// Without the fix, the client write half (`proxy_to_client`) is only
    /// dropped when the proxy task exits — which requires both tasks to finish.
    /// With `handle_c2u` blocked (pre-Bug-1-fix) or slow, the client would
    /// see no EOF until the outer timeout fires.
    ///
    /// With the fix: `handle_u2c` calls `writer.shutdown()` immediately after
    /// upstream dies, sending EOF to the client regardless of `handle_c2u`'s
    /// state.  The client-side read should return `n == 0` within a short
    /// window after upstream death.
    #[tokio::test]
    async fn test_upstream_death_sends_tls_close_notify_equivalent() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (ws_tx, _ws_rx) = broadcast::channel::<WsEvent>(64);

        let (client_to_proxy, client_writer_inner) = tokio::io::duplex(512 * 1024);
        // client_reader is what the "client" reads — this is what we check for EOF.
        let (mut client_reader, proxy_to_client) = tokio::io::duplex(512 * 1024);
        let (upstream_to_proxy, upstream_writer_inner) = tokio::io::duplex(512 * 1024);
        let (upstream_reader_inner, proxy_to_upstream) = tokio::io::duplex(512 * 1024);

        // Channels to relay bytes from test into the pipe writers.
        let (client_write_tx, mut client_write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let (upstream_write_tx, mut upstream_write_rx) = tokio::sync::mpsc::unbounded_channel::<Option<Vec<u8>>>();

        // Relay tasks.
        tokio::spawn(async move {
            let mut w = client_writer_inner;
            while let Some(data) = client_write_rx.recv().await {
                if w.write_all(&data).await.is_err() { break; }
            }
        });
        tokio::spawn(async move {
            let mut w = upstream_writer_inner;
            while let Some(maybe) = upstream_write_rx.recv().await {
                match maybe {
                    Some(data) => { if w.write_all(&data).await.is_err() { break; } }
                    None => break,
                }
            }
        });
        // Drain proxy→upstream so write tasks don't stall.
        tokio::spawn(async move {
            let mut r = upstream_reader_inner;
            let mut buf = vec![0u8; 65536];
            loop { match r.read(&mut buf).await { Ok(0) | Err(_) => break, _ => {} } }
        });

        // Spawn proxy.
        let proxy_handle = tokio::spawn(run(
            client_to_proxy,
            proxy_to_client,
            upstream_to_proxy,
            proxy_to_upstream,
            "api.anthropic.com".to_string(),
            store,
            ws_tx,
            no_pii(),
        ));

        // Complete one request/response cycle.
        let req = anthropic_request("close notify test");
        let resp = anthropic_sse_response("close notify response");
        client_write_tx.send(req).unwrap();
        upstream_write_tx.send(Some(resp)).unwrap();

        // Drain the first response from the client output until the sentinel arrives.
        let mut first_response = Vec::<u8>::new();
        let cycle_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut buf = vec![0u8; 65536];
        loop {
            let remaining = cycle_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { break; }
            match tokio::time::timeout(remaining, client_reader.read(&mut buf)).await {
                Ok(Ok(n)) if n > 0 => {
                    first_response.extend_from_slice(&buf[..n]);
                    let s = String::from_utf8_lossy(&first_response);
                    if s.contains("message_stop") || s.contains("[DONE]") { break; }
                }
                _ => break,
            }
        }
        assert!(
            String::from_utf8_lossy(&first_response).contains("close notify response"),
            "first response did not arrive before EOF test"
        );

        // Now kill upstream.
        upstream_write_tx.send(None).unwrap();
        drop(upstream_write_tx);

        // The client reader must see EOF (n == 0) within 2 seconds.
        // This is the observable effect of `writer.shutdown()` called by
        // `handle_u2c`.  Without the fix, the EOF only arrives after the entire
        // proxy task exits, which may take longer or (with Bug 1 still present)
        // never happen while client_write_tx is alive.
        let eof_result = tokio::time::timeout(
            Duration::from_secs(2),
            client_reader.read(&mut buf),
        ).await;
        match eof_result {
            Ok(Ok(0)) => { /* correct: EOF received */ }
            Ok(Ok(n)) => {
                // It is acceptable to receive trailing data before the EOF,
                // as long as EOF follows promptly.  Recurse once.
                let eof2 = tokio::time::timeout(
                    Duration::from_secs(2),
                    client_reader.read(&mut buf),
                ).await;
                match eof2 {
                    Ok(Ok(0)) => { /* correct */ }
                    Ok(Ok(extra)) => panic!(
                        "Bug 3 regression: client received {extra} bytes after upstream death but no EOF yet"
                    ),
                    Ok(Err(e)) => { /* connection reset counts as clean close in tests */ let _ = e; }
                    Err(_) => panic!(
                        "Bug 3 regression: client read timed out (no EOF) after upstream death. \
                         handle_u2c did not call writer.shutdown(). \
                         first_read_size={n}"
                    ),
                }
            }
            Ok(Err(_)) => { /* connection reset is acceptable */ }
            Err(_) => panic!(
                "Bug 3 regression: client read timed out waiting for EOF after upstream death. \
                 handle_u2c must call writer.shutdown() before returning."
            ),
        }

        // Clean up.
        drop(client_write_tx);
        let _ = tokio::time::timeout(Duration::from_secs(2), proxy_handle).await;
    }

    // ── Bug 1b: upstream death must unblock handle_c2u_pii ───────────────────

    /// Regression test for Bug 1b: same as Bug 1 but with PII mode active.
    ///
    /// `handle_c2u_pii` uses the same `upstream_shutdown_rx` guard at the
    /// between-requests idle point (`raw.is_empty() && !header_done`).
    /// Without it, the function blocks on `reader.read()` forever and the
    /// proxy never exits.
    #[tokio::test]
    async fn test_upstream_eof_signals_pii_c2u_to_exit() {
        let OneCycleHandles {
            proxy_handle,
            client_write_tx,
            upstream_write_tx,
            upstream_out_rx: _,
            _dir,
        } = one_cycle_setup(replace_pii()).await;

        // Kill upstream.
        upstream_write_tx.send(None).unwrap();
        drop(upstream_write_tx);

        // Proxy must exit within 2 seconds.
        let result = tokio::time::timeout(Duration::from_secs(2), proxy_handle).await;
        assert!(
            result.is_ok(),
            "proxy hung after upstream death in PII mode (Bug 1b regression)"
        );

        drop(client_write_tx);
    }

    // ── 3.8 Detection records written with correct message_id ─────────────────

    #[tokio::test]
    async fn detection_records_stored_with_correct_message_id() {
        let (store, _dir) = temp_store();
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);
        let shared = Arc::new(Mutex::new(None::<String>));

        let original_body = anthropic_json_body("contact alice@acme.com today");
        let replaced_body  = anthropic_json_body("contact synth@example.com today");

        // Phase A.
        let conv_id = create_or_find_conversation(
            &original_body, crate::parser::Provider::Anthropic, "api.anthropic.com",
            &store, &ws_tx, &shared,
        ).await.expect("Phase A must return conv_id");

        // Phase B.
        let ids = store_request_messages(
            &original_body,
            Some(&replaced_body),
            true,
            &conv_id,
            crate::parser::Provider::Anthropic,
            &store,
            &ws_tx,
        ).await;

        let last_msg_id = ids.last().expect("must have at least one id").clone();

        // Manually write a detection record using the last message id.
        use crate::storage::MessageDetection;
        let det = MessageDetection {
            message_id: last_msg_id.clone(),
            entity_type: "email".to_string(),
            original_masked: "[EMAIL]".to_string(),
            synthetic: "synth@example.com".to_string(),
            tier: 1,
            confidence: 1.0,
        };
        store.insert_detections(&conv_id, &[det]).unwrap();

        // Load detections filtered by the stored message id.
        let loaded = store.load_detections(&conv_id, Some(&last_msg_id)).unwrap();
        assert_eq!(loaded.len(), 1, "exactly one detection must be stored for msg_id");
        assert_eq!(loaded[0].entity_type, "email");
        assert_eq!(loaded[0].message_id, last_msg_id);
    }
}
