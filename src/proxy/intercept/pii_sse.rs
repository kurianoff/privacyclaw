use crate::dashboard::WsEvent;
use crate::parser::{self, Provider};
use crate::parser::sse::SseParser;
use crate::pii::buffer::ReplacementBuffer;
use crate::util::{new_uuid, now_iso8601};
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::broadcast;

use super::MAX_SSE_BUFFER;
use super::framing::write_http_chunk;

// ─── SSE helpers ─────────────────────────────────────────────────────────────

pub(super) fn reconstruct_sse_event_bytes(event_type: Option<&str>, data: &str) -> Vec<u8> {
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

/// Returns the serde_json pointer to the text-delta field for the given provider,
/// or `None` for `Provider::Unknown`.
///
/// Used by both `extract_text_delta` and `replace_text_delta` to locate the
/// text field without duplicating provider-specific JSON paths.
fn text_delta_pointer(provider: Provider) -> Option<&'static str> {
    match provider {
        Provider::Anthropic => Some("/delta/text"),
        Provider::OpenAI   => Some("/choices/0/delta/content"),
        Provider::Google   => Some("/candidates/0/content/parts/0/text"),
        Provider::Unknown  => None,
    }
}

pub(super) fn extract_text_delta(provider: Provider, data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    // Anthropic requires a type guard before reading the text pointer.
    if provider == Provider::Anthropic {
        if v["type"].as_str() != Some("content_block_delta")
            || v["delta"]["type"].as_str() != Some("text_delta")
        {
            return None;
        }
    }
    let ptr = text_delta_pointer(provider)?;
    v.pointer(ptr).and_then(|t| t.as_str()).map(|s| s.to_string())
}

pub(super) fn replace_text_delta(provider: Provider, data: &str, new_text: &str) -> String {
    let Ok(mut v) = serde_json::from_str::<serde_json::Value>(data) else {
        return data.to_string();
    };
    if let Some(ptr) = text_delta_pointer(provider) {
        if let Some(t) = v.pointer_mut(ptr) {
            *t = serde_json::Value::String(new_text.to_string());
        }
    }
    serde_json::to_string(&v).unwrap_or_else(|_| data.to_string())
}

pub(super) fn make_trailing_text_event(provider: Provider, text: &str) -> Option<Vec<u8>> {
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

// ─── PII SSE chunk processing ─────────────────────────────────────────────────

/// Write `event_bytes` as an HTTP chunk and flush. Thin semantic alias used
/// consistently within pii_sse.rs to forward a complete SSE event to the client.
async fn emit_event_chunk(
    writer: &mut (impl AsyncWrite + Unpin),
    event_bytes: &[u8],
) -> Result<()> {
    write_http_chunk(writer, event_bytes).await?;
    writer.flush().await?;
    Ok(())
}

/// Process one text delta through the replacement buffer, emit a WS TextDelta
/// event for any flushed output, and return the event bytes to forward to the
/// client (with PII-replaced text inserted, or an empty-text delta while the
/// buffer is still holding).
#[allow(clippy::too_many_arguments)]
async fn emit_text_delta(
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    accumulated: &mut String,
    synthetic_acc: &mut String,
    accumulation_stopped: &mut bool,
    rep_buf: &mut ReplacementBuffer,
    text: &str,
    provider: Provider,
    event_type: Option<&str>,
    raw_data: &str,
) -> Result<Vec<u8>> {
    // Accumulate raw LLM text (pre-restoration) for storage.
    if !*accumulation_stopped {
        synthetic_acc.push_str(text);
    }

    let flushed = rep_buf.process_delta(text);

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
        replaced = (flushed != text),
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
        let new_data = replace_text_delta(provider, raw_data, &flushed);
        Ok(reconstruct_sse_event_bytes(event_type, &new_data))
    } else {
        // Buffer holding: emit empty text delta to keep SSE stream alive.
        let new_data = replace_text_delta(provider, raw_data, "");
        Ok(reconstruct_sse_event_bytes(event_type, &new_data))
    }
}

/// Accumulate a passthrough SSE text delta (no rep_buf active) and emit a WS
/// TextDelta event. Returns the reconstructed SSE event bytes to forward.
fn forward_non_pii_delta(
    ws_tx: &broadcast::Sender<WsEvent>,
    shared_conv_id: &Arc<Mutex<Option<String>>>,
    accumulated: &mut String,
    accumulation_stopped: &mut bool,
    provider: Provider,
    event: &crate::parser::sse::SseEvent,
) -> Vec<u8> {
    if let Some(delta) = parser::extract_sse_delta(provider, event) {
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
}

/// Flush any buffered trailing text from `rep_buf`, send a TextDelta WS event,
/// and write it as an HTTP chunk. Called before forwarding `content_block_stop`
/// and `message_stop` so trailing PII-restored text reaches the client while
/// the content block is still open.
#[allow(clippy::too_many_arguments)]
pub(super) async fn flush_rep_buf_remaining_as_chunk(
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
pub(super) async fn process_sse_chunk_pii(
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
            emit_event_chunk(writer, &reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
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
                emit_event_chunk(writer, &reconstruct_sse_event_bytes(event.event_type.as_deref(), &event.data)).await?;
                return Ok(true);
            }
            let (ti, to) = crate::parser::anthropic::extract_message_start_tokens(&event);
            if ti.is_some() { *tokens_in = ti; }
            if to.is_some() { *tokens_out = to; }
        }

        let maybe_text = extract_text_delta(provider, &event.data);

        let event_bytes = if let (Some(text), Some(ref mut rb)) = (maybe_text.as_ref(), &mut *rep_buf) {
            emit_text_delta(
                ws_tx, shared_conv_id,
                accumulated, synthetic_acc, accumulation_stopped,
                rb, text, provider,
                event.event_type.as_deref(), &event.data,
            ).await?
        } else {
            // Non-text event or no rep_buf: forward reconstructed.
            if maybe_text.is_some() {
                // rep_buf is None — text delta forwarded without PII decoding
                tracing::debug!(rep_buf_is_none = true, "u2c_pii: text delta bypassing PII decoding (rep_buf=None)");
            }
            forward_non_pii_delta(ws_tx, shared_conv_id, accumulated, accumulation_stopped, provider, &event)
        };

        emit_event_chunk(writer, &event_bytes).await?;
    }
    Ok(false)
}
