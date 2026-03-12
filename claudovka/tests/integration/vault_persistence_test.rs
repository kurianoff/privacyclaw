use claudovka::storage::{Store, Conversation};
use tempfile::tempdir;

#[test]
fn vault_save_and_load() {
    let dir = tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();

    // Store::save_vault requires a conversation file to exist first.
    let conv = Conversation {
        id: "conv-persist-test".to_string(),
        started_at: "2026-03-08T00:00:00Z".to_string(),
        provider: "openai".to_string(),
        model: Some("gpt-4o".to_string()),
        client_hint: None,
    };
    store.insert_conversation(&conv).unwrap();

    let records = vec![
        ("alice@acme.com".to_string(), "bob@example.com".to_string(), "email".to_string(), 1u8, 1.0f32),
        ("123-45-6789".to_string(), "987-65-4321".to_string(), "ssn".to_string(), 1u8, 1.0f32),
    ];
    store.save_vault("conv-persist-test", 42, &records).unwrap();

    let result = store.load_vault("conv-persist-test").unwrap();
    assert!(result.is_some(), "vault should exist after save");

    let (seed, loaded_records) = result.unwrap();
    assert_eq!(seed, 42, "rng_seed must round-trip");
    assert_eq!(loaded_records.len(), 2, "expected 2 mappings");

    let synthetics: Vec<&str> = loaded_records.iter().map(|r| r.synthetic.as_str()).collect();
    assert!(synthetics.contains(&"bob@example.com"), "bob@example.com not found in synthetics");
    assert!(synthetics.contains(&"987-65-4321"), "987-65-4321 not found in synthetics");
}
