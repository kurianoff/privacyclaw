/// Non-interactive unit tests for `cmd_config` helpers.
use claudovka::cmd_config::{dot_path_to_json, protection_level_to_patch};
use claudovka::config::Config;

#[test]
fn dot_path_simple_string() {
    let v = dot_path_to_json("pii.mode", "replace").unwrap();
    assert_eq!(v["pii"]["mode"], serde_json::Value::String("replace".to_string()));
}

#[test]
fn dot_path_bool_true() {
    let v = dot_path_to_json("pii.tiers.ner", "true").unwrap();
    assert_eq!(v["pii"]["tiers"]["ner"], serde_json::Value::Bool(true));
}

#[test]
fn dot_path_bool_false() {
    let v = dot_path_to_json("pii.tiers.slm", "false").unwrap();
    assert_eq!(v["pii"]["tiers"]["slm"], serde_json::Value::Bool(false));
}

#[test]
fn dot_path_integer() {
    let v = dot_path_to_json("pii.vault_ttl_hours", "48").unwrap();
    assert_eq!(v["pii"]["vault_ttl_hours"], serde_json::Value::Number(48.into()));
}

#[test]
fn dot_path_single_segment() {
    let v = dot_path_to_json("mode", "detect-only").unwrap();
    assert_eq!(v["mode"], serde_json::Value::String("detect-only".to_string()));
}

#[test]
fn show_config_contains_pii_section() {
    // Verify the TOML serialiser produces a [pii] section — this is what
    // show_config prints.
    let cfg = Config::default();
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    assert!(toml_str.contains("[pii]"), "TOML output must contain [pii] section");
    assert!(toml_str.contains("mode"), "TOML output must contain pii.mode field");
    assert!(!toml_str.is_empty());
}

#[test]
fn protection_level_off_produces_correct_patch() {
    let patch = protection_level_to_patch("off").expect("off should not error");
    assert_eq!(patch["pii"]["mode"], "off");
    assert_eq!(patch["pii"]["tiers"]["regex"], false);
    assert_eq!(patch["pii"]["tiers"]["ner"], false);
    assert_eq!(patch["pii"]["tiers"]["slm"], false);
}

#[test]
fn protection_level_1_produces_correct_patch() {
    let patch = protection_level_to_patch("1").expect("level 1 should not error");
    assert_eq!(patch["pii"]["mode"], "replace");
    assert_eq!(patch["pii"]["tiers"]["regex"], true);
    assert_eq!(patch["pii"]["tiers"]["ner"], false);
    assert_eq!(patch["pii"]["tiers"]["slm"], false);
}

#[test]
fn protection_level_intelligent_produces_correct_patch() {
    let patch = protection_level_to_patch("intelligent").expect("intelligent should not error");
    assert_eq!(patch["pii"]["mode"], "replace");
    assert_eq!(patch["pii"]["tiers"]["regex"], false);
    assert_eq!(patch["pii"]["tiers"]["ner"], false);
    assert_eq!(patch["pii"]["tiers"]["slm"], true);
}

#[test]
fn protection_level_2_produces_correct_patch() {
    let patch = protection_level_to_patch("2").expect("level 2 should not error");
    assert_eq!(patch["pii"]["mode"], "replace");
    assert_eq!(patch["pii"]["tiers"]["regex"], true);
    assert_eq!(patch["pii"]["tiers"]["ner"], true);
    assert_eq!(patch["pii"]["tiers"]["slm"], false);
}

#[test]
fn protection_level_detect_produces_correct_patch() {
    let patch = protection_level_to_patch("detect").expect("detect should not error");
    assert_eq!(patch["pii"]["mode"], "detect-only");
    assert_eq!(patch["pii"]["tiers"]["regex"], true);
    assert_eq!(patch["pii"]["tiers"]["ner"], false);
    assert_eq!(patch["pii"]["tiers"]["slm"], false);
}

#[test]
fn protection_level_unknown_returns_error() {
    let result = protection_level_to_patch("banana");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("unknown protection level: banana"),
        "error message was: {msg}"
    );
    // Error must list valid options so the user knows what to type.
    assert!(
        msg.contains("intelligent"),
        "error message must list valid options, was: {msg}"
    );
}
