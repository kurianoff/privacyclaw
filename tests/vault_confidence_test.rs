/// Integration tests for vault confidence (tasks 2.10 and 11.4).
///
/// Covers:
///   - confidence stored in parallel vec and returned by quints()
///   - quints() returns correct (orig, synth, type, tier, confidence) tuples
///   - from_records handles None confidence → maps to 0.0
///   - confidence round-trips through save_vault → load_vault → from_records
///   - legacy StoredVaultRecord without confidence field → deserialises as None
///   - zero confidence (legacy sentinel) survives round-trip
///   - multiple confidences in one vault, each preserved independently
use privacyclaw::pii::vault::{PiiVault, VaultRecord};
use privacyclaw::pii::PiiType;
use privacyclaw::storage::{Conversation, Store, StoredVaultRecord};

fn make_store_with_conv(id: &str) -> (Store, tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let conv = Conversation {
        id: id.to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        provider: "anthropic".to_string(),
        model: Some("claude-3".to_string()),
        client_hint: Some(format!("fp-{id}")),
    };
    store.insert_conversation(&conv).unwrap();
    (store, dir, id.to_string())
}

// ── 1. quints() returns correct 5-tuples ──────────────────────────────────────

#[test]
fn quints_returns_correct_tuples() {
    let mut vault = PiiVault::new("conv-quints");
    vault.add_mapping("alice@acme.com".to_string(), "bob@example.com".to_string(), &PiiType::Email, 1, 0.99);
    vault.add_mapping("123-45-6789".to_string(),    "999-00-0001".to_string(),     &PiiType::Ssn,   2, 0.75);

    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 2);

    let email = quints.iter().find(|q| q.0 == "alice@acme.com").expect("email quint missing");
    assert_eq!(email.1, "bob@example.com");
    assert_eq!(email.2, "EMAIL");
    assert_eq!(email.3, 1);
    assert!((email.4 - 0.99).abs() < 1e-5, "email confidence mismatch: {}", email.4);

    let ssn = quints.iter().find(|q| q.0 == "123-45-6789").expect("ssn quint missing");
    assert_eq!(ssn.3, 2);
    assert!((ssn.4 - 0.75).abs() < 1e-4, "ssn confidence mismatch: {}", ssn.4);
}

// ── 2. confidences stored in parallel vec — mapping_count matches ─────────────

#[test]
fn confidence_vec_length_matches_mapping_count() {
    let mut vault = PiiVault::new("conv-parallel-len");
    for i in 0..10u32 {
        let conf = i as f32 / 10.0;
        vault.add_mapping(
            format!("orig-{i}"),
            format!("synth-{i}"),
            &PiiType::Custom("test".to_string()),
            1,
            conf,
        );
    }
    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), vault.mapping_count());
    assert_eq!(quints.len(), 10);

    // Verify each confidence is distinct and within range.
    for (i, q) in quints.iter().enumerate() {
        let orig_idx = q.0.strip_prefix("orig-")
            .and_then(|s| s.parse::<usize>().ok())
            .expect("unexpected orig key");
        let expected = orig_idx as f32 / 10.0;
        assert!(
            (q.4 - expected).abs() < 1e-5,
            "confidence mismatch at index {i}: got {}, expected {}", q.4, expected
        );
    }
}

// ── 3. from_records with explicit confidence ───────────────────────────────────

#[test]
fn from_records_preserves_confidence() {
    let records = vec![
        VaultRecord { original: "a@a.com".to_string(), synthetic: "x@x.com".to_string(), pii_type: PiiType::Email, tier: 1, confidence: 0.95 },
        VaultRecord { original: "b@b.com".to_string(), synthetic: "y@y.com".to_string(), pii_type: PiiType::Email, tier: 2, confidence: 0.60 },
    ];
    let vault = PiiVault::from_records("test-conv", 12345, records);

    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 2);

    let qa = quints.iter().find(|q| q.0 == "a@a.com").expect("a@a.com missing");
    assert!((qa.4 - 0.95).abs() < 1e-5);

    let qb = quints.iter().find(|q| q.0 == "b@b.com").expect("b@b.com missing");
    assert!((qb.4 - 0.60).abs() < 1e-5);
}

// ── 4. from_records with zero confidence (legacy sentinel) ────────────────────

#[test]
fn from_records_zero_confidence_survives() {
    let records = vec![VaultRecord {
        original: "legacy@example.com".to_string(),
        synthetic: "synth@example.com".to_string(),
        pii_type: PiiType::Email,
        tier: 0,
        confidence: 0.0,  // legacy sentinel
    }];
    let vault = PiiVault::from_records("legacy-conv", 0, records);
    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 1);
    assert_eq!(quints[0].3, 0);
    assert!((quints[0].4 - 0.0).abs() < 1e-9, "zero confidence must be preserved, not coerced");
}

// ── 5. StoredVaultRecord with None confidence deserialises correctly ───────────

#[test]
fn stored_vault_record_none_confidence_deserialises() {
    // Simulates a legacy JSON line that lacks the `confidence` field.
    let json = r#"{"original":"alice@acme.com","synthetic":"bob@example.com","pii_type":"email"}"#;
    let rec: StoredVaultRecord = serde_json::from_str(json).unwrap();
    assert_eq!(rec.original, "alice@acme.com");
    assert_eq!(rec.synthetic, "bob@example.com");
    assert!(rec.confidence.is_none(), "missing confidence should deserialise as None");
    assert!(rec.tier.is_none(), "missing tier should deserialise as None");
}

// ── 6. StoredVaultRecord with explicit confidence round-trips ──────────────────

#[test]
fn stored_vault_record_with_confidence_round_trips() {
    let rec = StoredVaultRecord {
        original: "alice@acme.com".to_string(),
        synthetic: "bob@example.com".to_string(),
        pii_type: "email".to_string(),
        tier: Some(2),
        confidence: Some(0.88),
    };
    let json = serde_json::to_string(&rec).unwrap();

    // Confidence must appear in the JSON.
    assert!(json.contains("\"confidence\""), "confidence field missing from JSON: {json}");
    // Tier must appear in the JSON.
    assert!(json.contains("\"tier\""), "tier field missing from JSON: {json}");

    let rec2: StoredVaultRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec2.confidence, Some(0.88f32));
    assert_eq!(rec2.tier, Some(2u8));
}

// ── 7. StoredVaultRecord with None fields omitted from JSON ───────────────────

#[test]
fn stored_vault_record_none_fields_omitted_from_json() {
    let rec = StoredVaultRecord {
        original: "x".to_string(),
        synthetic: "y".to_string(),
        pii_type: "email".to_string(),
        tier: None,
        confidence: None,
    };
    let json = serde_json::to_string(&rec).unwrap();

    // With skip_serializing_if = "Option::is_none", these fields must be absent.
    assert!(!json.contains("\"confidence\""), "None confidence should be absent from JSON: {json}");
    assert!(!json.contains("\"tier\""), "None tier should be absent from JSON: {json}");
}

// ── 8. Confidence round-trips through save_vault + load_vault ─────────────────

#[test]
fn confidence_round_trips_through_storage() {
    let (store, _dir, conv_id) = make_store_with_conv("conf-storage");

    let mappings: Vec<(String, String, String, u8, f32)> = vec![
        ("alice@acme.com".to_string(), "bob@example.com".to_string(), "email".to_string(), 1, 0.99),
        ("555-123-4567".to_string(),   "555-000-0001".to_string(),    "phone".to_string(), 1, 0.85),
        ("123-45-6789".to_string(),    "999-00-0001".to_string(),     "ssn".to_string(),   2, 0.72),
    ];

    store.save_vault(&conv_id, 777, &mappings).unwrap();

    let (seed, records) = store.load_vault(&conv_id).unwrap().expect("vault should be present");
    assert_eq!(seed, 777);
    assert_eq!(records.len(), 3);

    for (orig, _synth, pii_type, expected_tier, expected_conf) in &mappings {
        let rec = records.iter().find(|r| &r.original == orig)
            .unwrap_or_else(|| panic!("record for {} not found", orig));
        assert_eq!(rec.pii_type, *pii_type);
        assert_eq!(rec.tier, Some(*expected_tier));
        let stored_conf = rec.confidence.expect("confidence should be Some after save_vault");
        assert!(
            (stored_conf - expected_conf).abs() < 1e-5,
            "confidence mismatch for {}: stored={}, expected={}", orig, stored_conf, expected_conf
        );
    }
}

// ── 9. save_vault + load_vault + from_records: end-to-end confidence ──────────

#[test]
fn confidence_end_to_end_vault_restore() {
    let (store, _dir, conv_id) = make_store_with_conv("conf-e2e");

    let mappings: Vec<(String, String, String, u8, f32)> = vec![
        ("real@corp.com".to_string(), "fake@example.com".to_string(), "email".to_string(), 1, 0.95),
        ("John Doe".to_string(),      "Jane Smith".to_string(),       "person_name".to_string(), 2, 0.80),
    ];
    store.save_vault(&conv_id, 999, &mappings).unwrap();

    // Simulate vault reload (as VaultRegistry::get_or_create_with_store does).
    let (seed, records) = store.load_vault(&conv_id).unwrap().unwrap();
    let vault_records: Vec<VaultRecord> = records.into_iter().map(|r| VaultRecord {
        original: r.original,
        synthetic: r.synthetic,
        pii_type: PiiType::Custom(r.pii_type),
        tier: r.tier.unwrap_or(0),
        confidence: r.confidence.unwrap_or(0.0),
    }).collect();
    let vault = PiiVault::from_records(&conv_id, seed, vault_records);

    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 2);

    let email_q = quints.iter().find(|q| q.0 == "real@corp.com").expect("email quint missing");
    assert!((email_q.4 - 0.95).abs() < 1e-5, "email confidence after reload: {}", email_q.4);

    let person_q = quints.iter().find(|q| q.0 == "John Doe").expect("person quint missing");
    assert!((person_q.4 - 0.80).abs() < 1e-5, "person confidence after reload: {}", person_q.4);
}

// ── 10. Legacy None confidence maps to 0.0 in VaultRegistry load path ─────────

#[test]
fn none_confidence_in_stored_record_maps_to_zero_in_from_records() {
    // Simulates the StoredVaultRecord → VaultRecord conversion in VaultRegistry.
    let stored = StoredVaultRecord {
        original: "legacy@corp.com".to_string(),
        synthetic: "safe@example.com".to_string(),
        pii_type: "email".to_string(),
        tier: None,    // legacy: no tier stored
        confidence: None,  // legacy: no confidence stored
    };

    // Replicate the mapping done in vault.rs get_or_create_with_store.
    let vault_record = VaultRecord {
        original: stored.original.clone(),
        synthetic: stored.synthetic.clone(),
        pii_type: PiiType::Custom(stored.pii_type.clone()),
        tier: stored.tier.unwrap_or(0),
        confidence: stored.confidence.unwrap_or(0.0),
    };

    assert_eq!(vault_record.tier, 0, "None tier should map to 0");
    assert_eq!(vault_record.confidence, 0.0, "None confidence should map to 0.0");

    let vault = PiiVault::from_records("legacy-test", 0, vec![vault_record]);
    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 1);
    assert_eq!(quints[0].3, 0u8);
    assert!((quints[0].4 - 0.0).abs() < 1e-9);
}

// ── 11. quints() on empty vault returns no items ──────────────────────────────

#[test]
fn quints_on_empty_vault_returns_empty() {
    let vault = PiiVault::new("empty-vault");
    let quints: Vec<_> = vault.quints().collect();
    assert!(quints.is_empty());
}

// ── 12. idempotent add does not duplicate confidence ──────────────────────────

#[test]
fn idempotent_add_does_not_duplicate_confidence() {
    let mut vault = PiiVault::new("idem-conf");
    vault.add_mapping("a@a.com".to_string(), "x@x.com".to_string(), &PiiType::Email, 1, 0.9);
    // Second add with different confidence is silently ignored.
    vault.add_mapping("a@a.com".to_string(), "y@y.com".to_string(), &PiiType::Email, 1, 0.5);

    let quints: Vec<_> = vault.quints().collect();
    assert_eq!(quints.len(), 1, "idempotent add should not create a second entry");
    assert!((quints[0].4 - 0.9).abs() < 1e-5, "original confidence should be preserved");
}
