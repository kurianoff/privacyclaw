/// E2E tests for the proxy runtime and dashboard API.
///
/// Each test spawns a real privacyclaw binary with an isolated config (random
/// ports, temp storage directory).  The `ProxyHandle` guard kills the child
/// process when the test finishes so ports are freed even on failure.
///
/// These tests are intentionally coarse-grained: they verify the proxy starts,
/// the dashboard responds with correct shapes, and the stop endpoint works.
/// Fine-grained interception logic is covered by the inline unit tests in
/// `proxy/intercept.rs`.
#[path = "helpers.rs"]
mod helpers;
use helpers::*;

use std::io::{Read, Write};
use std::time::Duration;

// ── Dashboard API ─────────────────────────────────────────────────────────────

/// After start, GET /api/version must return HTTP 200 with a `version` field.
#[test]
fn test_dashboard_version_endpoint() {
    let proxy = ProxyHandle::start();
    let (status, body) = http_get(proxy.dashboard_port, "/api/version")
        .expect("GET /api/version");
    assert_eq!(status, 200, "/api/version body: {body}");
    assert!(
        body.contains("\"version\""),
        "/api/version missing 'version' field: {body}"
    );
}

/// GET /api/proxy/status must report running=true while the proxy is up.
#[test]
fn test_dashboard_proxy_status_running() {
    let proxy = ProxyHandle::start();
    let (status, body) = http_get(proxy.dashboard_port, "/api/proxy/status")
        .expect("GET /api/proxy/status");
    assert_eq!(status, 200, "proxy/status body: {body}");
    // Accept both compact and pretty-printed JSON.
    assert!(
        body.contains("\"running\":true") || body.contains("\"running\": true"),
        "proxy should be running: {body}"
    );
}

/// GET /api/config must return HTTP 200 with proxy/pii configuration keys.
#[test]
fn test_dashboard_config_endpoint() {
    let proxy = ProxyHandle::start();
    let (status, body) = http_get(proxy.dashboard_port, "/api/config")
        .expect("GET /api/config");
    assert_eq!(status, 200, "/api/config body: {body}");
    assert!(
        body.contains("pii") || body.contains("proxy"),
        "/api/config missing expected fields: {body}"
    );
}

/// GET /api/conversations must return HTTP 200 and a JSON array.
#[test]
fn test_dashboard_conversations_endpoint_returns_array() {
    let proxy = ProxyHandle::start();
    let (status, body) = http_get(proxy.dashboard_port, "/api/conversations")
        .expect("GET /api/conversations");
    assert_eq!(status, 200, "/api/conversations body: {body}");
    let trimmed = body.trim();
    assert!(
        trimmed.starts_with('['),
        "/api/conversations should be a JSON array, got: {trimmed}"
    );
}

/// Unknown API routes must return 404 (not a panic / 500).
#[test]
fn test_dashboard_unknown_route_returns_404() {
    let proxy = ProxyHandle::start();
    let (status, _) = http_get(proxy.dashboard_port, "/api/nonexistent_endpoint_xyz")
        .expect("GET unknown route");
    assert_eq!(status, 404, "unknown route should return 404");
}

/// POST /api/proxy/stop must return ok=true and the process must exit cleanly.
#[test]
fn test_dashboard_stop_endpoint() {
    let proxy = ProxyHandle::start();
    let dashboard_port = proxy.dashboard_port;

    let (status, body) =
        http_post(dashboard_port, "/api/proxy/stop", "{}").expect("POST /api/proxy/stop");
    assert_eq!(status, 200, "stop endpoint body: {body}");
    assert!(
        body.contains("\"ok\":true") || body.contains("\"ok\": true"),
        "stop should return ok:true: {body}"
    );

    // Give the process a moment to terminate.
    std::thread::sleep(Duration::from_millis(500));

    // Dashboard should no longer be reachable.
    let still_up = wait_for_port(dashboard_port, Duration::from_millis(200));
    assert!(
        !still_up,
        "dashboard port {dashboard_port} is still listening after stop"
    );
}

// ── Proxy port ────────────────────────────────────────────────────────────────

/// The CONNECT proxy port must accept raw TCP connections.
#[test]
fn test_proxy_port_accepts_tcp_connections() {
    let proxy = ProxyHandle::start();
    let conn = std::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.proxy_port));
    assert!(
        conn.is_ok(),
        "proxy port {} refused connection: {:?}",
        proxy.proxy_port,
        conn.err()
    );
}

/// Sending a malformed HTTP request to the proxy port must not crash the proxy
/// (dashboard must remain reachable afterwards).
#[test]
fn test_proxy_survives_malformed_request() {
    let proxy = ProxyHandle::start();

    // Send garbage bytes to the proxy port.
    if let Ok(mut conn) = std::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.proxy_port)) {
        let _ = conn.write_all(b"GARBAGE /not/http/1.1\r\n\r\n");
    }

    std::thread::sleep(Duration::from_millis(100));

    // Dashboard should still respond.
    let (status, _) = http_get(proxy.dashboard_port, "/api/version")
        .expect("dashboard after malformed request");
    assert_eq!(status, 200, "proxy crashed after malformed request");
}

/// Sending a valid CONNECT request to the proxy must receive a 200 tunnel response.
/// We start a local TCP listener to ensure the upstream target is reachable.
#[test]
fn test_proxy_connect_returns_200() {
    let proxy = ProxyHandle::start();

    // Bind a listener so the upstream target is definitely reachable.
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").expect("bind upstream");
    let target_port = upstream.local_addr().expect("local_addr").port();

    // Accept in background so the CONNECT doesn't block.
    std::thread::spawn(move || {
        let _ = upstream.accept(); // accept and drop — we only need the handshake
    });

    let request = format!(
        "CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\nHost: 127.0.0.1:{target_port}\r\n\r\n"
    );

    let mut conn = std::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.proxy_port))
        .expect("connect to proxy");
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
    conn.write_all(request.as_bytes()).expect("write CONNECT");

    // Read the first line of the response.
    let mut buf = [0u8; 256];
    let n = conn.read(&mut buf).unwrap_or(0);
    let response_line = std::str::from_utf8(&buf[..n]).unwrap_or("").lines().next().unwrap_or("");

    assert!(
        response_line.contains("200"),
        "CONNECT did not return 200: got {response_line:?}"
    );
}

// ── Full lifecycle ────────────────────────────────────────────────────────────

/// Start → verify running → stop via dashboard → verify stopped.
/// This is the canonical install/runtime/stop lifecycle test.
#[test]
fn test_full_proxy_lifecycle() {
    // --- START ---
    let proxy = ProxyHandle::start();
    let dashboard_port = proxy.dashboard_port;

    // --- VERIFY RUNNING ---
    let (status, body) = http_get(dashboard_port, "/api/proxy/status")
        .expect("status while running");
    assert_eq!(status, 200);
    assert!(
        body.contains("true"),
        "expected running=true: {body}"
    );

    // --- VERSION ---
    let (status, body) = http_get(dashboard_port, "/api/version").expect("version");
    assert_eq!(status, 200);
    assert!(body.contains("version"), "version field missing: {body}");

    // --- CONFIG ---
    let (status, _) = http_get(dashboard_port, "/api/config").expect("config");
    assert_eq!(status, 200);

    // --- CONVERSATIONS (empty list) ---
    let (status, body) = http_get(dashboard_port, "/api/conversations").expect("conversations");
    assert_eq!(status, 200);
    assert!(body.trim().starts_with('['), "not a JSON array: {body}");

    // --- STOP ---
    let (status, body) = http_post(dashboard_port, "/api/proxy/stop", "{}")
        .expect("stop");
    assert_eq!(status, 200, "stop body: {body}");
    assert!(
        body.contains("true"),
        "stop should return ok:true: {body}"
    );

    // --- VERIFY STOPPED ---
    std::thread::sleep(Duration::from_millis(600));
    let still_up = wait_for_port(dashboard_port, Duration::from_millis(200));
    assert!(
        !still_up,
        "dashboard still listening after stop (port {dashboard_port})"
    );
}
