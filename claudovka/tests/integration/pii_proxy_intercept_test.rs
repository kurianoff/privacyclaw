/// End-to-end PII masking / unmasking tests exercising the full proxy intercept
/// pipeline (outbound c2u masking + inbound u2c unmasking via ReplacementBuffer).
///
/// These tests run `intercept::run` with in-memory duplex streams — no real
/// network or CA required.  A "mock upstream" task reads the proxy-forwarded
/// request, verifies outbound PII masking, and writes back an Anthropic SSE
/// response containing the synthetic token.  The test then reads the proxy's
/// decoded response from the client side and verifies that original PII has
/// been restored.
///
/// # Test lifecycle
/// The proxy's c2u task keeps the request-channel open waiting for keep-alive
/// requests, so closing the client writer BEFORE reading the response causes a
/// race that interrupts the u2c task mid-stream.  To avoid this:
///   1. Write the HTTP request (leave client writer open).
///   2. Spawn intercept::run in a background task.
///   3. Upstream mock reads the request, writes SSE response, shuts down.
///   4. Read the decoded response from the client reader until the chunked
///      terminator `0\r\n\r\n` is seen.
///   5. Close the client writer — lets c2u exit gracefully.
///   6. Await intercept task to confirm clean shutdown.
use claudovka::pii::{Locale, PiiContext, PiiMode, PiiPipeline};
use claudovka::pii::vault::VaultRegistry;
use claudovka::proxy::intercept;
use claudovka::storage::Store;
use std::sync::Arc;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_pii_context() -> Arc<PiiContext> {
    Arc::new(PiiContext {
        registry: Arc::new(VaultRegistry::new(Duration::from_secs(3600))),
        locale: Locale::EnUs,
        mode: PiiMode::Replace,
        pipeline: PiiPipeline::tier1_only(),
    })
}

fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (store, dir)
}

/// Build a minimal Anthropic Messages HTTP/1.1 POST request.
fn anthropic_request(content: &str) -> Vec<u8> {
    let body = serde_json::json!({
        "model": "claude-3-5-haiku-20241022",
        "max_tokens": 100,
        "messages": [{"role": "user", "content": content}]
    })
    .to_string();
    format!(
        "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

/// Build a multi-turn Anthropic request.
fn anthropic_multiturn_request(messages: &[(&str, &str)]) -> Vec<u8> {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
        .collect();
    let body = serde_json::json!({
        "model": "claude-3-5-haiku-20241022",
        "max_tokens": 100,
        "messages": msgs
    })
    .to_string();
    format!(
        "POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

/// Build a mock Anthropic SSE response that emits `text` as a single
/// content_block_delta and then terminates the stream cleanly.
fn anthropic_sse_response(text: &str) -> Vec<u8> {
    let msg_start = serde_json::json!({
        "type": "message_start",
        "message": {"id": "msg_test01", "type": "message", "role": "assistant",
            "content": [], "model": "claude-3-5-haiku-20241022",
            "stop_reason": null, "stop_sequence": null,
            "usage": {"input_tokens": 5, "output_tokens": 0}}
    }).to_string();
    let cbs = serde_json::json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "text", "text": ""}}).to_string();
    let delta = serde_json::json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": text}}).to_string();
    let cbe = serde_json::json!({"type": "content_block_stop", "index": 0}).to_string();
    let msg_delta = serde_json::json!({"type": "message_delta",
        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
        "usage": {"output_tokens": 5}}).to_string();
    let msg_stop = serde_json::json!({"type": "message_stop"}).to_string();

    let events = format!(
        "event: message_start\ndata: {msg_start}\n\n\
         event: content_block_start\ndata: {cbs}\n\n\
         event: content_block_delta\ndata: {delta}\n\n\
         event: content_block_stop\ndata: {cbe}\n\n\
         event: message_delta\ndata: {msg_delta}\n\n\
         event: message_stop\ndata: {msg_stop}\n\n"
    );
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{events}").into_bytes()
}

/// Build an SSE response where `text` is split into two consecutive deltas,
/// simulating the synthetic token being split across chunk boundaries.
fn anthropic_sse_response_split(part1: &str, part2: &str) -> Vec<u8> {
    let msg_start = serde_json::json!({"type": "message_start",
        "message": {"id": "msg_split", "type": "message", "role": "assistant",
            "content": [], "model": "claude-3-5-haiku-20241022",
            "stop_reason": null, "stop_sequence": null,
            "usage": {"input_tokens": 5, "output_tokens": 0}}}).to_string();
    let cbs = serde_json::json!({"type": "content_block_start", "index": 0,
        "content_block": {"type": "text", "text": ""}}).to_string();
    let d1 = serde_json::json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": part1}}).to_string();
    let d2 = serde_json::json!({"type": "content_block_delta", "index": 0,
        "delta": {"type": "text_delta", "text": part2}}).to_string();
    let cbe = serde_json::json!({"type": "content_block_stop", "index": 0}).to_string();
    let msg_delta = serde_json::json!({"type": "message_delta",
        "delta": {"stop_reason": "end_turn", "stop_sequence": null},
        "usage": {"output_tokens": 5}}).to_string();
    let msg_stop = serde_json::json!({"type": "message_stop"}).to_string();

    let events = format!(
        "event: message_start\ndata: {msg_start}\n\n\
         event: content_block_start\ndata: {cbs}\n\n\
         event: content_block_delta\ndata: {d1}\n\n\
         event: content_block_delta\ndata: {d2}\n\n\
         event: content_block_stop\ndata: {cbe}\n\n\
         event: message_delta\ndata: {msg_delta}\n\n\
         event: message_stop\ndata: {msg_stop}\n\n"
    );
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n{events}").into_bytes()
}

/// Read an HTTP response from `reader` until complete.  Handles:
/// - Chunked transfer encoding (SSE/PII path): terminates on `0\r\n\r\n`
/// - Content-Length: terminates once all body bytes received
/// - EOF (fallback): terminates when the writer side closes
async fn read_full_response(reader: &mut (impl AsyncReadExt + Unpin)) -> Vec<u8> {
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
/// and return the message content.  Panics if the content-length is missing or
/// the body isn't valid JSON.
async fn read_forwarded_request_content(
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
fn collect_sse_text(response_bytes: &[u8]) -> String {
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
        let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { continue };
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
fn decode_chunked_or_raw(data: &[u8]) -> String {
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

// ── tests ─────────────────────────────────────────────────────────────────────

/// Full round-trip: email PII in request is masked outbound; the synthetic
/// echoed back by the mock LLM is decoded before reaching the client.
#[tokio::test]
async fn intercept_pii_email_roundtrip() {
    let (store, _dir) = make_store();
    let pii_ctx = make_pii_context();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);

    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r, mut client_w) = tokio::io::split(client_io);
    let (mut upstream_r, mut upstream_w) = tokio::io::split(upstream_io);

    // Mock upstream: read masked request, echo synthetic back in SSE.
    let upstream_task = tokio::spawn(async move {
        let masked = read_forwarded_request_content(&mut upstream_r, 0).await;
        assert!(!masked.contains("alice@corp.com"), "PII not masked outbound: {masked}");
        assert!(masked.contains('@'), "masked content should contain synthetic email: {masked}");

        // The masked content is "My email is <synthetic>"; extract the synthetic.
        let synthetic = masked.trim_start_matches("My email is ").trim().to_string();
        let sse = anthropic_sse_response(&format!("Got your email: {synthetic}"));
        upstream_w.write_all(&sse).await.unwrap();
        upstream_w.shutdown().await.unwrap();
    });

    // Intercept proxy.
    let intercept_task = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store, ws_tx, Some(pii_ctx),
    ));

    // Write request (keep writer open — closing it early interrupts u2c).
    client_w.write_all(&anthropic_request("My email is alice@corp.com")).await.unwrap();

    // Read response until chunked terminator, then close writer.
    let response = read_full_response(&mut client_r).await;
    client_w.shutdown().await.unwrap();

    intercept_task.await.unwrap().unwrap();
    upstream_task.await.unwrap();

    let decoded = collect_sse_text(&response);
    assert!(
        decoded.contains("alice@corp.com"),
        "decoded response must contain original email, got: {decoded:?}"
    );
}

/// Phone number round-trip.
#[tokio::test]
async fn intercept_pii_phone_roundtrip() {
    let (store, _dir) = make_store();
    let pii_ctx = make_pii_context();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);

    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r, mut client_w) = tokio::io::split(client_io);
    let (mut upstream_r, mut upstream_w) = tokio::io::split(upstream_io);

    let upstream_task = tokio::spawn(async move {
        let masked = read_forwarded_request_content(&mut upstream_r, 0).await;
        assert!(!masked.contains("555-867-5309"), "phone not masked: {masked}");
        let synthetic = masked.trim_start_matches("My phone is ").trim().to_string();
        upstream_w.write_all(&anthropic_sse_response(&format!("Your phone: {synthetic}"))).await.unwrap();
        upstream_w.shutdown().await.unwrap();
    });

    let intercept_task = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store, ws_tx, Some(pii_ctx),
    ));

    client_w.write_all(&anthropic_request("My phone is 555-867-5309")).await.unwrap();
    let response = read_full_response(&mut client_r).await;
    client_w.shutdown().await.unwrap();

    intercept_task.await.unwrap().unwrap();
    upstream_task.await.unwrap();

    let decoded = collect_sse_text(&response);
    assert!(
        decoded.contains("555-867-5309"),
        "decoded response must contain original phone, got: {decoded:?}"
    );
}

/// Both email and phone in the same message — both masked outbound, both decoded inbound.
#[tokio::test]
async fn intercept_pii_email_and_phone_roundtrip() {
    let (store, _dir) = make_store();
    let pii_ctx = make_pii_context();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);

    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r, mut client_w) = tokio::io::split(client_io);
    let (mut upstream_r, mut upstream_w) = tokio::io::split(upstream_io);

    let upstream_task = tokio::spawn(async move {
        let masked = read_forwarded_request_content(&mut upstream_r, 0).await;
        assert!(!masked.contains("alice@corp.com"), "email not masked: {masked}");
        assert!(!masked.contains("555-867-5309"), "phone not masked: {masked}");
        // Echo entire masked content — contains both synthetics.
        upstream_w.write_all(&anthropic_sse_response(&format!("Received: {masked}"))).await.unwrap();
        upstream_w.shutdown().await.unwrap();
    });

    let intercept_task = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store, ws_tx, Some(pii_ctx),
    ));

    client_w.write_all(&anthropic_request("email alice@corp.com phone 555-867-5309")).await.unwrap();
    let response = read_full_response(&mut client_r).await;
    client_w.shutdown().await.unwrap();

    intercept_task.await.unwrap().unwrap();
    upstream_task.await.unwrap();

    let decoded = collect_sse_text(&response);
    assert!(decoded.contains("alice@corp.com"), "email not decoded: {decoded:?}");
    assert!(decoded.contains("555-867-5309"), "phone not decoded: {decoded:?}");
}

/// Synthetic token split across two SSE content_block_delta events — the
/// ReplacementBuffer must buffer the partial token and decode it once complete.
#[tokio::test]
async fn intercept_pii_split_token_across_sse_events() {
    let (store, _dir) = make_store();
    let pii_ctx = make_pii_context();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);

    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r, mut client_w) = tokio::io::split(client_io);
    let (mut upstream_r, mut upstream_w) = tokio::io::split(upstream_io);

    let upstream_task = tokio::spawn(async move {
        let masked = read_forwarded_request_content(&mut upstream_r, 0).await;
        assert!(!masked.contains("alice@corp.com"), "email not masked: {masked}");

        // Extract synthetic email; split at '@' to force token fragmentation.
        let synthetic = masked.trim_start_matches("contact ").trim().to_string();
        let at_pos = synthetic.find('@').unwrap_or(synthetic.len() / 2);
        let part1 = format!("email: {}", &synthetic[..at_pos]);
        let part2 = synthetic[at_pos..].to_string();

        upstream_w.write_all(&anthropic_sse_response_split(&part1, &part2)).await.unwrap();
        upstream_w.shutdown().await.unwrap();
    });

    let intercept_task = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store, ws_tx, Some(pii_ctx),
    ));

    client_w.write_all(&anthropic_request("contact alice@corp.com")).await.unwrap();
    let response = read_full_response(&mut client_r).await;
    client_w.shutdown().await.unwrap();

    intercept_task.await.unwrap().unwrap();
    upstream_task.await.unwrap();

    let decoded = collect_sse_text(&response);
    assert!(
        decoded.contains("alice@corp.com"),
        "split synthetic must be decoded after reassembly, got: {decoded:?}"
    );
}

/// Anti-chaining: a synthetic email in the assistant turn of a multi-turn
/// conversation must NOT be re-detected and re-masked in the next request.
#[tokio::test]
async fn intercept_pii_no_chaining_in_multiturn() {
    let (store, _dir) = make_store();
    let pii_ctx = make_pii_context();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    // ── Turn 1: single message with original PII ───────────────────────────
    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);
    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r1, mut client_w1) = tokio::io::split(client_io);
    let (mut upstream_r1, mut upstream_w1) = tokio::io::split(upstream_io);

    // Capture the synthetic assigned to alice@corp.com in turn 1.
    let captured_synthetic: Arc<std::sync::Mutex<String>> = Default::default();
    let cap_clone = Arc::clone(&captured_synthetic);

    let upstream_task1 = tokio::spawn(async move {
        let masked = read_forwarded_request_content(&mut upstream_r1, 0).await;
        assert!(!masked.contains("alice@corp.com"), "turn1: email not masked: {masked}");
        let synthetic = masked.trim_start_matches("My email is ").trim().to_string();
        *cap_clone.lock().unwrap() = synthetic.clone();
        upstream_w1.write_all(&anthropic_sse_response(&format!("I see {synthetic}"))).await.unwrap();
        upstream_w1.shutdown().await.unwrap();
    });

    let pii_ctx1 = Arc::clone(&pii_ctx);
    let store1 = store.clone();
    let ws_tx1 = ws_tx.clone();
    let intercept_task1 = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store1, ws_tx1, Some(pii_ctx1),
    ));

    client_w1.write_all(&anthropic_request("My email is alice@corp.com")).await.unwrap();
    let _resp1 = read_full_response(&mut client_r1).await;
    client_w1.shutdown().await.unwrap();

    intercept_task1.await.unwrap().unwrap();
    upstream_task1.await.unwrap();

    let synthetic_from_turn1 = captured_synthetic.lock().unwrap().clone();

    // ── Turn 2: include assistant turn that mentions the synthetic ─────────
    let (client_io2, proxy_client_io2) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io2, upstream_io2) = tokio::io::duplex(128 * 1024);
    let (proxy_client_r2, proxy_client_w2) = tokio::io::split(proxy_client_io2);
    let (proxy_upstream_r2, proxy_upstream_w2) = tokio::io::split(proxy_upstream_io2);
    let (mut client_r2, mut client_w2) = tokio::io::split(client_io2);
    let (mut upstream_r2, mut upstream_w2) = tokio::io::split(upstream_io2);

    let syn_check = synthetic_from_turn1.clone();
    let upstream_task2 = tokio::spawn(async move {
        // Turn 2 request has 3 messages.
        let mut forwarded = Vec::new();
        let mut tmp = vec![0u8; 32 * 1024];
        loop {
            let n = tokio::time::timeout(Duration::from_secs(5), upstream_r2.read(&mut tmp))
                .await.unwrap().unwrap();
            if n == 0 { break; }
            forwarded.extend_from_slice(&tmp[..n]);
            if let Some(hdr_end) = forwarded.windows(4).position(|w| w == b"\r\n\r\n") {
                let hdr = String::from_utf8_lossy(&forwarded[..hdr_end]);
                let cl: usize = hdr.lines()
                    .find(|l| l.to_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1)?.trim().parse().ok())
                    .unwrap_or(0);
                if forwarded.len() >= hdr_end + 4 + cl { break; }
            }
        }
        let hdr_end = forwarded.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let body = &forwarded[hdr_end + 4..];
        let req_json: serde_json::Value = serde_json::from_slice(body).unwrap();
        let msgs = req_json["messages"].as_array().unwrap();

        // No message should contain the original email.
        for (i, msg) in msgs.iter().enumerate() {
            let content = msg["content"].as_str().unwrap_or("");
            assert!(!content.contains("alice@corp.com"),
                "turn2 msg[{i}] leaked original PII: {content}");
        }

        // The assistant message (index 1) had the synthetic from turn 1.
        // Anti-chaining: it must still appear as the SAME synthetic, not
        // as a new third-level synthetic.
        if !syn_check.is_empty() {
            let asst_content = msgs[1]["content"].as_str().unwrap_or("");
            // asst_content in request had the synthetic from turn1; after PII pipeline it
            // should still have that SAME synthetic (not chained to a new one).
            // We verify this by checking the synthetic is_not further replaced:
            // the pipeline must NOT produce a doubly-replaced address.
            // (If chaining occurred, asst_content would have a DIFFERENT token.)
            assert!(
                asst_content.contains(&syn_check) || !asst_content.contains('@'),
                "anti-chaining failed: assistant synthetic was re-replaced in turn2: {asst_content}"
            );
        }

        upstream_w2.write_all(&anthropic_sse_response("Confirmed.")).await.unwrap();
        upstream_w2.shutdown().await.unwrap();
    });

    let pii_ctx2 = Arc::clone(&pii_ctx);
    let store2 = store.clone();
    let ws_tx2 = ws_tx.clone();
    let intercept_task2 = tokio::spawn(intercept::run(
        proxy_client_r2, proxy_client_w2,
        proxy_upstream_r2, proxy_upstream_w2,
        "api.anthropic.com".to_string(),
        store2, ws_tx2, Some(pii_ctx2),
    ));

    let turn2_req = anthropic_multiturn_request(&[
        ("user", "My email is alice@corp.com"),
        ("assistant", &format!("I see {synthetic_from_turn1}")),
        ("user", "Please confirm my email alice@corp.com"),
    ]);
    client_w2.write_all(&turn2_req).await.unwrap();
    let _resp2 = read_full_response(&mut client_r2).await;
    client_w2.shutdown().await.unwrap();

    intercept_task2.await.unwrap().unwrap();
    upstream_task2.await.unwrap();
}

/// PII-off mode: `pii = None` — proxy is byte-identical passthrough with no
/// masking or decoding applied.
#[tokio::test]
async fn intercept_no_pii_passthrough() {
    let (store, _dir) = make_store();
    let (ws_tx, _) = broadcast::channel::<claudovka::dashboard::WsEvent>(8);

    let (client_io, proxy_client_io) = tokio::io::duplex(128 * 1024);
    let (proxy_upstream_io, upstream_io) = tokio::io::duplex(128 * 1024);

    let (proxy_client_r, proxy_client_w) = tokio::io::split(proxy_client_io);
    let (proxy_upstream_r, proxy_upstream_w) = tokio::io::split(proxy_upstream_io);
    let (mut client_r, mut client_w) = tokio::io::split(client_io);
    let (mut upstream_r, mut upstream_w) = tokio::io::split(upstream_io);

    let upstream_task = tokio::spawn(async move {
        let content = read_forwarded_request_content(&mut upstream_r, 0).await;
        assert!(content.contains("alice@corp.com"), "no-PII must not mask: {content}");

        let resp_body = r#"{"content":[{"text":"alice@corp.com"}]}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            resp_body.len(), resp_body
        );
        upstream_w.write_all(resp.as_bytes()).await.unwrap();
        upstream_w.shutdown().await.unwrap();
    });

    let intercept_task = tokio::spawn(intercept::run(
        proxy_client_r, proxy_client_w,
        proxy_upstream_r, proxy_upstream_w,
        "api.anthropic.com".to_string(),
        store, ws_tx, None, // PII disabled
    ));

    client_w.write_all(&anthropic_request("My email is alice@corp.com")).await.unwrap();
    let response = read_full_response(&mut client_r).await;
    client_w.shutdown().await.unwrap();

    intercept_task.await.unwrap().unwrap();
    upstream_task.await.unwrap();

    let resp_str = String::from_utf8_lossy(&response);
    assert!(
        resp_str.contains("alice@corp.com"),
        "no-PII passthrough must forward email unchanged: {resp_str:?}"
    );
}
