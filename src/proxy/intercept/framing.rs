use anyhow::Result;
use tokio::io::{AsyncWrite, AsyncWriteExt};

use super::UPSTREAM_WRITE_TIMEOUT;

// ─── Per-request HTTP framing state ──────────────────────────────────────────

/// Per-request mutable state shared between the two `handle_c2u_*` variants.
///
/// Both `handle_c2u_passthrough` and `handle_c2u_pii` track the same set of
/// framing fields; this struct groups them so that each per-request reset is a
/// single `state = HttpFramingState::default()` rather than five separate
/// assignments.
#[derive(Default)]
pub(super) struct HttpFramingState {
    pub(super) header_done: bool,
    pub(super) content_length: Option<usize>,
    pub(super) is_chunked: bool,
    pub(super) body_start: usize,
    /// Bytes of `raw` already written to upstream.
    /// For chunked requests stays at 0 until the complete body is available;
    /// for all other framing bytes are forwarded eagerly.
    pub(super) forwarded: usize,
    /// Body bytes received so far (PII path only; ignored in passthrough).
    pub(super) body_received: usize,
}

// ─── HTTP request framing helpers ─────────────────────────────────────────────

pub(super) fn find_header_end(data: &[u8]) -> Option<usize> {
    for i in 0..data.len().saturating_sub(3) {
        if data[i] == b'\r' && data[i+1] == b'\n' && data[i+2] == b'\r' && data[i+3] == b'\n' {
            return Some(i + 4);
        }
    }
    None
}

pub(super) fn parse_content_length(headers: &[u8]) -> Option<usize> {
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
pub(super) fn decode_chunked_body(data: &[u8]) -> Option<Vec<u8>> {
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
pub(super) fn is_chunked_encoding(headers: &[u8]) -> bool {
    let text = std::str::from_utf8(headers).unwrap_or("");
    text.lines().any(|line| {
        let lo = line.to_lowercase();
        lo.starts_with("transfer-encoding:") && lo.contains("chunked")
    })
}

/// Find the end of a chunked-encoded body, returning the byte offset just past
/// the terminal chunk (`\r\n0\r\n\r\n`). Returns `None` if the terminator is
/// not yet present in `body`.
pub(super) fn find_chunked_body_end(body: &[u8]) -> Option<usize> {
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

/// Rebuild an HTTP/1.1 request replacing `Transfer-Encoding: chunked` with
/// `Content-Length: <body.len()>`.  Also strips any existing `Content-Length`
/// header to avoid conflicts.  The reconstructed request is suitable for
/// forwarding to an upstream that requires a known content length.
pub(super) fn rebuild_request_with_content_length(raw_headers: &[u8], body: &[u8]) -> Vec<u8> {
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

/// Write `data` as a single HTTP/1.1 chunked-encoding chunk.
/// Used when PII mode reconstructs SSE events and must maintain
/// the `Transfer-Encoding: chunked` framing the upstream sent.
pub(super) async fn write_http_chunk(writer: &mut (impl AsyncWrite + Unpin), data: &[u8]) -> Result<()> {
    let header = format!("{:X}\r\n", data.len());
    writer.write_all(header.as_bytes()).await?;
    writer.write_all(data).await?;
    writer.write_all(b"\r\n").await?;
    Ok(())
}

pub(super) async fn upstream_write(
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
pub(super) struct ChunkedDecoder {
    pub(super) buf: Vec<u8>,
    pub(super) chunk_remaining: usize,
    pub(super) state: ChunkDecoderState,
}

#[derive(PartialEq)]
pub(super) enum ChunkDecoderState {
    ReadingSize,
    ReadingBody,
    BodyTrail,
}

impl ChunkedDecoder {
    pub(super) fn new() -> Self {
        Self { buf: Vec::new(), chunk_remaining: 0, state: ChunkDecoderState::ReadingSize }
    }

    /// Push raw framed bytes; returns decoded payload bytes.
    pub(super) fn push(&mut self, raw: &[u8]) -> Vec<u8> {
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
