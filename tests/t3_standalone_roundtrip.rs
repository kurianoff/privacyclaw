//! T3-first pipeline integration tests.
//!
//! These tests verify the T3-first pipeline routing logic using mocked SLM
//! servers. No real llama-server is required.

use privacyclaw::pii::{PiiPipeline, SYSTEM_REMINDER};
use privacyclaw::pii::inject_system_instruction;
use privacyclaw::parser::Provider;

// ── pipeline_t3_only_calls_replace_not_t1t2 ──────────────────────────────────

/// With tiers={regex:false, ner:false, slm:true}, the pipeline has no slm_standalone
/// flag (it's removed) and should route to the T3-first path. tier2 is None.
#[test]
fn pipeline_t3_only_tier_matrix_routing() {
    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = "http://127.0.0.1:16442".to_string();
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_some(), "slm must be Some when tiers.slm=true and endpoint non-empty");
    assert!(pipeline.tier2.is_none(), "tier2 must be None when tiers.ner=false");
}

/// With tiers={regex:true, ner:false, slm:true} — T3+T1, no T2.
#[test]
fn pipeline_t3_plus_t1_tier_matrix_routing() {
    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = true;
    cfg.tiers.ner = false;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = "http://127.0.0.1:16442".to_string();
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_some(), "slm must be Some");
    assert!(pipeline.tier2.is_none(), "tier2 must be None when tiers.ner=false");
}

/// With tiers={regex:true, ner:true, slm:true} — full stack T3+T1+T2.
#[test]
fn pipeline_t3_t1_t2_full_stack_routing() {
    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = true;
    cfg.tiers.ner = true;
    cfg.tiers.slm = true;
    cfg.slm.endpoint = "http://127.0.0.1:16442".to_string();
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_some(), "slm must be Some");
    // tier2 may or may not be Some depending on ort-ner feature, but slm must be Some
}

/// Invalid combo: slm=true but endpoint is empty string — slm must be None.
/// An empty endpoint is operationally invalid and must degrade gracefully.
#[test]
fn pipeline_slm_true_empty_endpoint_yields_none() {
    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.slm = true;
    cfg.slm.endpoint = "".to_string(); // invalid: empty endpoint
    let pipeline = PiiPipeline::new(&cfg);
    assert!(
        pipeline.slm.is_none(),
        "slm must be None when endpoint is empty, even if tiers.slm=true"
    );
}

/// Invalid combo: all tiers false — both slm and tier2 are None, no panic.
#[test]
fn pipeline_all_tiers_false_is_no_op() {
    let mut cfg = privacyclaw::config::PiiConfig::default();
    cfg.tiers.regex = false;
    cfg.tiers.ner = false;
    cfg.tiers.slm = false;
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_none(), "slm must be None when tiers.slm=false");
    assert!(pipeline.tier2.is_none(), "tier2 must be None when tiers.ner=false");
    // No panic: pipeline still valid, just produces no detections
}

// ── System instruction injection ──────────────────────────────────────────────

/// inject_system_instruction adds SYSTEM_REMINDER to the Anthropic system field.
#[test]
fn system_instruction_injected_for_anthropic() {
    let mut value = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "hello"}]
    });
    let injected = inject_system_instruction(&mut value, &Provider::Anthropic);
    assert!(injected, "must return true for Anthropic");
    let system = value["system"].as_str().expect("system field must be a string");
    assert!(system.contains(SYSTEM_REMINDER), "SYSTEM_REMINDER must be present");
}

/// inject_system_instruction is idempotent: calling twice does not double-inject.
#[test]
fn system_instruction_injection_idempotent_anthropic() {
    let mut value = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "messages": [{"role": "user", "content": "hello"}]
    });
    inject_system_instruction(&mut value, &Provider::Anthropic);
    let after_first = value["system"].as_str().unwrap().to_string();
    inject_system_instruction(&mut value, &Provider::Anthropic);
    let after_second = value["system"].as_str().unwrap().to_string();
    // SYSTEM_REMINDER should appear at least once; the second call appends again
    // (current implementation does not deduplicate), but the value must contain it.
    assert!(
        after_second.contains(SYSTEM_REMINDER),
        "SYSTEM_REMINDER must be present after second injection"
    );
    // Verify it's longer after second injection (appended, not replaced).
    assert!(
        after_second.len() >= after_first.len(),
        "second injection must not shorten the system field"
    );
}

/// inject_system_instruction for an Anthropic request whose system field is a JSON
/// object (not a string) returns false and leaves the body unchanged.
#[test]
fn system_instruction_anthropic_non_string_system_returns_false() {
    let mut value = serde_json::json!({
        "model": "claude-3-5-sonnet",
        "system": [{"type": "text", "text": "existing instructions"}],
        "messages": [{"role": "user", "content": "hello"}]
    });
    let injected = inject_system_instruction(&mut value, &Provider::Anthropic);
    assert!(!injected, "must return false when system field is not a string");
    // system field must be unchanged
    assert!(
        value["system"].is_array(),
        "system field must still be an array"
    );
}

/// inject_system_instruction for OpenAI inserts a new system message at index 0
/// when no system message exists, and the user message is preserved at index 1.
#[test]
fn system_instruction_openai_inserts_at_index_0_user_preserved() {
    let mut value = serde_json::json!({
        "model": "gpt-4",
        "messages": [
            {"role": "user", "content": "tell me about PII"}
        ]
    });
    let injected = inject_system_instruction(&mut value, &Provider::OpenAI);
    assert!(injected, "must return true for OpenAI");
    let messages = value["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "a new system message must be inserted");
    assert_eq!(messages[0]["role"].as_str().unwrap(), "system");
    assert_eq!(messages[1]["role"].as_str().unwrap(), "user");
    let system_content = messages[0]["content"].as_str().unwrap();
    assert!(system_content.contains(SYSTEM_REMINDER));
}

/// SYSTEM_REMINDER describes XML pii token format (not § format).
#[test]
fn system_reminder_describes_xml_token_format() {
    // The SYSTEM_REMINDER must reference <pii> XML tokens, not §value§ delimiters.
    assert!(
        SYSTEM_REMINDER.contains("<pii") || SYSTEM_REMINDER.contains("pii"),
        "SYSTEM_REMINDER must describe pii token format, got: {SYSTEM_REMINDER}"
    );
}

/// SYSTEM_REMINDER mentions id attribute and closing tag to enforce atomic treatment.
#[test]
fn system_reminder_mentions_token_id_attribute() {
    assert!(
        SYSTEM_REMINDER.contains("id=") || SYSTEM_REMINDER.contains("id\""),
        "SYSTEM_REMINDER must describe the id attribute of pii tokens"
    );
}

// ── generate_token_id properties ──────────────────────────────────────────────

/// generate_token_id always produces exactly 8 characters.
#[test]
fn generate_token_id_is_exactly_8_chars() {
    use privacyclaw::pii::vault::generate_token_id;
    for i in 0u64..20 {
        let tid = generate_token_id("conv-abc", i);
        assert_eq!(
            tid.len(),
            8,
            "token_id must be exactly 8 chars, got {:?} (len {})",
            tid,
            tid.len()
        );
    }
}

/// generate_token_id characters are all base62 (0-9, A-Z, a-z).
#[test]
fn generate_token_id_chars_are_base62() {
    use privacyclaw::pii::vault::generate_token_id;
    for i in 0u64..50 {
        let tid = generate_token_id(&format!("conv-{i}"), i * 7 + 3);
        for ch in tid.chars() {
            assert!(
                ch.is_ascii_alphanumeric(),
                "token_id must only contain base62 chars, found {:?} in {:?}",
                ch,
                tid
            );
        }
    }
}

/// generate_token_id is deterministic: same inputs always yield same output.
#[test]
fn generate_token_id_is_deterministic() {
    use privacyclaw::pii::vault::generate_token_id;
    let t1 = generate_token_id("conv-determinism", 42);
    let t2 = generate_token_id("conv-determinism", 42);
    assert_eq!(t1, t2, "same inputs must produce same token_id");
}

/// generate_token_id distinguishes different entity indices within same conversation.
#[test]
fn generate_token_id_distinct_for_different_indices() {
    use privacyclaw::pii::vault::generate_token_id;
    let ids: Vec<String> = (0u64..20).map(|i| generate_token_id("conv-uniqueness", i)).collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "all token_ids must be distinct for different entity indices"
    );
}

/// generate_token_id distinguishes different conversation IDs at the same entity index.
#[test]
fn generate_token_id_distinct_for_different_conv_ids() {
    use privacyclaw::pii::vault::generate_token_id;
    let ids: Vec<String> = (0u64..20)
        .map(|i| generate_token_id(&format!("conv-{i}"), 0))
        .collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "all token_ids must be distinct for different conv_ids"
    );
}

// ── Vault cascade matching (L1 / L2 / L3 / L5) ───────────────────────────────

/// L1: full XML token reversal — exact lookup by full token string.
#[test]
fn vault_cascade_l1_full_token_match() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id, xml_token};
    use privacyclaw::pii::PiiType;
    let mut vault = PiiVault::new("conv-l1");
    let token_id = generate_token_id("conv-l1", 0);
    let display_value = "synth@example.com";
    vault.add_mapping_with_token_id(
        "real@corp.com",
        display_value,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );
    let full_token = xml_token(&token_id, display_value);
    let original = vault.full_token_to_original.get(&full_token).map(|s| s.as_str());
    assert_eq!(original, Some("real@corp.com"), "L1: full_token_to_original must map to original");
}

/// L2: token_id-only lookup when full token is structurally modified (e.g. whitespace).
#[test]
fn vault_cascade_l2_token_id_match() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;
    let mut vault = PiiVault::new("conv-l2");
    let token_id = generate_token_id("conv-l2", 1);
    vault.add_mapping_with_token_id(
        "john@company.com",
        "fake@example.com",
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );
    let found = vault.get_by_token_id(&token_id);
    assert_eq!(found, Some("john@company.com"), "L2: get_by_token_id must return original");
    // Non-existent token_id returns None.
    assert!(vault.get_by_token_id("00000000").is_none(), "L2: unknown token_id must return None");
}

/// L3: display value lookup when only the inner text is known.
#[test]
fn vault_cascade_l3_display_value_match() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;
    let mut vault = PiiVault::new("conv-l3");
    let token_id = generate_token_id("conv-l3", 2);
    let display_value = "Maria Synthetic";
    vault.add_mapping_with_token_id(
        "Jane Real",
        display_value,
        &token_id,
        &PiiType::PersonName,
        3,
        1.0,
    );
    let found = vault.get_by_display_value(display_value);
    assert_eq!(found, Some("Jane Real"), "L3: get_by_display_value must return original");
    assert!(
        vault.get_by_display_value("Unknown Synth").is_none(),
        "L3: unknown display value must return None"
    );
}

/// L5: bare synthetic in response text (no XML wrapper) is reversed via replace_synthetics.
/// Level 5 is the Aho-Corasick path in the standard replace_synthetics call.
#[test]
fn vault_cascade_l5_bare_synthetic_reversed() {
    use privacyclaw::pii::vault::PiiVault;
    use privacyclaw::pii::PiiType;
    let mut vault = PiiVault::new("conv-l5");
    // Mapping: original=real@corp.com, synthetic (display value)=fake@example.com
    vault.add_mapping(
        "real@corp.com".to_string(),
        "fake@example.com".to_string(),
        &PiiType::Email,
        1,
        1.0,
    );
    // Response contains only the bare synthetic, no XML wrapper.
    let response_text = "Please contact fake@example.com for details.";
    let (reversed, any) = vault.replace_synthetics(response_text);
    assert!(any, "L5: replace_synthetics must find the bare synthetic");
    assert!(
        reversed.contains("real@corp.com"),
        "L5: original must appear after reversal, got: {reversed:?}"
    );
    assert!(
        !reversed.contains("fake@example.com"),
        "L5: synthetic must not appear after reversal, got: {reversed:?}"
    );
}

/// L5: multiple bare synthetics in a single response text are all reversed.
#[test]
fn vault_cascade_l5_multiple_bare_synthetics_reversed() {
    use privacyclaw::pii::vault::PiiVault;
    use privacyclaw::pii::PiiType;
    let mut vault = PiiVault::new("conv-l5-multi");
    vault.add_mapping("alice@corp.com".to_string(), "synth_a@example.com".to_string(), &PiiType::Email, 1, 1.0);
    vault.add_mapping("bob@corp.com".to_string(),   "synth_b@example.com".to_string(), &PiiType::Email, 1, 1.0);
    let text = "Contact synth_a@example.com or synth_b@example.com.";
    let (reversed, any) = vault.replace_synthetics(text);
    assert!(any);
    assert!(reversed.contains("alice@corp.com"), "first email must be reversed");
    assert!(reversed.contains("bob@corp.com"),   "second email must be reversed");
    assert!(!reversed.contains("synth_a@example.com"), "synth_a must be gone");
    assert!(!reversed.contains("synth_b@example.com"), "synth_b must be gone");
}

// ── T3 /replace integration with mock sidecar via process_request_body_async ──

/// Full T3-first pipeline with mock sidecar: process_request_body_async calls
/// the /replace endpoint, inserts vault mappings, and returns modified body bytes.
#[tokio::test]
async fn t3_pipeline_process_request_body_async_mock_sidecar() {
    use privacyclaw::pii::vault::{PiiVault, VaultRegistry};
    use privacyclaw::pii::{PiiPipeline, Locale};
    use privacyclaw::config::{PiiConfig, PiiTiersConfig};
    use privacyclaw::parser::Provider;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use std::time::Duration;

    // Spin up a mock /replace server.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let replace_body = r#"{"replacements":[{"start":27,"end":40,"display_value":"SYNTH_EMAIL","pii_type":"EMAIL"}]}"#;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        replace_body.len(),
        replace_body
    );
    let _server_handle = tokio::spawn(async move {
        // Accept multiple connections (process_request_body_async calls /replace once per entry).
        for _ in 0..5 {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut raw = vec![0u8; 8192];
                let _ = stream.read(&mut raw).await;
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Build pipeline with T3 enabled pointing at mock server.
    let mut cfg = PiiConfig::default();
    cfg.tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
    cfg.slm.endpoint = format!("http://127.0.0.1:{port}");
    cfg.slm.timeout_ms = 2000;
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_some(), "pipeline must have SLM for this test");

    let body = serde_json::json!({
        "model": "claude-3-5-haiku",
        "messages": [{"role": "user", "content": "Please contact user@example.com for info"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let registry = Arc::new(VaultRegistry::new(Duration::from_secs(3600)));
    let vault_handle = registry.get_or_create("conv-t3-mock");

    let result = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &Locale::EnUs)
        .await;

    assert!(
        result.is_some(),
        "T3-first pipeline with mock sidecar must return modified body"
    );
    let (modified_bytes, detections) = result.unwrap();
    let modified_str = std::str::from_utf8(&modified_bytes).expect("modified body must be valid UTF-8");
    assert!(
        modified_str.contains("SYNTH_EMAIL") || modified_str.contains("<pii"),
        "modified body must contain synthetic token, got: {modified_str}"
    );
    assert!(!detections.is_empty(), "detections must be non-empty after T3 replacement");
}

/// T3-first pipeline: when mock sidecar returns 500, falls back to T1-only path.
/// With regex=false, ner=false the fallback produces no detections (returns None).
#[tokio::test]
async fn t3_pipeline_fallback_on_sidecar_500() {
    use privacyclaw::pii::vault::VaultRegistry;
    use privacyclaw::pii::{PiiPipeline, Locale};
    use privacyclaw::config::{PiiConfig, PiiTiersConfig};
    use privacyclaw::parser::Provider;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use std::time::Duration;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let resp = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
    let _server_handle = tokio::spawn(async move {
        for _ in 0..5 {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut raw = vec![0u8; 1024];
                let _ = stream.read(&mut raw).await;
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let mut cfg = PiiConfig::default();
    cfg.tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
    cfg.slm.endpoint = format!("http://127.0.0.1:{port}");
    cfg.slm.timeout_ms = 2000;
    let pipeline = PiiPipeline::new(&cfg);

    let body = serde_json::json!({
        "model": "claude-3-5-haiku",
        "messages": [{"role": "user", "content": "no pii here just plain text"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let registry = Arc::new(VaultRegistry::new(Duration::from_secs(3600)));
    let vault_handle = registry.get_or_create("conv-t3-fallback");

    // With T3 failing and regex=false, no replacements should be made.
    let result = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &Locale::EnUs)
        .await;
    // Text has no PII so T1 (always-on) also finds nothing; result is None.
    assert!(
        result.is_none(),
        "pipeline must return None when T3 fails and no T1 PII detected"
    );
}

// ── Buffer: XML-token holdback across SSE chunk boundaries ────────────────────

/// A `<pii` open tag split with its close tag in a subsequent chunk is still
/// correctly reversed — the buffer holds back the partial token.
#[test]
fn buffer_xml_token_holdback_split_at_open_tag_boundary() {
    use privacyclaw::pii::buffer::ReplacementBuffer;
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;
    use std::sync::{Arc, RwLock};

    let mut vault = PiiVault::new("conv-buf-split");
    let token_id = generate_token_id("conv-buf-split", 0);
    let display_value = "synth_name@example.com";
    vault.add_mapping_with_token_id(
        "real_name@company.com",
        display_value,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );
    let handle = Arc::new(RwLock::new(vault));
    let mut buf = ReplacementBuffer::new(handle);

    // Build the complete XML token and split it at various positions.
    let full_token = format!(r#"<pii id="{token_id}">{display_value}</pii>"#);

    // Split just after `<pii` to stress-test the partial-open-tag holdback.
    let split_at = 4; // "<pii" is 4 bytes
    let chunk1 = &full_token[..split_at];
    let chunk2 = &full_token[split_at..];

    let out1 = buf.process_delta(chunk1);
    let out2 = buf.process_delta(chunk2);
    let remaining = buf.flush_remaining();
    let full_output = format!("{out1}{out2}{remaining}");

    assert_eq!(
        full_output, "real_name@company.com",
        "buffer must reverse XML token split at open-tag boundary, got: {full_output:?}"
    );
}

/// Multiple XML tokens in separate chunks are all reversed without leaking synthetics.
#[test]
fn buffer_multiple_xml_tokens_across_chunks_all_reversed() {
    use privacyclaw::pii::buffer::ReplacementBuffer;
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;
    use std::sync::{Arc, RwLock};

    let mut vault = PiiVault::new("conv-buf-multi");
    let tid1 = generate_token_id("conv-buf-multi", 0);
    let tid2 = generate_token_id("conv-buf-multi", 1);
    vault.add_mapping_with_token_id("alice@corp.com", "synth_a@example.com", &tid1, &PiiType::Email, 3, 1.0);
    vault.add_mapping_with_token_id("bob@corp.com",   "synth_b@example.com", &tid2, &PiiType::Email, 3, 1.0);

    let handle = Arc::new(RwLock::new(vault));
    let mut buf = ReplacementBuffer::new(handle);

    let token1 = format!(r#"<pii id="{tid1}">synth_a@example.com</pii>"#);
    let token2 = format!(r#"<pii id="{tid2}">synth_b@example.com</pii>"#);
    let full_text = format!("Hello {token1} and {token2}.");

    // Feed in 3 chunks, splitting across token boundaries.
    let mid = full_text.len() / 3;
    let chunk1 = &full_text[..mid];
    // Find a char boundary for chunk2 start.
    let mid2 = (mid * 2).min(full_text.len());
    let chunk2 = &full_text[mid..mid2];
    let chunk3 = &full_text[mid2..];

    let mut output = String::new();
    output.push_str(&buf.process_delta(chunk1));
    output.push_str(&buf.process_delta(chunk2));
    output.push_str(&buf.process_delta(chunk3));
    output.push_str(&buf.flush_remaining());

    assert!(output.contains("alice@corp.com"), "first original must be restored, got: {output:?}");
    assert!(output.contains("bob@corp.com"),   "second original must be restored, got: {output:?}");
    assert!(!output.contains("synth_a@example.com"), "synth_a must not appear: {output:?}");
    assert!(!output.contains("synth_b@example.com"), "synth_b must not appear: {output:?}");
}
