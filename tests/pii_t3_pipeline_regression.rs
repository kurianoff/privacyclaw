//! Regression tests for the five T3-first PII pipeline correctness bugs.
//!
//! Each test corresponds to one root cause (A–E) identified in the
//! fix/pii-t3-pipeline-correctness fix.  The tests are written to fail on the
//! unfixed code and pass on the fixed code, serving as permanent guards against
//! regressions.
//!
//! Root causes:
//!   A — T3 original PII text was stored as "T3_0" placeholder instead of real text.
//!   B — conv_id was hardcoded as "conv" instead of using the vault's real conv_id.
//!   C — cascade index maps were skipped when original was already in original_to_synthetic.
//!   D — token_id and display_value were not persisted / not reloaded from storage.
//!   E — stale test used a config that didn't exercise the T2-without-T1 dependency.

// ─── Root Cause A: T3 originals carry real PII text, not placeholder ──────────

/// Regression: before fix A, `PiiDetection.original` was set to "T3_0" (a
/// placeholder) instead of the actual substring captured from the source text.
/// After the fix, the original_text is sliced from the working text before
/// replace_range mutates it; the detection must carry the real PII string.
///
/// This test drives the pipeline with a mock /replace sidecar response and
/// asserts that the detection's `original` field equals the real PII value.
#[tokio::test]
async fn regression_a_t3_original_is_real_pii_not_placeholder() {
    use privacyclaw::pii::vault::VaultRegistry;
    use privacyclaw::pii::{PiiPipeline, Locale};
    use privacyclaw::config::{PiiConfig, PiiTiersConfig};
    use privacyclaw::parser::Provider;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    // The real PII text that appears in the message body.
    let real_email = "alice.smith@corp.com";
    // Start byte offset of the email in the message content field after JSON parse.
    // We build the body first to compute it.
    let msg_content = format!("Send the report to {real_email} immediately.");
    let body = serde_json::json!({
        "model": "claude-3-5-haiku",
        "messages": [{"role": "user", "content": msg_content}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    // The mock sidecar says the email spans bytes 20..40 within msg_content.
    let email_start = msg_content.find(real_email).unwrap();
    let email_end = email_start + real_email.len();
    let replace_body = serde_json::json!({
        "replacements": [{
            "start": email_start,
            "end":   email_end,
            "display_value": "REDACTED_EMAIL",
            "pii_type": "EMAIL"
        }]
    })
    .to_string();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        replace_body.len(),
        replace_body
    );
    let _srv = tokio::spawn(async move {
        for _ in 0..5 {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(resp.as_bytes()).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let mut cfg = PiiConfig::default();
    cfg.tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
    cfg.slm.endpoint = format!("http://127.0.0.1:{port}");
    cfg.slm.timeout_ms = 2000;
    let pipeline = PiiPipeline::new(&cfg);

    let registry = Arc::new(VaultRegistry::new(Duration::from_secs(3600)));
    let vault_handle = registry.get_or_create("conv-regression-a");

    let result = pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &Locale::EnUs)
        .await;

    let (_, detections) = result.expect("T3 pipeline must return Some when sidecar succeeds");
    assert!(!detections.is_empty(), "detections must be non-empty");

    let det = &detections[0];
    // Root Cause A regression: original must NOT be a "T3_N" placeholder string.
    assert_ne!(
        det.original, "T3_0",
        "regression A: original must not be the old T3_0 placeholder"
    );
    assert_ne!(
        det.original, "",
        "regression A: original must not be empty"
    );
    // It must equal the real PII text extracted from the message.
    assert_eq!(
        det.original, real_email,
        "regression A: original must carry the real PII text, got: {:?}",
        det.original
    );
}

// ─── Root Cause A (vault): get_by_token_id returns real PII, not placeholder ──

/// Regression: PiiVault must store the real PII text as the original in all
/// cascade index maps, not a "T3_0" placeholder.  After calling
/// add_mapping_with_token_id with a real original value, get_by_token_id must
/// return that real value.
#[test]
fn regression_a_vault_stores_real_original_not_placeholder() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;

    let conv_id = "conv-a-vault";
    let mut vault = PiiVault::new(conv_id);
    let token_id = generate_token_id(conv_id, 0);
    let real_original = "alice.smith@corp.com";
    let display_val = "REDACTED_EMAIL";

    vault.add_mapping_with_token_id(
        real_original,
        display_val,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );

    let found = vault.get_by_token_id(&token_id);
    assert_ne!(found, Some("T3_0"), "regression A: token_id must not map to placeholder T3_0");
    assert_eq!(
        found,
        Some(real_original),
        "regression A: get_by_token_id must return real PII text, got: {:?}", found
    );
}

// ─── Root Cause B: conv_id propagation to token_id generation ─────────────────

/// Regression: before fix B, all token_ids were generated with the hardcoded
/// conversation_id "conv" instead of the vault's real conversation_id.
/// After the fix, generate_token_id uses the real conv_id from the vault.
///
/// This test verifies that two vaults with different conv_ids produce different
/// token_ids for the same entity index — which would be impossible if conv_id
/// were hardcoded as "conv" for both.
#[test]
fn regression_b_token_id_uses_real_conv_id_not_hardcoded() {
    use privacyclaw::pii::vault::generate_token_id;

    let conv_a = "conv-real-alpha-12345";
    let conv_b = "conv-real-beta-67890";

    // Both generate at entity index 0.
    let tid_a = generate_token_id(conv_a, 0);
    let tid_b = generate_token_id(conv_b, 0);

    // If conv_id were hardcoded as "conv", both would equal generate_token_id("conv", 0).
    let tid_hardcoded = generate_token_id("conv", 0);

    assert_ne!(
        tid_a, tid_hardcoded,
        "regression B: token_id for conv_a must not equal the hardcoded 'conv' token_id"
    );
    assert_ne!(
        tid_b, tid_hardcoded,
        "regression B: token_id for conv_b must not equal the hardcoded 'conv' token_id"
    );
    assert_ne!(
        tid_a, tid_b,
        "regression B: two different conv_ids must yield different token_ids at the same entity index"
    );
}

/// Regression B (integration): the vault's conversation_id() accessor returns
/// the correct conv_id so the pipeline can read it for token_id generation.
#[test]
fn regression_b_vault_conversation_id_accessor_returns_correct_value() {
    use privacyclaw::pii::vault::PiiVault;

    let conv_id = "conv-real-regression-b";
    let vault = PiiVault::new(conv_id);
    assert_eq!(
        vault.conversation_id(),
        conv_id,
        "regression B: conversation_id() must return the conv_id passed to PiiVault::new"
    );

    // Verify it differs from the old hardcoded "conv" value.
    assert_ne!(
        vault.conversation_id(),
        "conv",
        "regression B: conversation_id() must not return hardcoded 'conv'"
    );
}

/// Regression B (end-to-end): token_ids stored in vault after a T3 pipeline run
/// are consistent with generate_token_id(real_conv_id, index), not "conv".
#[tokio::test]
async fn regression_b_pipeline_token_ids_use_real_conv_id() {
    use privacyclaw::pii::vault::{VaultRegistry, generate_token_id};
    use privacyclaw::pii::{PiiPipeline, Locale};
    use privacyclaw::config::{PiiConfig, PiiTiersConfig};
    use privacyclaw::parser::Provider;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let real_conv_id = "conv-regression-b-e2e";
    let msg_content = "Contact john.doe@example.com for details.";
    let email_start = msg_content.find("john.doe@example.com").unwrap();
    let email_end = email_start + "john.doe@example.com".len();

    let replace_body = serde_json::json!({
        "replacements": [{
            "start": email_start,
            "end":   email_end,
            "display_value": "SYNTH_EMAIL",
            "pii_type": "EMAIL"
        }]
    }).to_string();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        replace_body.len(),
        replace_body
    );
    let _srv = tokio::spawn(async move {
        for _ in 0..5 {
            if let Ok((mut s, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(resp.as_bytes()).await;
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(10)).await;

    let mut cfg = PiiConfig::default();
    cfg.tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
    cfg.slm.endpoint = format!("http://127.0.0.1:{port}");
    cfg.slm.timeout_ms = 2000;
    let pipeline = PiiPipeline::new(&cfg);

    let registry = Arc::new(VaultRegistry::new(Duration::from_secs(3600)));
    let vault_handle = registry.get_or_create(real_conv_id);

    let body = serde_json::json!({
        "model": "claude-3-5-haiku",
        "messages": [{"role": "user", "content": msg_content}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    pipeline
        .process_request_body_async(&body_bytes, &vault_handle, Provider::Anthropic, &Locale::EnUs)
        .await
        .expect("pipeline must succeed");

    // The expected token_id uses the REAL conv_id, not "conv".
    let expected_tid = generate_token_id(real_conv_id, 0);
    let wrong_tid = generate_token_id("conv", 0);
    assert_ne!(expected_tid, wrong_tid, "test sanity: expected vs wrong must differ");

    // After the run the vault must have the token_id derived from the real conv_id.
    let vault = vault_handle.read().unwrap();
    let found = vault.get_by_token_id(&expected_tid);
    assert!(
        found.is_some(),
        "regression B: vault must have a mapping for token_id derived from real conv_id {:?}; \
         wrong 'conv'-based token_id = {:?}",
        expected_tid,
        wrong_tid
    );

    // Conversely, the old hardcoded conv_id-based token should NOT be present
    // (unless by coincidence the hash collides, which is astronomically unlikely).
    if expected_tid != wrong_tid {
        assert!(
            vault.get_by_token_id(&wrong_tid).is_none(),
            "regression B: no mapping must exist for the old hardcoded 'conv' token_id"
        );
    }
}

// ─── Root Cause C: cascade index maps populated even after get_or_create ───────

/// Regression: before fix C, add_mapping_with_token_id skipped the cascade index
/// inserts when the original was already present in original_to_synthetic (e.g.
/// because get_or_create had previously inserted it).  After the fix, the three
/// index HashMaps are always populated regardless of whether the core mapping
/// already existed.
///
/// Scenario:
///   1. Call get_or_create first (inserts via add_mapping — no cascade maps).
///   2. Call add_mapping_with_token_id for the same original.
///   3. get_by_token_id must return the original (cascade maps populated).
#[test]
fn regression_c_cascade_maps_populated_when_original_already_in_vault() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;

    let conv_id = "conv-regression-c";
    let mut vault = PiiVault::new(conv_id);
    let original = "carol.white@corp.com";
    let synthetic_first = "synth_first@example.com";

    // Step 1: insert via add_mapping (no cascade maps populated).
    vault.add_mapping(
        original.to_string(),
        synthetic_first.to_string(),
        &PiiType::Email,
        1,
        1.0,
    );
    assert_eq!(vault.mapping_count(), 1, "one mapping after add_mapping");

    // Step 2: call add_mapping_with_token_id for the same original.
    let token_id = generate_token_id(conv_id, 0);
    let display_val = "synth_display@example.com";
    vault.add_mapping_with_token_id(
        original,
        display_val,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );

    // Core mapping count must still be 1 (idempotent).
    assert_eq!(vault.mapping_count(), 1, "mapping count must stay 1 (idempotent)");

    // Step 3: cascade maps MUST be populated — regression C test.
    let found = vault.get_by_token_id(&token_id);
    assert!(
        found.is_some(),
        "regression C: get_by_token_id must return Some after add_mapping_with_token_id, \
         even when original was already in vault via add_mapping"
    );
    assert_eq!(
        found,
        Some(original),
        "regression C: get_by_token_id must return the real original, got: {:?}",
        found
    );

    // Display value cascade must also be populated.
    let by_display = vault.get_by_display_value(display_val);
    assert_eq!(
        by_display,
        Some(original),
        "regression C: get_by_display_value must return original, got: {:?}",
        by_display
    );
}

/// Regression C: calling add_mapping_with_token_id for a brand-new original
/// (not previously in the vault) also populates all three cascade maps.
#[test]
fn regression_c_cascade_maps_populated_for_new_original() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id, xml_token};
    use privacyclaw::pii::PiiType;

    let conv_id = "conv-c-new";
    let mut vault = PiiVault::new(conv_id);
    let original = "dave.jones@company.com";
    let display_val = "DAVE_SYNTH@example.com";
    let token_id = generate_token_id(conv_id, 0);

    vault.add_mapping_with_token_id(
        original,
        display_val,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );

    // L1 cascade: full_token_to_original
    let full_tok = xml_token(&token_id, display_val);
    assert_eq!(
        vault.full_token_to_original.get(&full_tok).map(|s| s.as_str()),
        Some(original),
        "regression C: full_token_to_original must be populated"
    );

    // L2 cascade: token_id_to_original
    assert_eq!(
        vault.get_by_token_id(&token_id),
        Some(original),
        "regression C: token_id_to_original must be populated"
    );

    // L3 cascade: display_value_to_original
    assert_eq!(
        vault.get_by_display_value(display_val),
        Some(original),
        "regression C: display_value_to_original must be populated"
    );
}

// ─── Root Cause D: token_id/display_value persist through save/load ────────────

/// Regression: before fix D, StoredVaultRecord lacked token_id/display_value
/// fields and save_vault used a 5-tuple.  After the fix, both fields are
/// optional in StoredVaultRecord (backward-compatible) and save_vault accepts
/// a 7-tuple with token_id and display_value.
///
/// Test: save a vault mapping with a non-empty token_id and display_value,
/// reload via load_vault, confirm get_by_token_id still works on the
/// reconstructed vault.
#[test]
fn regression_d_token_id_persists_through_save_and_load() {
    use privacyclaw::pii::vault::{PiiVault, VaultRecord, generate_token_id};
    use privacyclaw::pii::PiiType;
    use privacyclaw::storage::{Conversation, Store};

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let conv_id = "conv-regression-d";
    store.insert_conversation(&Conversation {
        id: conv_id.to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        provider: "anthropic".to_string(),
        model: Some("claude-3".to_string()),
        client_hint: Some("fp-d".to_string()),
    }).unwrap();

    let original = "eve.miller@corp.com";
    let display_val = "SYNTH_MILLER@example.com";
    let token_id = generate_token_id(conv_id, 0);

    // Build a vault and take a snapshot.
    let mut vault = PiiVault::new(conv_id);
    vault.add_mapping_with_token_id(
        original,
        display_val,
        &token_id,
        &PiiType::Email,
        3,
        1.0,
    );

    // Persist via to_records() + save_vault — the 7-tuple path.
    // Use a fixed seed (rng_seed is pub(crate) and not accessible from integration tests;
    // the seed value is irrelevant for the cascade-map assertion below).
    let rng_seed: u64 = 42;
    let records: Vec<(String, String, String, u8, f32, String, String)> = vault
        .to_records()
        .into_iter()
        .map(|r| (r.original, r.synthetic, r.pii_type.label().to_string(), r.tier, r.confidence, r.token_id, r.display_value))
        .collect();
    store.save_vault(conv_id, rng_seed, &records).unwrap();

    // Reload from storage.
    let (seed, stored_records) = store.load_vault(conv_id).unwrap().expect("vault must exist after save");

    // Reconstruct vault_records — mirrors the VaultRegistry::load_or_create path.
    let vault_records: Vec<VaultRecord> = stored_records.into_iter().map(|r| VaultRecord {
        original: r.original,
        synthetic: r.synthetic,
        pii_type: PiiType::Custom(r.pii_type),
        tier: r.tier.unwrap_or(0),
        confidence: r.confidence.unwrap_or(0.0),
        token_id: r.token_id.unwrap_or_default(),
        display_value: r.display_value.unwrap_or_default(),
    }).collect();

    // Regression D assertion: token_id and display_value must have survived round-trip.
    let reconstructed = PiiVault::from_records(conv_id, seed, vault_records);
    let found = reconstructed.get_by_token_id(&token_id);
    assert!(
        found.is_some(),
        "regression D: get_by_token_id must work after save+load round-trip. \
         token_id={:?} was not found in reloaded vault",
        token_id
    );
    assert_eq!(
        found,
        Some(original),
        "regression D: reloaded vault must map token_id to original PII text, got: {:?}",
        found
    );
}

/// Regression D: StoredVaultRecord with token_id and display_value serializes
/// and deserializes correctly, including skip_serializing_if semantics.
#[test]
fn regression_d_stored_vault_record_token_id_roundtrip() {
    use privacyclaw::storage::StoredVaultRecord;

    // With token_id and display_value populated.
    let rec = StoredVaultRecord {
        original: "frank@acme.com".to_string(),
        synthetic: "SYNTH_FRANK@example.com".to_string(),
        pii_type: "email".to_string(),
        tier: Some(3),
        confidence: Some(1.0),
        token_id: Some("abc12345".to_string()),
        display_value: Some("SYNTH_FRANK@example.com".to_string()),
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert!(json.contains("\"token_id\""), "regression D: token_id must appear in JSON: {json}");
    assert!(json.contains("\"display_value\""), "regression D: display_value must appear in JSON: {json}");

    let rec2: StoredVaultRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec2.token_id, Some("abc12345".to_string()));
    assert_eq!(rec2.display_value, Some("SYNTH_FRANK@example.com".to_string()));
}

/// Regression D: StoredVaultRecord with None token_id/display_value omits
/// those fields from JSON (backward-compat: old readers don't see unknown fields).
#[test]
fn regression_d_stored_vault_record_none_token_id_omitted() {
    use privacyclaw::storage::StoredVaultRecord;

    let rec = StoredVaultRecord {
        original: "grace@acme.com".to_string(),
        synthetic: "SYNTH_GRACE@example.com".to_string(),
        pii_type: "email".to_string(),
        tier: None,
        confidence: None,
        token_id: None,
        display_value: None,
    };
    let json = serde_json::to_string(&rec).unwrap();
    assert!(
        !json.contains("\"token_id\""),
        "regression D: None token_id must be omitted from JSON: {json}"
    );
    assert!(
        !json.contains("\"display_value\""),
        "regression D: None display_value must be omitted from JSON: {json}"
    );
}

/// Regression D: legacy StoredVaultRecord JSON (no token_id/display_value fields)
/// deserializes successfully with those fields as None.
#[test]
fn regression_d_legacy_stored_vault_record_without_token_id_deserializes() {
    use privacyclaw::storage::StoredVaultRecord;

    // Simulates an NDJSON line written by the old 5-tuple code.
    let legacy_json = r#"{"original":"henry@acme.com","synthetic":"SYNTH@example.com","pii_type":"email","tier":1,"confidence":0.99}"#;
    let rec: StoredVaultRecord = serde_json::from_str(legacy_json)
        .expect("legacy StoredVaultRecord (no token_id/display_value) must deserialize OK");
    assert!(rec.token_id.is_none(), "regression D: missing token_id field must deserialize as None");
    assert!(rec.display_value.is_none(), "regression D: missing display_value field must deserialize as None");
}

// ─── Root Cause E: ConfigManager rejects T2-without-T1 dependency ─────────────

/// Regression E: the original test `patch_tier3_without_tier2_returns_error` used
/// an initial config of {regex:false, ner:false, slm:false} and patched {slm:true},
/// which was actually a valid T3-standalone config (regex:false, ner:false, slm:true).
/// After fix E the initial config is {regex:false, ner:true, slm:false} so that
/// patching {slm:true} yields {regex:false, ner:true, slm:true}, which is invalid
/// (T2 active without T1).
///
/// This test confirms: ConfigManager rejects {regex:false, ner:true, slm:true}.
#[tokio::test]
async fn regression_e_t2_without_t1_rejected_by_config_manager() {
    use privacyclaw::config::{Config, ConfigManager};

    let mut cfg = Config::default();
    // Start with T2 enabled but T1 disabled — this is the pre-condition.
    cfg.pii.tiers.regex = false;
    cfg.pii.tiers.ner = true;
    cfg.pii.tiers.slm = false;
    let mgr = ConfigManager::new(cfg, None);

    // Patching slm=true yields {regex:false, ner:true, slm:true} — T2 without T1 → invalid.
    let patch = serde_json::json!({ "pii": { "tiers": { "slm": true } } });
    let result = mgr.patch(patch).await;
    assert!(
        result.is_err(),
        "regression E: enabling T3 on top of T2-without-T1 must be rejected, got Ok"
    );
    let err_msg = result.unwrap_err().to_string().to_lowercase();
    assert!(
        err_msg.contains("tier 1") || err_msg.contains("tier1")
            || err_msg.contains("regex") || err_msg.contains("requires"),
        "regression E: error must mention T1/T2 dependency: {err_msg}"
    );
}

/// Regression E: contrast test — {regex:false, ner:false, slm:true} (T3 standalone)
/// is a VALID config and must be accepted by ConfigManager.
#[tokio::test]
async fn regression_e_t3_standalone_is_valid_config() {
    use privacyclaw::config::{Config, ConfigManager};

    let cfg = Config::default();
    let mgr = ConfigManager::new(cfg, None);

    // T3 standalone: only slm enabled, no T1 or T2.
    let patch = serde_json::json!({
        "pii": { "tiers": { "slm": true, "regex": false, "ner": false } }
    });
    let result = mgr.patch(patch).await;
    assert!(
        result.is_ok(),
        "regression E: T3-standalone config (slm=true, regex=false, ner=false) must be accepted, \
         got error: {:?}",
        result.err()
    );
}

// ─── Adjacent-behaviour guard: cascade maps work after persistence roundtrip ───

/// Guard: when load_or_create reconstructs a vault from storage that includes
/// non-empty token_id/display_value, all three cascade maps are populated via
/// from_records so that get_by_token_id works without any add_mapping call.
#[test]
fn guard_cascade_maps_populated_after_from_records_with_token_id() {
    use privacyclaw::pii::vault::{PiiVault, VaultRecord, generate_token_id, xml_token};
    use privacyclaw::pii::PiiType;

    let conv_id = "conv-guard-cascade";
    let token_id = generate_token_id(conv_id, 0);
    let display_val = "SYNTH_IAN@example.com";
    let original = "ian.moore@corp.com";

    // Simulate what VaultRegistry::load_or_create does: build VaultRecords from StoredVaultRecord.
    let records = vec![VaultRecord {
        original: original.to_string(),
        synthetic: display_val.to_string(),
        pii_type: PiiType::Email,
        tier: 3,
        confidence: 1.0,
        token_id: token_id.clone(),
        display_value: display_val.to_string(),
    }];
    let vault = PiiVault::from_records(conv_id, 0, records);

    // All three cascade maps must be populated.
    assert_eq!(vault.get_by_token_id(&token_id), Some(original), "L2 cascade must work after from_records");
    assert_eq!(vault.get_by_display_value(display_val), Some(original), "L3 cascade must work after from_records");

    let full_token = xml_token(&token_id, display_val);
    assert_eq!(
        vault.full_token_to_original.get(&full_token).map(|s| s.as_str()),
        Some(original),
        "L1 cascade must work after from_records"
    );
}

/// Guard: cascade maps are independent per vault — two vaults with the same
/// entity index but different conv_ids have non-overlapping token_ids.
#[test]
fn guard_cascade_maps_are_per_vault_independent() {
    use privacyclaw::pii::vault::{PiiVault, generate_token_id};
    use privacyclaw::pii::PiiType;

    let conv_a = "conv-guard-indep-a";
    let conv_b = "conv-guard-indep-b";
    let mut vault_a = PiiVault::new(conv_a);
    let mut vault_b = PiiVault::new(conv_b);

    let tid_a = generate_token_id(conv_a, 0);
    let tid_b = generate_token_id(conv_b, 0);

    vault_a.add_mapping_with_token_id("alice@corp.com", "synth_a@example.com", &tid_a, &PiiType::Email, 3, 1.0);
    vault_b.add_mapping_with_token_id("bob@corp.com",   "synth_b@example.com", &tid_b, &PiiType::Email, 3, 1.0);

    // Each vault knows only its own token_id.
    assert_eq!(vault_a.get_by_token_id(&tid_a), Some("alice@corp.com"));
    assert_eq!(vault_b.get_by_token_id(&tid_b), Some("bob@corp.com"));

    // Cross-lookup: vault A must not know vault B's token and vice-versa
    // (unless, by extreme coincidence, the hashes collide — acceptable to skip).
    if tid_a != tid_b {
        assert!(vault_a.get_by_token_id(&tid_b).is_none(), "vault A must not contain vault B's token_id");
        assert!(vault_b.get_by_token_id(&tid_a).is_none(), "vault B must not contain vault A's token_id");
    }
}
