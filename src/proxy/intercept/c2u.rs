use crate::dashboard::WsEvent;
use crate::parser::Provider;
use crate::pii::{self, PiiCtx, PiiDetection, PiiMode};
use crate::storage::{Conversation, Message, MessageDetection, Store};
use crate::util::{new_uuid, now_iso8601};
use crate::pii::vault::VaultHandle;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{broadcast, oneshot};

use super::READ_BUF;
use super::framing::{
    decode_chunked_body, find_chunked_body_end, find_header_end, is_chunked_encoding,
    parse_content_length, rebuild_request_with_content_length, upstream_write,
};

// ─── Client → Upstream ───────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_c2u(
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

// ─── Phase A / Phase B request logging ───────────────────────────────────────

/// Phase A: create or find conversation, set shared_conv_id, broadcast ConversationStart.
/// Returns the conv_id. Does NOT store messages.
pub(super) async fn create_or_find_conversation(
    body: &[u8],
    provider: Provider,
    host: &str,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
) -> Option<String> {
    let parsed = crate::parser::parse_request(provider, body)?;
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
pub(super) async fn store_request_messages(
    original_body: &[u8],
    replaced_body: Option<&[u8]>,
    pii_processed: bool,
    conv_id: &str,
    provider: Provider,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
) -> Vec<String> {
    let Some(parsed_orig) = crate::parser::parse_request(provider, original_body) else {
        return vec![];
    };

    let store_clone = store.clone();
    let conv_id_owned = conv_id.to_string();
    let orig_messages = parsed_orig.messages.clone();

    // Parse replaced body outside spawn_blocking (no I/O).
    let repl_messages: Option<Vec<crate::parser::Message>> = replaced_body
        .and_then(|b| crate::parser::parse_request(provider, b))
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
pub(super) async fn log_request(
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

/// Format a PII entity type as a bracketed label, e.g. `"email"` → `"[EMAIL]"`.
pub(super) fn entity_type_label(entity_type: &str) -> String {
    format!("[{}]", entity_type.to_uppercase())
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
