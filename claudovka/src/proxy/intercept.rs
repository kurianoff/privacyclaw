use crate::dashboard::WsEvent;
use crate::parser::{self, Provider};
use crate::parser::sse::SseParser;
use crate::storage::{Conversation, Message, Store};
use crate::util::{fmt_chunk_hex, fmt_headers, new_uuid, now_iso8601};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, oneshot};

const MAX_SSE_BUFFER: usize = 10 * 1024 * 1024; // 10 MB
const READ_BUF: usize = 65536;

/// How long u_to_c waits for any upstream data before giving up.
/// Resets on every chunk received — this is an idle timeout, not a total timeout.
const UPSTREAM_READ_TIMEOUT: Duration = Duration::from_secs(120);

/// How long c_to_u waits for a single write to upstream to complete.
const UPSTREAM_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle a fully decrypted bidirectional stream between client and upstream.
///
/// Both directions run as independent spawned tasks (not select!) so that
/// u_to_c always runs finalize_response when it exits — even if the client
/// disconnects mid-stream. c_to_u sends a oneshot signal when it finishes;
/// u_to_c uses that signal plus an idle timeout to know when to stop reading.
pub async fn run(
    client_reader: impl AsyncRead + Unpin + Send + 'static,
    client_writer: impl AsyncWrite + Unpin + Send + 'static,
    upstream_reader: impl AsyncRead + Unpin + Send + 'static,
    upstream_writer: impl AsyncWrite + Unpin + Send + 'static,
    host: String,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
) -> Result<()> {
    let provider = Provider::from_host(&host);
    tracing::warn!(host = %host, provider = provider.as_str(), "intercept: session started");

    // Shared conversation ID set by request handler, read by response handler
    let shared_conv_id: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // c_to_u signals u_to_c when the client side is done so u_to_c can
    // finalize and exit instead of waiting forever on a dead connection.
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let c_to_u = tokio::spawn({
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let shared_conv_id = Arc::clone(&shared_conv_id);
        let host_clone = host.clone();
        async move {
            let result = handle_c2u(
                client_reader, upstream_writer, provider,
                store, ws_tx, host_clone, shared_conv_id,
            ).await;
            // Fire regardless of success/error so u_to_c always unblocks.
            let _ = shutdown_tx.send(());
            if let Err(e) = result {
                tracing::debug!("c→u closed: {}", e);
            }
        }
    });

    let u_to_c = tokio::spawn(async move {
        if let Err(e) = handle_u2c(
            upstream_reader, client_writer, provider,
            store, ws_tx, shared_conv_id, shutdown_rx,
        ).await {
            tracing::debug!("u→c closed: {}", e);
        }
    });

    // Wait for both — neither is cancelled; u_to_c always runs finalize_response.
    let _ = tokio::join!(c_to_u, u_to_c);
    tracing::warn!(host = %host, "intercept: session ended");
    Ok(())
}

// ─── Client → Upstream ───────────────────────────────────────────────────────

async fn handle_c2u(
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
        tracing::debug!(
            raw_len = raw.len(), header_done, body_received,
            content_length = ?content_length,
            "c2u: loop"
        );
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            tracing::debug!("c2u: EOF from client");
            break;
        }

        let chunk = &buf[..n];
        tracing::info!(bytes = n, "c2u: read chunk from client");
        tracing::debug!(
            chunk_hex = %fmt_chunk_hex(chunk, 256),
            total_bytes = n,
            "c2u: chunk data"
        );

        match tokio::time::timeout(UPSTREAM_WRITE_TIMEOUT, writer.write_all(chunk)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => anyhow::bail!("upstream write stalled for {}s", UPSTREAM_WRITE_TIMEOUT.as_secs()),
        }
        tracing::info!(bytes = n, "c2u: forwarded chunk to upstream");

        raw.extend_from_slice(chunk);

        if !header_done {
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                body_start = hdr_end;
                content_length = parse_content_length(&raw[..hdr_end]);
                body_received = raw.len() - body_start;
                let headers_text = String::from_utf8_lossy(&raw[..hdr_end]);
                tracing::debug!(
                    body_start, content_length = ?content_length,
                    "c2u: header delimiter found"
                );
                tracing::debug!(headers = %fmt_headers(&headers_text), "c2u: HTTP headers");
            }
        } else {
            body_received += chunk.len();
        }

        // Parse once we have the full request body (Content-Length requests).
        if header_done && !parse_attempted {
            if let Some(len) = content_length {
                let body = &raw[body_start..];
                if body.len() >= len {
                    tracing::info!(body_bytes = len, "c2u: full request body received");
                    parse_attempted = true;
                    // Flush the upstream TLS writer so all buffered records are sent.
                    // Without this, large requests (600KB+) can leave the last records in
                    // the TLS send buffer: Anthropic waits for the remaining Content-Length
                    // bytes, times out, and closes the connection — causing u2c to see a
                    // silent EOF and exit before any response arrives.
                    if let Err(e) = writer.flush().await {
                        tracing::debug!("c2u: upstream flush error after request body: {}", e);
                        return Err(e.into());
                    }
                    tracing::debug!("c2u: upstream flushed after full request body");
                    log_request(body, provider, &host, &store, &ws_tx, &shared_conv_id).await;
                    raw.clear();
                    raw.shrink_to_fit();
                }
            }
        }

        // Reset per-request state when body is fully forwarded.
        // Enables logging subsequent requests on the same keep-alive connection.
        if header_done {
            if let Some(len) = content_length {
                if body_received >= len {
                    tracing::debug!(body_received, "c2u: resetting per-request state");
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

    // Chunked / unknown-length request body: parse at end of stream.
    if header_done && content_length.is_none() && !parse_attempted {
        let body = &raw[body_start..];
        if !body.is_empty() {
            tracing::debug!(body_bytes = body.len(), "c2u: chunked/unknown-length body end");
            log_request(body, provider, &host, &store, &ws_tx, &shared_conv_id).await;
        }
    }

    Ok(())
}

async fn log_request(
    body: &[u8],
    provider: Provider,
    host: &str,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
) {
    let t0 = std::time::Instant::now();
    let Some(parsed) = parser::parse_request(provider, body) else {
        tracing::debug!("Could not parse request body for {} ({} bytes)", host, body.len());
        return;
    };
    tracing::debug!(
        elapsed_ms = t0.elapsed().as_millis(),
        body_bytes = body.len(),
        msgs = parsed.messages.len(),
        "log_request: parse_request done"
    );

    // Fingerprint = sha1 of first message role+content.
    // Lets us detect when a new API request is a continuation of an existing conversation
    // (LLM APIs are stateless — each turn resends the full history).
    let fingerprint = conversation_fingerprint(&parsed.messages);

    // Capture everything needed for the blocking closure.
    let store_clone = store.clone();
    let provider_str = provider.as_str().to_string();
    let model = parsed.model.clone();
    let t_clone = std::time::Instant::now();
    let messages = parsed.messages.clone();
    tracing::debug!(
        elapsed_ms = t_clone.elapsed().as_millis(),
        content_bytes = messages.iter().map(|m| m.content.len()).sum::<usize>(),
        "log_request: messages cloned"
    );

    // ALL blocking file I/O runs on a blocking thread — never stalls the tokio executor.
    let t_spawn = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let t_sb = std::time::Instant::now();

        let t_find = std::time::Instant::now();
        let (conv_id, msg_offset, is_new) =
            match store_clone.find_conversation_by_fingerprint(&provider_str, &fingerprint) {
                Some(existing_id) => {
                    tracing::debug!(
                        elapsed_ms = t_find.elapsed().as_millis(),
                        "storage: find_conversation_by_fingerprint (found)"
                    );
                    let t_count = std::time::Instant::now();
                    let stored = store_clone.count_request_messages(&existing_id);
                    tracing::debug!(
                        elapsed_ms = t_count.elapsed().as_millis(),
                        stored,
                        "storage: count_request_messages"
                    );
                    tracing::debug!(
                        conv_id = %existing_id,
                        stored_msgs = stored,
                        total_msgs = messages.len(),
                        "Continuing conversation"
                    );
                    (existing_id, stored, false)
                }
                None => {
                    tracing::debug!(
                        elapsed_ms = t_find.elapsed().as_millis(),
                        "storage: find_conversation_by_fingerprint (not found)"
                    );
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
                    tracing::info!(
                        conv_id = %new_id,
                        provider = %provider_str,
                        model = %model,
                        "New conversation"
                    );
                    (new_id, 0, true)
                }
            };

        // Only store messages that are new since the last request on this conversation.
        let new_messages = messages.get(msg_offset..).unwrap_or(&[]);
        if new_messages.is_empty() {
            tracing::debug!(elapsed_ms = t_sb.elapsed().as_millis(), "storage: spawn_blocking done (no new msgs)");
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

        let t_batch = std::time::Instant::now();
        if let Err(e) = store_clone.batch_insert_messages(&stored_msgs) {
            tracing::warn!("Failed to store request messages: {}", e);
        }
        tracing::debug!(
            elapsed_ms = t_batch.elapsed().as_millis(),
            msgs = stored_msgs.len(),
            "storage: batch_insert_messages"
        );
        tracing::debug!(elapsed_ms = t_sb.elapsed().as_millis(), "storage: spawn_blocking done");

        Ok::<_, anyhow::Error>((conv_id, is_new, model, stored_msgs))
    }).await;
    tracing::debug!(
        elapsed_ms = t_spawn.elapsed().as_millis(),
        "log_request: spawn_blocking total (incl. queue wait)"
    );

    let Ok(Ok((conv_id, is_new, model, stored_msgs))) = result else {
        return;
    };

    // Set the shared conv_id so u_to_c can tag the response.
    *shared_conv_id.lock().unwrap() = Some(conv_id.clone());

    // Broadcast events to dashboard (async, non-blocking).
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

/// SHA-1 fingerprint of the first message's role+content.
/// Used to detect when consecutive API requests are turns of the same conversation.
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

// ─── Upstream → Client ───────────────────────────────────────────────────────

async fn handle_u2c(
    mut reader: impl AsyncRead + Unpin,
    mut writer: impl AsyncWrite + Unpin,
    provider: Provider,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    shared_conv_id: Arc<Mutex<Option<String>>>,
    mut shutdown: oneshot::Receiver<()>,
) -> Result<()> {
    let mut buf = vec![0u8; READ_BUF];

    // Per-response state — reset after each response completes.
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
    let mut chunk_count: u64 = 0;
    let mut t_last_chunk = std::time::Instant::now();

    loop {
        tracing::debug!(
            chunk_count, header_done, is_sse, body_received,
            content_length = ?content_length,
            "u2c: loop"
        );
        // Race: upstream data, client-side shutdown signal, or idle timeout.
        // The idle timeout resets on every chunk — it only fires when upstream
        // stops sending entirely (dead connection, not just a slow model).
        let n = tokio::select! {
            result = tokio::time::timeout(UPSTREAM_READ_TIMEOUT, reader.read(&mut buf)) => {
                match result {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => {
                        tracing::debug!("u2c: upstream read error: {}", e);
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            timeout_secs = UPSTREAM_READ_TIMEOUT.as_secs(),
                            "u2c: upstream idle timeout, closing"
                        );
                        break;
                    }
                }
            }
            _ = &mut shutdown => {
                tracing::debug!("u2c: client side closed, finalizing remaining response");
                break;
            }
        };
        if n == 0 {
            tracing::debug!("u2c: upstream EOF");
            break;
        }
        chunk_count += 1;
        let t_gap = t_last_chunk.elapsed();

        let chunk = &buf[..n];
        tracing::info!(bytes = n, chunk = chunk_count, "u2c: read chunk from upstream");
        tracing::debug!(
            chunk_hex = %fmt_chunk_hex(chunk, 256),
            total_bytes = n,
            "u2c: chunk data"
        );

        // Forward immediately — zero latency.
        let t_write = std::time::Instant::now();
        writer.write_all(chunk).await?;
        let write_ms = t_write.elapsed().as_millis();
        tracing::info!(bytes = n, write_ms, chunk = chunk_count, "u2c: forwarded chunk to client");

        if write_ms > 10 || t_gap.as_millis() > 100 {
            tracing::debug!(
                chunk = chunk_count,
                bytes = n,
                gap_ms = t_gap.as_millis(),
                write_ms,
                "u2c: slow chunk"
            );
        }
        t_last_chunk = std::time::Instant::now();

        let t_proc = std::time::Instant::now();
        let sse_done = if !header_done {
            raw.extend_from_slice(chunk);
            if let Some(hdr_end) = find_header_end(&raw) {
                header_done = true;
                let headers_text = String::from_utf8_lossy(&raw[..hdr_end]);
                is_sse = headers_text.contains("text/event-stream");
                content_length = parse_content_length(&raw[..hdr_end]);
                tracing::debug!(is_sse, content_length = ?content_length, "u2c: response headers parsed");
                tracing::debug!(headers = %fmt_headers(&headers_text), "u2c: response HTTP headers");

                // Process body bytes that arrived in the same read as headers,
                // passing the slice directly before clearing raw (avoids a Vec allocation).
                if hdr_end < raw.len() {
                    body_received += raw.len() - hdr_end;
                    let done = process_response_chunk(
                        &raw[hdr_end..], is_sse, &mut sse_parser, &mut accumulated,
                        &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                        &mut tokens_in, &mut tokens_out, &mut body_buf,
                    );
                    raw.clear();
                    raw.shrink_to_fit();
                    done
                } else {
                    raw.clear();
                    raw.shrink_to_fit();
                    false
                }
            } else {
                false
            }
        } else {
            body_received += chunk.len();
            process_response_chunk(
                chunk, is_sse, &mut sse_parser, &mut accumulated,
                &mut accumulation_stopped, provider, &ws_tx, &shared_conv_id,
                &mut tokens_in, &mut tokens_out, &mut body_buf,
            )
        };
        let proc_ms = t_proc.elapsed().as_millis();
        if proc_ms > 5 {
            tracing::debug!(
                chunk = chunk_count,
                proc_ms,
                "u2c: slow process_response_chunk"
            );
        }

        // Response is complete when:
        //   - SSE: process_response_chunk saw data: [DONE]
        //   - Non-SSE: all Content-Length bytes received
        let response_complete = sse_done
            || (!is_sse && content_length.is_some_and(|cl| body_received >= cl));

        if response_complete && header_done {
            tracing::info!(
                body_received, is_sse, chunks = chunk_count,
                complete_reason = if sse_done { "SSE [DONE]" } else { "Content-Length" },
                "u2c: response complete"
            );
            finalize_response(
                &shared_conv_id, &store, &ws_tx, provider,
                &mut accumulated, &mut body_buf, tokens_in, tokens_out,
            ).await;

            // Reset per-response state for the next cycle on this keep-alive connection.
            tracing::debug!(chunks_this_response = chunk_count, accumulated_len = accumulated.len(), "u2c: resetting per-response state");
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
        }
    }

    // Always finalize on exit: shutdown signal, idle timeout, EOF, or error.
    // finalize_response is a no-op if accumulated and body_buf are already empty
    // (i.e. the response already completed normally inside the loop above).
    finalize_response(
        &shared_conv_id, &store, &ws_tx, provider,
        &mut accumulated, &mut body_buf, tokens_in, tokens_out,
    ).await;

    Ok(())
}

/// Store the completed response and broadcast ResponseComplete to the dashboard.
#[allow(clippy::too_many_arguments, clippy::ptr_arg)]
async fn finalize_response(
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    store: &Store,
    ws_tx: &broadcast::Sender<WsEvent>,
    provider: Provider,
    accumulated: &mut String,
    body_buf: &mut Vec<u8>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
) {
    let conv_id = shared_conv_id.lock().unwrap().clone();
    tracing::debug!(
        conv_id_present = conv_id.is_some(),
        accumulated_len = accumulated.len(),
        body_buf_len = body_buf.len(),
        "finalize_response: called"
    );
    let Some(cid) = conv_id else { return; };

    let content = if !accumulated.is_empty() {
        std::mem::take(accumulated)
    } else {
        match provider {
            Provider::Anthropic =>
                crate::parser::anthropic::extract_response_content(body_buf).unwrap_or_default(),
            Provider::OpenAI =>
                crate::parser::openai::extract_response_content(body_buf).unwrap_or_default(),
            _ => String::from_utf8_lossy(body_buf).to_string(),
        }
    };

    if !content.is_empty() {
        let content_len = content.len();
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
        let t_fin = std::time::Instant::now();
        if let Err(e) = tokio::task::spawn_blocking(move || store_clone.insert_message(&msg_clone)).await {
            tracing::warn!("Failed to store response: {}", e);
        }
        tracing::debug!(
            elapsed_ms = t_fin.elapsed().as_millis(),
            "finalize_response: insert_message spawn_blocking done"
        );
        tracing::info!(
            conv_id = %cid,
            content_len,
            tokens_in = ?tokens_in,
            tokens_out = ?tokens_out,
            "finalize_response: stored response"
        );
    }

    let _ = ws_tx.send(WsEvent::ResponseComplete {
        conversation_id: cid,
        tokens_in,
        tokens_out,
    });
}

/// Process a chunk of response body bytes.
///
/// Returns `true` if the SSE stream signalled completion (`data: [DONE]`).
/// For non-SSE responses, always returns `false` (completion is detected by
/// the caller via Content-Length tracking).
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
    // Non-SSE: accumulate for final parsing; completion detected by Content-Length.
    if !is_sse {
        body_buf.extend_from_slice(chunk);
        return false;
    }

    // Read conv_id once per chunk — avoids a mutex lock per SSE event.
    let cid_opt = shared_conv_id.lock().unwrap().clone();

    for event in sse_parser.push(chunk) {
        tracing::debug!(
            event_type = ?event.event_type,
            data_len = event.data.len(),
            "u2c: SSE event"
        );
        tracing::debug!(
            data = %event.data.chars().take(256).collect::<String>(),
            "u2c: SSE data"
        );

        if SseParser::is_done_sentinel(&event) {
            tracing::debug!("u2c: SSE [DONE] sentinel detected");
            return true; // SSE stream complete
        }

        if provider == Provider::Anthropic {
            // Anthropic ends SSE with message_stop, not [DONE]
            if event.event_type.as_deref() == Some("message_stop") {
                tracing::debug!("u2c: Anthropic message_stop detected");
                return true;
            }
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

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Returns the index just past the HTTP header block (after `\r\n\r\n`).
fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i+1] == b'\n' && data[i+2] == b'\r' && data[i+3] == b'\n' {
            return Some(i + 4);
        }
    }
    None
}

/// Parse `Content-Length` value from raw header bytes.
fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(headers).ok()?;
    for line in text.lines() {
        let lower = line.to_lowercase();
        if let Some(rest) = lower.strip_prefix("content-length:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

// ─── Proxy pipeline tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dashboard::WsEvent;
    use crate::storage::Store;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::broadcast;
    use tempfile::TempDir;

    fn temp_store() -> (Store, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        (store, dir)
    }

    /// Build an HTTP request with Anthropic JSON body.
    fn make_anthropic_request(n_turns: usize) -> Vec<u8> {
        let mut messages = Vec::new();
        for i in 0..n_turns {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            messages.push(serde_json::json!({"role": role, "content": format!("Message {}", i)}));
        }
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "max_tokens": 1024,
            "messages": messages
        }).to_string();
        format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(), body
        ).into_bytes()
    }

    /// Build an HTTP SSE response with n_events content events + message_stop.
    fn make_anthropic_sse_response(n_events: usize) -> Vec<u8> {
        let mut sse = String::new();
        for i in 0..n_events {
            sse.push_str(&format!(
                "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"word{}\"}}}}\n\n",
                i
            ));
        }
        sse.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        let http_header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        let mut resp = http_header.into_bytes();
        resp.extend_from_slice(sse.as_bytes());
        resp
    }

    /// Run the proxy with given request and response bytes.
    /// Returns (forwarded_request_bytes, received_response_bytes).
    async fn run_proxy(
        request: Vec<u8>,
        response: Vec<u8>,
        host: &str,
        store: Store,
    ) -> (Vec<u8>, Vec<u8>) {
        let buf_size = (request.len() + response.len() + 1024 * 1024).max(2 * 1024 * 1024);

        let (client_end, proxy_client) = tokio::io::duplex(buf_size);
        let (upstream_end, proxy_upstream) = tokio::io::duplex(buf_size);

        let (proxy_cr, proxy_cw) = tokio::io::split(proxy_client);
        let (proxy_ur, proxy_uw) = tokio::io::split(proxy_upstream);

        let (ws_tx, _) = broadcast::channel::<WsEvent>(128);

        let req_len = request.len();

        // Client task: write request, shutdown, read response
        let client_task = tokio::spawn(async move {
            let (mut cr, mut cw) = tokio::io::split(client_end);
            cw.write_all(&request).await.unwrap();
            cw.shutdown().await.unwrap();
            let mut resp = Vec::new();
            cr.read_to_end(&mut resp).await.unwrap();
            resp
        });

        // Upstream task: read forwarded request, write response, shutdown
        let upstream_task = tokio::spawn(async move {
            let (mut ur, mut uw) = tokio::io::split(upstream_end);
            let mut fwd_req = vec![0u8; req_len];
            ur.read_exact(&mut fwd_req).await.unwrap();
            uw.write_all(&response).await.unwrap();
            uw.shutdown().await.unwrap();
            fwd_req
        });

        // Run proxy (blocks until done)
        run(proxy_cr, proxy_cw, proxy_ur, proxy_uw, host.to_string(), store, ws_tx)
            .await
            .unwrap();

        let fwd_req = upstream_task.await.unwrap();
        let received_resp = client_task.await.unwrap();

        (fwd_req, received_resp)
    }

    // ── 2.1 Roundtrip fidelity ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_small_request_response_forwarded_verbatim() {
        let (store, _dir) = temp_store();
        let request = make_anthropic_request(1);
        let response = make_anthropic_sse_response(3);

        let (fwd_req, received_resp) = run_proxy(
            request.clone(), response.clone(), "api.anthropic.com", store,
        ).await;

        assert_eq!(fwd_req, request, "Forwarded request must be byte-for-byte identical");
        assert_eq!(received_resp, response, "Received response must be byte-for-byte identical");
    }

    #[tokio::test]
    async fn test_large_request_response_forwarded_verbatim() {
        // 40-turn request ~600KB — regression test for TLS flush fix
        let (store, _dir) = temp_store();
        let request = make_anthropic_request(40);
        let response = make_anthropic_sse_response(10);

        let (fwd_req, received_resp) = run_proxy(
            request.clone(), response.clone(), "api.anthropic.com", store,
        ).await;

        assert_eq!(fwd_req, request, "Large request must be forwarded verbatim");
        assert_eq!(received_resp, response, "Response must be forwarded verbatim");
    }

    // ── 2.4 Upstream failure modes ──────────────────────────────────────────

    #[tokio::test]
    async fn test_upstream_immediate_eof_no_panic() {
        let (store, _dir) = temp_store();
        let request = make_anthropic_request(1);

        let buf_size = 1024 * 1024;
        let (client_end, proxy_client) = tokio::io::duplex(buf_size);
        let (upstream_end, proxy_upstream) = tokio::io::duplex(buf_size);

        let (proxy_cr, proxy_cw) = tokio::io::split(proxy_client);
        let (proxy_ur, proxy_uw) = tokio::io::split(proxy_upstream);
        let (ws_tx, _) = broadcast::channel::<WsEvent>(16);

        let req_len = request.len();

        // Client sends request and shuts down
        let client_task = tokio::spawn(async move {
            let (mut cr, mut cw) = tokio::io::split(client_end);
            cw.write_all(&request).await.unwrap();
            cw.shutdown().await.unwrap();
            let mut resp = Vec::new();
            cr.read_to_end(&mut resp).await.unwrap();
            resp
        });

        // Upstream reads request then immediately closes without sending any response
        let upstream_task = tokio::spawn(async move {
            let (mut ur, mut uw) = tokio::io::split(upstream_end);
            let mut fwd = vec![0u8; req_len];
            ur.read_exact(&mut fwd).await.unwrap();
            // Immediately shutdown without writing any response
            uw.shutdown().await.unwrap();
        });

        // Proxy should not panic, should exit cleanly
        let result = run(proxy_cr, proxy_cw, proxy_ur, proxy_uw, "api.anthropic.com".to_string(), store, ws_tx).await;
        assert!(result.is_ok(), "Proxy should not error on upstream immediate EOF");

        upstream_task.await.unwrap();
        let _ = client_task.await.unwrap(); // response will be empty
    }

    // ── 2.5 SSE streaming correctness ──────────────────────────────────────

    #[tokio::test]
    async fn test_anthropic_message_stop_terminates_stream() {
        let (store, _dir) = temp_store();
        let request = make_anthropic_request(1);
        let response = make_anthropic_sse_response(5);

        let (ws_tx, mut ws_rx) = broadcast::channel::<WsEvent>(128);
        let mut ws_rx_conv = ws_tx.subscribe();

        let buf = 2 * 1024 * 1024;
        // Four unidirectional pairs — avoids BrokenPipe from dropped writer halves
        let (mut test_req_write, proxy_cr) = tokio::io::duplex(buf); // test→proxy
        let (proxy_cw, mut test_resp_read) = tokio::io::duplex(buf); // proxy→test
        let (proxy_ur, mut us_resp_write) = tokio::io::duplex(buf);  // upstream→proxy
        let (mut us_req_read, proxy_uw) = tokio::io::duplex(buf);    // proxy→upstream

        let req_len = request.len();

        // Client writes request but does NOT shutdown — keeps c2u alive until test ends.
        // Response side unblocks when proxy drops proxy_cw (u2c exits after SSE done).
        let client_task = tokio::spawn(async move {
            test_req_write.write_all(&request).await.unwrap();
            let mut resp = Vec::new();
            test_resp_read.read_to_end(&mut resp).await.unwrap();
            // Drop test_req_write here (task exit) to signal EOF to c2u
        });

        // Upstream reads the forwarded request, waits for ConversationStart (guarantees
        // shared_conv_id is set by log_request's spawn_blocking), then sends the response.
        let upstream_task = tokio::spawn(async move {
            let mut fwd = vec![0u8; req_len];
            us_req_read.read_exact(&mut fwd).await.unwrap();
            let _ = tokio::time::timeout(
                tokio::time::Duration::from_secs(5),
                async {
                    loop {
                        match ws_rx_conv.recv().await {
                            Ok(WsEvent::ConversationStart { .. }) => break,
                            Ok(_) => continue,
                            Err(_) => break,
                        }
                    }
                },
            ).await;
            us_resp_write.write_all(&response).await.unwrap();
            us_resp_write.shutdown().await.unwrap();
        });

        run(proxy_cr, proxy_cw, proxy_ur, proxy_uw, "api.anthropic.com".to_string(), store, ws_tx).await.unwrap();
        client_task.await.unwrap();
        upstream_task.await.unwrap();

        let mut events = Vec::new();
        while let Ok(ev) = ws_rx.try_recv() {
            events.push(ev);
        }

        let has_response_complete = events.iter().any(|e| matches!(e, WsEvent::ResponseComplete { .. }));
        assert!(has_response_complete, "ResponseComplete event must be fired after message_stop");
    }

    // ── 2.6 Storage integration ─────────────────────────────────────────────

    #[tokio::test]
    async fn test_new_conversation_created_on_first_request() {
        let (store, dir) = temp_store();
        let request = make_anthropic_request(1);
        let response = make_anthropic_sse_response(3);

        run_proxy(request, response, "api.anthropic.com", store).await;

        // Check that a conversation file was created
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("ndjson"))
            .collect();
        assert_eq!(files.len(), 1, "Exactly one conversation file must be created");
    }
}

// ─── Performance tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod perf_tests {
    use super::*;
    use crate::storage::Store;
    use std::time::{Duration, Instant};
    use tokio::io::AsyncWriteExt;

    /// Build a realistic Anthropic request body with `n_turns` of conversation history.
    /// Each turn has ~200 chars of user text and ~300 chars of assistant text.
    fn make_large_request_body(n_turns: usize) -> Vec<u8> {
        let mut messages = Vec::new();
        for i in 0..n_turns {
            let user_content = format!(
                "This is turn {} of the conversation. I am asking about an important topic \
                 that requires a detailed and thoughtful response from the assistant. \
                 Please consider all the context provided so far.",
                i + 1
            );
            messages.push(format!(
                r#"{{"role":"user","content":{}}}"#,
                serde_json::to_string(&user_content).unwrap()
            ));
            if i + 1 < n_turns {
                let asst_content = format!(
                    "Certainly! For turn {} I'll provide a thorough answer. \
                     Here is my detailed response addressing your question about \
                     the important topic you mentioned. The key points are: \
                     first, context matters; second, precision is important; \
                     third, always verify your assumptions before proceeding.",
                    i + 1
                );
                messages.push(format!(
                    r#"{{"role":"assistant","content":{}}}"#,
                    serde_json::to_string(&asst_content).unwrap()
                ));
            }
        }
        let body = format!(
            r#"{{"model":"claude-opus-4-5-20251001","max_tokens":4096,"stream":true,"system":"You are a helpful assistant. Always be detailed and thorough in your responses.","messages":[{}]}}"#,
            messages.join(",")
        );
        body.into_bytes()
    }

    /// Build an HTTP request envelope around a body.
    fn make_http_request(body: &[u8]) -> Vec<u8> {
        let headers = format!(
            "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let mut req = headers.into_bytes();
        req.extend_from_slice(body);
        req
    }

    /// Build a synthetic Anthropic SSE response with `n_events` text deltas.
    fn make_sse_response(n_events: usize, chars_per_event: usize) -> Vec<u8> {
        let mut body = Vec::new();

        // message_start event
        body.extend_from_slice(
            b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_01\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"claude-opus-4-5-20251001\",\"stop_reason\":null,\"usage\":{\"input_tokens\":100,\"output_tokens\":0}}}\n\n"
        );
        // content_block_start
        body.extend_from_slice(
            b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n"
        );

        let chunk_text = "a".repeat(chars_per_event);
        let delta_json = format!(
            "{{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}",
            chunk_text
        );
        for _ in 0..n_events {
            body.extend_from_slice(b"event: content_block_delta\n");
            body.extend_from_slice(b"data: ");
            body.extend_from_slice(delta_json.as_bytes());
            body.extend_from_slice(b"\n\n");
        }

        // message_stop
        body.extend_from_slice(
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":500}}\n\n"
        );
        body.extend_from_slice(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        body.extend_from_slice(b"data: [DONE]\n\n");

        body
    }

    fn make_sse_http_response(sse_body: &[u8]) -> Vec<u8> {
        let headers =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
        let mut resp = headers.as_bytes().to_vec();
        resp.extend_from_slice(sse_body);
        resp
    }

    /// Full pipeline test: simulate large request + SSE response through intercept::run.
    ///
    /// Wiring:
    ///   client_writer  →[client_rx]→  proxy  →[upstream_tx]→  upstream_writer_rx
    ///   upstream_resp_tx  →[upstream_rx]→  proxy  →[client_tx]→  response_reader
    ///
    /// The mock upstream task reads the forwarded request bytes then writes the SSE
    /// response back through its own pipe that the proxy reads as "upstream reader".
    #[tokio::test]
    async fn test_long_conversation_latency() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("claudovka=debug")
            .with_test_writer()
            .try_init();

        // ── parameters ───────────────────────────────────────────────────────
        let n_turns = 30;
        let n_sse_events = 2000;
        let chars_per_event = 5;

        // ── build payloads ────────────────────────────────────────────────────
        let req_body = make_large_request_body(n_turns);
        let req_bytes = make_http_request(&req_body);
        let sse_body = make_sse_response(n_sse_events, chars_per_event);
        let resp_bytes = make_sse_http_response(&sse_body);
        eprintln!(
            "\n[PERF] request: {} KB, SSE response: {} KB ({} events)",
            req_bytes.len() / 1024,
            resp_bytes.len() / 1024,
            n_sse_events,
        );

        // ── temp store ────────────────────────────────────────────────────────
        let tmp = std::env::temp_dir().join(format!("claudovka_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let store = Store::open(&tmp).unwrap();
        let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel(64);

        // ── four duplex pipes ─────────────────────────────────────────────────
        // 1. client → proxy (request)
        let (mut client_writer, client_reader) = tokio::io::duplex(1 << 20);
        // 2. proxy → client (response forwarded by proxy)
        let (mut response_reader, client_out_writer) = tokio::io::duplex(1 << 20);
        // 3. proxy → upstream (forwarded request bytes)
        let (mut upstream_req_reader, upstream_req_writer) = tokio::io::duplex(1 << 20);
        // 4. upstream → proxy (SSE response)
        let (upstream_resp_reader, mut upstream_resp_writer) = tokio::io::duplex(1 << 20);

        // ── mock upstream: drain request in background, write SSE response immediately ──
        // Real servers don't wait for EOF on the request before responding —
        // they start sending as soon as they've processed enough of the body.
        // If we await EOF here we deadlock: the proxy never drops upstream_req_writer
        // while the client connection stays open.
        let resp_bytes_clone = resp_bytes.clone();
        let upstream_task = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;

            // Drain request bytes in background (the proxy's c_to_u writes these).
            let _drain = tokio::spawn(async move {
                let mut buf = [0u8; 65536];
                loop {
                    match upstream_req_reader.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });

            // Write SSE response immediately.
            let t = Instant::now();
            upstream_resp_writer.write_all(&resp_bytes_clone).await.ok();
            drop(upstream_resp_writer); // EOF → proxy's u_to_c exits after [DONE]
            eprintln!("[UPSTREAM] sent {} KB SSE in {}ms", resp_bytes_clone.len() / 1024, t.elapsed().as_millis());
        });

        // ── proxy task ────────────────────────────────────────────────────────
        let proxy_task = tokio::spawn(async move {
            crate::proxy::intercept::run(
                client_reader,
                client_out_writer,
                upstream_resp_reader,   // proxy reads SSE response from here
                upstream_req_writer,    // proxy writes forwarded request here
                "api.anthropic.com".to_string(),
                store,
                ws_tx,
            ).await
        });

        // ── send request, measure time to receive full response ───────────────
        let t_total = Instant::now();

        client_writer.write_all(&req_bytes).await.unwrap();
        eprintln!("[CLIENT] sent request ({} KB)", req_bytes.len() / 1024);
        // Do NOT drop client_writer yet: in real HTTP/1.1 the client keeps the
        // connection open while waiting for the response. Dropping it here causes
        // c_to_u to get EOF and return, which makes select! cancel u_to_c before
        // it reads any upstream data. Instead, keep client_writer alive;
        // u_to_c will exit naturally after seeing [DONE], which terminates select!
        // and closes client_out_writer, which gives response_reader an EOF.

        let mut response = Vec::new();
        let t_resp = Instant::now();
        let _ = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::io::copy(&mut response_reader, &mut response),
        ).await;

        eprintln!(
            "[CLIENT] received {} KB in {}ms (resp latency: {}ms)",
            response.len() / 1024,
            t_total.elapsed().as_millis(),
            t_resp.elapsed().as_millis(),
        );
        drop(client_writer);

        // The response bytes forwarded by the proxy must match what upstream sent.
        assert_eq!(
            response, resp_bytes,
            "proxy must forward response bytes verbatim"
        );

        upstream_task.abort();
        proxy_task.abort();
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Targeted microbenchmark: how long does parse_request take for large bodies?
    #[tokio::test]
    async fn test_parse_request_scaling() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("claudovka=debug")
            .with_test_writer()
            .try_init();

        for n_turns in [1, 5, 10, 20, 30, 50] {
            let body = make_large_request_body(n_turns);
            let t = Instant::now();
            let parsed = crate::parser::parse_request(Provider::Anthropic, &body);
            let elapsed = t.elapsed();
            eprintln!(
                "[PARSE] {} turns → {} KB body → {} messages → {}ms",
                n_turns,
                body.len() / 1024,
                parsed.map(|p| p.messages.len()).unwrap_or(0),
                elapsed.as_millis()
            );
        }
    }

    /// Microbenchmark: SSE parser drain performance for large chunks.
    #[test]
    fn test_sse_parser_drain_scaling() {
        use crate::parser::sse::SseParser;

        for n_events in [100, 500, 1000, 2000, 5000] {
            let sse = make_sse_response(n_events, 5);
            let t = Instant::now();
            let mut parser = SseParser::new();
            // Feed the entire SSE body as one big chunk (worst case for drain)
            let events = parser.push(&sse);
            let elapsed = t.elapsed();
            eprintln!(
                "[SSE] {} events, {} KB body → parsed {} events in {}ms ({} µs/event)",
                n_events,
                sse.len() / 1024,
                events.len(),
                elapsed.as_millis(),
                elapsed.as_micros() / n_events.max(1) as u128,
            );
        }
    }

    /// Microbenchmark: serde_json parse per SSE event (current approach).
    #[test]
    fn test_sse_delta_json_parse_per_event() {
        use crate::parser::sse::SseParser;

        let n_events = 2000;
        let sse = make_sse_response(n_events, 5);
        let mut parser = SseParser::new();
        let events = parser.push(&sse);

        let t = Instant::now();
        let mut delta_count = 0;
        for event in &events {
            if let Some(_d) = crate::parser::anthropic::extract_sse_delta(event) {
                delta_count += 1;
            }
        }
        let elapsed = t.elapsed();
        eprintln!(
            "[JSON/EVENT] {} events, {} deltas → {}ms total ({} µs/event)",
            events.len(),
            delta_count,
            elapsed.as_millis(),
            elapsed.as_micros() / events.len().max(1) as u128,
        );
    }

    /// Microbenchmark: storage find_conversation_by_fingerprint with growing file count.
    #[tokio::test]
    async fn test_storage_find_scaling() {
        let tmp = std::env::temp_dir().join(format!("claudovka_store_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let store = Store::open(&tmp).unwrap();

        // Pre-populate with N conversations of varying message counts
        for i in 0..20 {
            let conv_id = uuid::Uuid::new_v4().to_string();
            let conv = crate::storage::Conversation {
                id: conv_id.clone(),
                started_at: chrono::Utc::now().to_rfc3339(),
                provider: "anthropic".to_string(),
                model: Some("claude-opus-4-5-20251001".to_string()),
                client_hint: Some(format!("fingerprint_{}", i)),
            };
            store.insert_conversation(&conv).unwrap();

            // Add messages to simulate a long conversation
            let msgs: Vec<crate::storage::Message> = (0..i * 5).map(|j| crate::storage::Message {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: conv_id.clone(),
                direction: if j % 2 == 0 { "request".to_string() } else { "response".to_string() },
                timestamp: chrono::Utc::now().to_rfc3339(),
                role: Some("user".to_string()),
                content: "x".repeat(1000), // 1KB per message
                tokens_in: None,
                tokens_out: None,
            }).collect();
            if !msgs.is_empty() {
                store.batch_insert_messages(&msgs).unwrap();
            }
        }

        // Now measure lookup time
        let t = Instant::now();
        let result = store.find_conversation_by_fingerprint("anthropic", "fingerprint_19");
        let elapsed = t.elapsed();
        eprintln!(
            "[STORAGE] find with 20 conversations (up to 95 msgs each) → {}ms, found={}",
            elapsed.as_millis(),
            result.is_some()
        );

        // Measure insert_message on a conversation with many existing messages
        if let Some(cid) = result {
            let msg = crate::storage::Message {
                id: uuid::Uuid::new_v4().to_string(),
                conversation_id: cid.clone(),
                direction: "response".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                role: Some("assistant".to_string()),
                content: "y".repeat(5000), // 5KB response
                tokens_in: Some(100),
                tokens_out: Some(500),
            };
            let t2 = Instant::now();
            store.insert_message(&msg).unwrap();
            eprintln!("[STORAGE] insert_message into large conv → {}ms", t2.elapsed().as_millis());
        }

        std::fs::remove_dir_all(&tmp).ok();
    }
}
