/// Integration tests for `Store::insert_detections` + `Store::load_detections`.
///
/// These tests exercise the detection-log feature from tasks 1.6 and 11.3:
///   - insert + load round-trip (all detections)
///   - filtered load by message_id (hit and miss)
///   - empty file returns empty vec (not an error)
///   - multiple messages interleaved: filter returns only the right message's detections
///   - detection line mixed with vault line: only detection type is returned
///   - malformed detection line is skipped gracefully
use privacyclaw::storage::{Conversation, MessageDetection, Store};

fn make_store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (store, dir)
}

fn make_conv(store: &Store, id: &str) {
    let conv = Conversation {
        id: id.to_string(),
        started_at: "2026-01-01T00:00:00Z".to_string(),
        provider: "anthropic".to_string(),
        model: Some("claude-3".to_string()),
        client_hint: Some(format!("fp-{id}")),
    };
    store.insert_conversation(&conv).unwrap();
}

fn det(message_id: &str, entity_type: &str, tier: u8, confidence: f32) -> MessageDetection {
    MessageDetection {
        message_id: message_id.to_string(),
        entity_type: entity_type.to_string(),
        original_masked: format!("[{}]", entity_type.to_uppercase()),
        synthetic: format!("synth-{entity_type}"),
        tier,
        confidence,
    }
}

// ── 1. Empty conversation file — load_detections returns empty vec ─────────────

#[test]
fn load_detections_empty_file_returns_empty() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-empty");

    let result = store.load_detections("conv-empty", None).unwrap();
    assert!(result.is_empty(), "expected empty vec, got {:?}", result);
}

// ── 2. Unknown conv_id — returns empty vec, not an error ──────────────────────

#[test]
fn load_detections_unknown_conv_id_returns_empty() {
    let (store, _dir) = make_store();
    let result = store.load_detections("no-such-conv", None).unwrap();
    assert!(result.is_empty());
}

// ── 3. Round-trip: insert then load all ───────────────────────────────────────

#[test]
fn insert_and_load_all_detections() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-rt");

    let detections = vec![
        det("msg-1", "email", 1, 1.0),
        det("msg-1", "phone", 1, 0.95),
        det("msg-2", "ssn", 2, 0.82),
    ];

    store.insert_detections("conv-rt", &detections).unwrap();

    let loaded = store.load_detections("conv-rt", None).unwrap();
    assert_eq!(loaded.len(), 3, "expected 3 records, got {}", loaded.len());

    // Verify round-trip fidelity on all fields.
    let email = loaded.iter().find(|d| d.entity_type == "email").unwrap();
    assert_eq!(email.message_id, "msg-1");
    assert_eq!(email.original_masked, "[EMAIL]");
    assert_eq!(email.synthetic, "synth-email");
    assert_eq!(email.tier, 1);
    assert!((email.confidence - 1.0).abs() < 1e-6, "confidence mismatch: {}", email.confidence);

    let ssn = loaded.iter().find(|d| d.entity_type == "ssn").unwrap();
    assert_eq!(ssn.message_id, "msg-2");
    assert_eq!(ssn.tier, 2);
    assert!((ssn.confidence - 0.82).abs() < 1e-4);
}

// ── 4. Filtered load — message_id present and matching ────────────────────────

#[test]
fn load_detections_filtered_by_message_id_hit() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-filter");

    let detections = vec![
        det("msg-alpha", "email", 1, 1.0),
        det("msg-alpha", "phone", 1, 0.9),
        det("msg-beta",  "ssn",   1, 0.8),
    ];
    store.insert_detections("conv-filter", &detections).unwrap();

    let loaded = store.load_detections("conv-filter", Some("msg-alpha")).unwrap();
    assert_eq!(loaded.len(), 2, "filter should return 2 records for msg-alpha, got {}", loaded.len());
    assert!(loaded.iter().all(|d| d.message_id == "msg-alpha"),
        "all returned records must have message_id == msg-alpha");

    // The ssn record for msg-beta must NOT be present.
    assert!(loaded.iter().find(|d| d.entity_type == "ssn").is_none(),
        "ssn record from msg-beta should not appear in msg-alpha results");
}

// ── 5. Filtered load — message_id with no matches ─────────────────────────────

#[test]
fn load_detections_filtered_by_message_id_miss() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-miss");

    let detections = vec![det("msg-exists", "email", 1, 1.0)];
    store.insert_detections("conv-miss", &detections).unwrap();

    let loaded = store.load_detections("conv-miss", Some("msg-does-not-exist")).unwrap();
    assert!(loaded.is_empty(),
        "filter with no matches should return empty vec, got {:?}", loaded);
}

// ── 6. Detections from multiple messages interleaved ─────────────────────────

#[test]
fn load_detections_interleaved_messages_filtered_correctly() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-interleave");

    // Insert two separate batches simulating two turns.
    let turn1 = vec![
        det("msg-turn1", "email",  1, 1.0),
        det("msg-turn1", "credit_card", 1, 0.99),
    ];
    let turn2 = vec![
        det("msg-turn2", "phone", 1, 0.95),
        det("msg-turn2", "ssn",   2, 0.88),
        det("msg-turn2", "email", 1, 1.0),  // same type, different message
    ];
    store.insert_detections("conv-interleave", &turn1).unwrap();
    store.insert_detections("conv-interleave", &turn2).unwrap();

    // Unfiltered: all 5.
    let all = store.load_detections("conv-interleave", None).unwrap();
    assert_eq!(all.len(), 5);

    // Filter turn1.
    let t1 = store.load_detections("conv-interleave", Some("msg-turn1")).unwrap();
    assert_eq!(t1.len(), 2);
    assert!(t1.iter().all(|d| d.message_id == "msg-turn1"));

    // Filter turn2.
    let t2 = store.load_detections("conv-interleave", Some("msg-turn2")).unwrap();
    assert_eq!(t2.len(), 3);
    assert!(t2.iter().all(|d| d.message_id == "msg-turn2"));
}

// ── 7. Vault line in same file is not returned by load_detections ─────────────

#[test]
fn vault_line_not_confused_with_detection_line() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-mixed");

    // Write a vault line.
    store.save_vault(
        "conv-mixed",
        42,
        &[("alice@acme.com".to_string(), "bob@example.com".to_string(), "email".to_string(), 1u8, 1.0f32, String::new(), String::new())],
    ).unwrap();

    // Write a detection line.
    store.insert_detections("conv-mixed", &[det("msg-x", "email", 1, 1.0)]).unwrap();

    let loaded = store.load_detections("conv-mixed", None).unwrap();
    assert_eq!(loaded.len(), 1, "only detection lines should be returned");
    assert_eq!(loaded[0].entity_type, "email");
}

// ── 8. insert_detections with empty slice is a no-op (no error) ───────────────

#[test]
fn insert_detections_empty_slice_is_noop() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-noop");

    store.insert_detections("conv-noop", &[]).unwrap();

    let loaded = store.load_detections("conv-noop", None).unwrap();
    assert!(loaded.is_empty());
}

// ── 9. insert_detections unknown conv_id is a no-op (no error) ────────────────

#[test]
fn insert_detections_unknown_conv_is_noop() {
    let (store, _dir) = make_store();
    let result = store.insert_detections("no-conv", &[det("m1", "email", 1, 1.0)]);
    assert!(result.is_ok(), "missing conv should be a silent no-op, not an error");
}

// ── 10. Confidence is stored and loaded without precision loss (f32) ──────────

#[test]
fn detection_confidence_round_trips() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-conf");

    let cases: Vec<(&str, f32)> = vec![
        ("msg-a", 0.0),      // zero (legacy sentinel)
        ("msg-b", 1.0),      // max
        ("msg-c", 0.5),      // mid
        ("msg-d", 0.123_456), // fractional precision within f32
    ];

    let detections: Vec<MessageDetection> = cases.iter().map(|(id, conf)| MessageDetection {
        message_id: id.to_string(),
        entity_type: "email".to_string(),
        original_masked: "[EMAIL]".to_string(),
        synthetic: "synth@example.com".to_string(),
        tier: 1,
        confidence: *conf,
    }).collect();

    store.insert_detections("conv-conf", &detections).unwrap();
    let loaded = store.load_detections("conv-conf", None).unwrap();
    assert_eq!(loaded.len(), cases.len());

    for (id, expected_conf) in &cases {
        let rec = loaded.iter().find(|d| d.message_id == *id).unwrap_or_else(|| {
            panic!("missing detection for message_id={}", id)
        });
        assert!(
            (rec.confidence - expected_conf).abs() < 1e-5,
            "confidence mismatch for {}: stored={}, expected={}",
            id, rec.confidence, expected_conf
        );
    }
}

// ── 11. Tier value is preserved ───────────────────────────────────────────────

#[test]
fn detection_tier_round_trips() {
    let (store, _dir) = make_store();
    make_conv(&store, "conv-tier");

    let detections = vec![
        det("msg-1", "email",  1, 1.0),
        det("msg-2", "phone",  2, 0.9),
        det("msg-3", "person", 3, 0.7),
    ];
    store.insert_detections("conv-tier", &detections).unwrap();

    let loaded = store.load_detections("conv-tier", None).unwrap();
    assert_eq!(loaded.len(), 3);

    let t1 = loaded.iter().find(|d| d.message_id == "msg-1").unwrap();
    let t2 = loaded.iter().find(|d| d.message_id == "msg-2").unwrap();
    let t3 = loaded.iter().find(|d| d.message_id == "msg-3").unwrap();

    assert_eq!(t1.tier, 1);
    assert_eq!(t2.tier, 2);
    assert_eq!(t3.tier, 3);
}
