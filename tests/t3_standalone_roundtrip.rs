//! T3 Standalone mode roundtrip integration tests.
//!
//! These tests use a real `SlmSidecar` pointing at a minimal in-process mock HTTP
//! server.  No real llama-server is required.  All mocks run in the same process
//! via `tokio::net::TcpListener`.

use privacyclaw::parser::Provider;
use privacyclaw::pii::buffer::ReplacementBuffer;
use privacyclaw::pii::inject_system_instruction;
use privacyclaw::pii::tier3::SlmSidecar;
use privacyclaw::pii::vault::{PiiVault, PiiType};
use privacyclaw::pii::{PiiPipeline, SYSTEM_REMINDER};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ── helpers ────────────────────────────────────────────────────────────────────

/// Spin up a mock HTTP server that always responds with `body_json` content in
/// an OpenAI-compatible chat completion response.  Returns the port number.
/// The server accepts exactly one connection then stops.
async fn mock_slm_server(content: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let response_body = format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":"{}"}}}}]}}"#,
        content
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(),
        response_body
    );

    tokio::spawn(async move {
        // Accept up to 4 connections so multi-message bodies work.
        for _ in 0..4 {
            match listener.accept().await {
                Ok((mut stream, _)) => {
                    // Read until end of headers
                    let mut raw = Vec::new();
                    loop {
                        let mut tmp = vec![0u8; 4096];
                        let n = stream.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 { break; }
                        raw.extend_from_slice(&tmp[..n]);
                        if raw.windows(4).any(|w| w == b"\r\n\r\n") { break; }
                    }
                    // Parse Content-Length and read body
                    let header_end = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap_or(raw.len());
                    let headers_str = std::str::from_utf8(&raw[..header_end]).unwrap_or("");
                    let content_length: usize = headers_str.lines()
                        .find(|l| l.to_lowercase().starts_with("content-length:"))
                        .and_then(|l| l.split(':').nth(1))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    let mut body_bytes = raw[header_end + 4..].to_vec();
                    while body_bytes.len() < content_length {
                        let mut tmp = vec![0u8; 4096];
                        let n = stream.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 { break; }
                        body_bytes.extend_from_slice(&tmp[..n]);
                    }
                    let _ = stream.write_all(response.as_bytes()).await;
                }
                Err(_) => break,
            }
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    port
}

// ── T3 standalone outbound: PII replaced in forwarded body, vault populated ───

/// process_body_t3_standalone wraps PII tokens in the body and populates the
/// vault so the inbound buffer can reverse them.
///
/// Spec scenario: "End-to-end standalone replacement and reversal"
#[tokio::test]
async fn t3_standalone_outbound_populates_vault() {
    // Mock SLM: wraps "Alice" and "Acme" as §token§.
    let port = mock_slm_server("My name is §Alice§ and I work at §Acme§").await;
    let endpoint = format!("http://127.0.0.1:{}", port);

    let vault_inner = PiiVault::new("t3-roundtrip-conv");
    let vault_handle = Arc::new(RwLock::new(vault_inner));

    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = endpoint;
    cfg.slm.timeout_ms = 2000;

    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm_standalone, "pipeline must be in T3 standalone mode");

    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "My name is Alice and I work at Acme"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let result = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &privacyclaw::pii::Locale::EnUs)
        .await;

    let (modified_bytes, detections) = result.expect("T3 standalone must return modified body");

    // The forwarded body must contain § tokens.
    let modified_json: serde_json::Value = serde_json::from_slice(&modified_bytes).unwrap();
    let content = modified_json["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("§Alice§"),
        "forwarded body must contain §Alice§, got: {content}");
    assert!(content.contains("§Acme§"),
        "forwarded body must contain §Acme§, got: {content}");
    assert!(!content.contains("Alice "), // "Alice " without § — original leaked
        "original 'Alice' must not appear unguarded in forwarded body: {content}");

    // Vault must have mappings for both originals.
    let vault = vault_handle.read().unwrap();
    assert_eq!(vault.get_synthetic("Alice"), Some("§Alice§"),
        "vault must map Alice → §Alice§");
    assert_eq!(vault.get_synthetic("Acme"), Some("§Acme§"),
        "vault must map Acme → §Acme§");
    assert!(!vault.is_empty(), "vault must not be empty after T3 standalone replacement");

    // Detections list must record both entities.
    assert_eq!(detections.len(), 2,
        "expected 2 PiiDetection entries, got: {:?}", detections);
    let originals: Vec<&str> = detections.iter().map(|d| d.original.as_str()).collect();
    assert!(originals.contains(&"Alice"), "Alice must appear in detections");
    assert!(originals.contains(&"Acme"), "Acme must appear in detections");
}

// ── T3 standalone inbound: ReplacementBuffer restores originals ───────────────

/// After T3 standalone replacement, the inbound ReplacementBuffer must reverse
/// §token§ markers back to their originals using the populated vault.
///
/// Spec scenario: "End-to-end standalone replacement and reversal" (inbound leg)
#[tokio::test]
async fn t3_standalone_inbound_restores_original() {
    // Build a vault with the mappings T3 standalone would have created.
    let mut vault = PiiVault::new("t3-inbound-conv");
    vault.add_mapping("Alice".to_string(), "§Alice§".to_string(), &PiiType::Custom("T3".to_string()), 3, 1.0f32);
    vault.add_mapping("Acme".to_string(),  "§Acme§".to_string(),  &PiiType::Custom("T3".to_string()), 3, 1.0f32);
    let vault_handle = Arc::new(RwLock::new(vault));

    let mut buf = ReplacementBuffer::new(vault_handle);

    // Feed the LLM response containing §token§ markers in two chunks to exercise
    // the split-across-chunks path.
    let part1 = "My name is §Ali";
    let part2 = "ce§ and I work at §Acme§";

    let out1 = buf.process_delta(part1);
    let out2 = buf.process_delta(part2);
    let remaining = buf.flush_remaining();

    let full = format!("{}{}{}", out1, out2, remaining);

    assert!(full.contains("Alice"),
        "Alice must be restored from §Alice§, got: {full:?}");
    assert!(full.contains("Acme"),
        "Acme must be restored from §Acme§, got: {full:?}");
    assert!(!full.contains("§Alice§"),
        "§Alice§ synthetic must not remain in output: {full:?}");
    assert!(!full.contains("§Acme§"),
        "§Acme§ synthetic must not remain in output: {full:?}");

    let expected = "My name is Alice and I work at Acme";
    assert_eq!(full, expected,
        "full output must equal original text, got: {full:?}");
}

// ── T3 standalone: no PII → system instruction still injected ─────────────────

/// When the SLM returns no § markers (no PII detected), process_request_body_async
/// returns None, but inject_system_instruction still adds SYSTEM_REMINDER to the
/// Anthropic body.
///
/// Spec scenario: "System instruction injected even when no PII found"
#[tokio::test]
async fn t3_standalone_no_pii_system_instruction_still_injected() {
    // Mock SLM returns text with no § markers.
    let port = mock_slm_server("just plain text no pii here").await;
    let endpoint = format!("http://127.0.0.1:{}", port);

    let vault_inner = PiiVault::new("t3-no-pii-conv");
    let vault_handle = Arc::new(RwLock::new(vault_inner));

    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = endpoint;
    cfg.slm.timeout_ms = 2000;

    let pipeline = PiiPipeline::new(&cfg);

    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "just plain text no pii here"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    // With no § tokens the pipeline returns None — body unchanged.
    let result = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &privacyclaw::pii::Locale::EnUs)
        .await;
    assert!(result.is_none(),
        "no § markers → process_request_body_async must return None");

    // But inject_system_instruction must STILL inject SYSTEM_REMINDER into the body.
    let mut value: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    let injected = inject_system_instruction(&mut value, &Provider::Anthropic);
    assert!(injected, "inject_system_instruction must return true for Anthropic");
    let system_field = value["system"].as_str().expect("system field must be a string");
    assert!(system_field.contains(SYSTEM_REMINDER),
        "system field must contain SYSTEM_REMINDER even when no PII found, got: {system_field}");
}

// ── T3 standalone: system instruction injected after PII replacement ──────────

/// When PII is detected and replaced, the forwarded body must have SYSTEM_REMINDER
/// in the system field AND the § synthetic tokens in message content.
///
/// Spec scenario: "System instruction injected after PII replacement"
#[tokio::test]
async fn t3_standalone_system_instruction_present_with_pii_tokens() {
    let port = mock_slm_server("Contact §alice@corp.com§ for help").await;
    let endpoint = format!("http://127.0.0.1:{}", port);

    let vault_handle = Arc::new(RwLock::new(PiiVault::new("t3-sys-instr-conv")));

    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = endpoint;
    cfg.slm.timeout_ms = 2000;

    let pipeline = PiiPipeline::new(&cfg);

    let body = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "Contact alice@corp.com for help"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let (modified_bytes, _detections) = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &privacyclaw::pii::Locale::EnUs)
        .await
        .expect("email PII must trigger T3 replacement");

    // Inject system instruction into the modified body (as the proxy intercept does).
    let mut value: serde_json::Value = serde_json::from_slice(&modified_bytes).unwrap();
    inject_system_instruction(&mut value, &Provider::Anthropic);

    // Both the § token and the system reminder must be present.
    let content = value["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("§alice@corp.com§"),
        "§alice@corp.com§ must be in content, got: {content}");
    assert!(!content.contains("alice@corp.com "),
        "original email must not appear unguarded in content: {content}");

    let system = value["system"].as_str().unwrap();
    assert!(system.contains(SYSTEM_REMINDER),
        "SYSTEM_REMINDER must be in system field, got: {system}");
}

// ── SlmSidecar::detect_and_rewrite: large input max_tokens calculation ─────────

/// For an input longer than (4096 - 128) * 4 = 15872 chars, max_tokens should
/// be capped at 4096 (the upper clamp boundary).
#[tokio::test]
async fn detect_and_rewrite_large_input_max_tokens_capped_at_4096() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<u8>>();
    let response_body = r#"{"choices":[{"message":{"role":"assistant","content":"no pii"}}]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        response_body.len(),
        response_body
    );

    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = vec![0u8; 65536];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let _ = tx.send(buf[..n].to_vec());
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 20000 chars → (20000/4 + 128) = 5128, clamped to 4096.
    let large_text: String = "B".repeat(20000);
    let sidecar = SlmSidecar::new(&format!("http://127.0.0.1:{}", port), 3000);
    let _ = sidecar.detect_and_rewrite(&large_text).await;

    let req_bytes = rx.await.unwrap_or_default();
    let req_str = String::from_utf8_lossy(&req_bytes);
    let body_start = req_str.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let body_json: serde_json::Value = serde_json::from_str(&req_str[body_start..]).unwrap();

    assert_eq!(body_json["max_tokens"].as_u64(), Some(4096),
        "max_tokens must be clamped to 4096 for large inputs, got: {:?}", body_json["max_tokens"]);
}
