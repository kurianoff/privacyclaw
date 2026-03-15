/// Shared helpers for integration tests.
///
/// Include in test files with:
///   #[path = "helpers.rs"]
///   mod helpers;
///   use helpers::*;
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Read an HTTP response from `reader` until complete. Handles:
/// - Chunked transfer encoding (SSE/PII path): terminates on `0\r\n\r\n`
/// - Content-Length: terminates once all body bytes received
/// - EOF (fallback): terminates when the writer side closes
pub async fn read_full_response(reader: &mut (impl AsyncReadExt + Unpin)) -> Vec<u8> {
    let mut response = Vec::new();
    let mut tmp = vec![0u8; 4096];
    let mut header_end: Option<usize> = None;
    let mut content_length: Option<usize> = None;

    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), reader.read(&mut tmp))
            .await
            .expect("read_full_response: timeout waiting for proxy response")
            .expect("read_full_response: I/O error");
        if n == 0 {
            break; // EOF
        }
        response.extend_from_slice(&tmp[..n]);

        // Locate header/body boundary on first pass.
        if header_end.is_none() {
            if let Some(pos) = response.windows(4).position(|w| w == b"\r\n\r\n") {
                header_end = Some(pos + 4);
                let headers = String::from_utf8_lossy(&response[..pos]);
                content_length = headers
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1)?.trim().parse().ok());
            }
        }

        // Content-Length: stop once we have all body bytes.
        if let (Some(hdr), Some(cl)) = (header_end, content_length) {
            if response.len() >= hdr + cl {
                break;
            }
        }
        // Chunked (SSE PII path): stop on chunked terminator.
        if response.windows(5).any(|w| w == b"0\r\n\r\n") {
            break;
        }
    }
    response
}

/// Read the full forwarded request from `upstream_reader`, parse the JSON body,
/// and return the message content at `msg_index`.
/// Panics if the content-length is missing or the body isn't valid JSON.
pub async fn read_forwarded_request_content(
    upstream_reader: &mut (impl AsyncReadExt + Unpin),
    msg_index: usize,
) -> String {
    let mut forwarded = Vec::new();
    let mut tmp = vec![0u8; 32 * 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), upstream_reader.read(&mut tmp))
            .await
            .expect("upstream_reader timeout")
            .expect("upstream_reader I/O error");
        if n == 0 {
            break;
        }
        forwarded.extend_from_slice(&tmp[..n]);
        if let Some(hdr_end) = forwarded.windows(4).position(|w| w == b"\r\n\r\n") {
            let hdr = String::from_utf8_lossy(&forwarded[..hdr_end]);
            let cl: usize = hdr
                .lines()
                .find(|l| l.to_lowercase().starts_with("content-length:"))
                .and_then(|l| l.split(':').nth(1)?.trim().parse().ok())
                .unwrap_or(0);
            if forwarded.len() >= hdr_end + 4 + cl {
                break;
            }
        }
    }
    let hdr_end = forwarded.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let body = &forwarded[hdr_end + 4..];
    let req_json: serde_json::Value = serde_json::from_slice(body).unwrap();
    req_json["messages"][msg_index]["content"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Extract all `text` field values from `content_block_delta` events in a
/// (possibly chunked) HTTP SSE response.
pub fn collect_sse_text(response_bytes: &[u8]) -> String {
    let body_start = response_bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .unwrap_or(0);
    let body = &response_bytes[body_start..];
    let text = decode_chunked_or_raw(body);

    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let data = line.trim_start_matches("data:").trim();
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else {
            continue;
        };
        if v["type"].as_str() == Some("content_block_delta")
            && v["delta"]["type"].as_str() == Some("text_delta")
        {
            if let Some(t) = v["delta"]["text"].as_str() {
                out.push_str(t);
            }
        }
    }
    out
}

/// Decode HTTP/1.1 chunked transfer encoding or return the input verbatim.
pub fn decode_chunked_or_raw(data: &[u8]) -> String {
    let mut result = Vec::new();
    let mut pos = 0;
    loop {
        let Some(nl_off) = data[pos..].windows(2).position(|w| w == b"\r\n") else {
            return String::from_utf8_lossy(data).into_owned();
        };
        let size_str = std::str::from_utf8(&data[pos..pos + nl_off]).unwrap_or("0");
        let chunk_size = usize::from_str_radix(size_str.trim(), 16).unwrap_or(0);
        if chunk_size == 0 {
            break;
        }
        pos += nl_off + 2;
        if pos + chunk_size > data.len() {
            result.extend_from_slice(&data[pos..]);
            break;
        }
        result.extend_from_slice(&data[pos..pos + chunk_size]);
        pos += chunk_size + 2;
        if pos >= data.len() {
            break;
        }
    }
    if result.is_empty() {
        String::from_utf8_lossy(data).into_owned()
    } else {
        String::from_utf8(result).unwrap_or_default()
    }
}
