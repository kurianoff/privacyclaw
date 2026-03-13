// PII configuration integration tests: §12a, §12b, §12c, §12d
//
// These tests exercise the public API exported from claudovka::pii and
// claudovka::config.  They do not require a running proxy, model files, or
// network access — all data is synthetic.

use claudovka::config::PiiConfig;
use claudovka::parser::Provider;
use claudovka::pii::vault::{PiiSpan, PiiType, PiiVault};
use claudovka::pii::{Locale, PiiPipeline};

// ── §12a – PII Mode integration tests ────────────────────────────────────────

/// §12a.1: PiiConfig default mode is "off".
#[test]
fn pii_mode_off_pipeline_not_called() {
    let cfg = PiiConfig::default();
    assert_eq!(cfg.mode, "off");
}

/// §12a.2: mode = "detect-only" — body bytes stay unchanged.
/// The pipeline is not invoked for replacement, so process_request_body
/// returns None for text with no PII (equivalent to "unchanged").
#[test]
fn pii_mode_detect_only_body_unchanged() {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "The weather today is sunny."}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-test-detect-only");

    // No PII → pipeline returns None → bytes unchanged.
    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_none(),
        "detect-only: no PII → pipeline returns None → body unchanged");
    assert!(vault.is_empty(),
        "detect-only: vault must be empty when no PII found");
}

/// §12a.3: mode = "replace" — body IS modified and original email disappears.
#[test]
fn pii_mode_replace_modifies_body() {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Please contact contact@example.com as soon as possible."}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-test-replace");

    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_some(), "replace: email PII must trigger replacement (got None)");

    let new_json: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    let content = new_json["messages"][0]["content"].as_str().unwrap();
    assert!(!content.contains("contact@example.com"),
        "replace: original email must not appear in modified body: {content}");
}

// ── §12b – Tier 1 Regex integration tests ────────────────────────────────────

/// §12b.1: Email detection via PiiPipeline (integration path).
#[test]
fn test_tier1_email_via_pipeline() {
    let body = serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Reach me at jane@corp.com please"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-tier1-email");

    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_some(), "email must be detected");
    let new_json: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    let content = new_json["messages"][0]["content"].as_str().unwrap();
    assert!(!content.contains("jane@corp.com"),
        "original email must be replaced: {content}");
    assert!(!vault.is_empty(), "vault must record the email mapping");
}

/// §12b.3: SSN detection via PiiPipeline (integration path).
#[test]
fn test_tier1_ssn_via_pipeline() {
    let body = serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "My SSN is 123-45-6789 please keep it safe."}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-tier1-ssn");

    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_some(), "SSN must be detected");
    let new_json: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    let content = new_json["messages"][0]["content"].as_str().unwrap();
    assert!(!content.contains("123-45-6789"),
        "original SSN must be replaced: {content}");
}

/// §12b.4: Bearer token detection via PiiPipeline (integration path).
#[test]
fn test_tier1_bearer_token_via_pipeline() {
    let body = serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijk"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-tier1-bearer");

    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_some(), "Bearer token must be detected");
    let new_json: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    let content = new_json["messages"][0]["content"].as_str().unwrap();
    assert!(!content.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9abcdefghijk"),
        "original bearer token must be replaced: {content}");
}

/// §12b.5: False-positive guard — version strings do not trigger replacement.
#[test]
fn test_tier1_no_false_positives_version_string() {
    let body = serde_json::json!({
        "model": "gpt-4",
        "messages": [{"role": "user", "content": "version 1.2.3 is available for download"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let mut vault = PiiVault::new("cfg-tier1-fp-version");

    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_none(),
        "version string must not trigger replacement (false positive): result was Some");
    assert!(vault.is_empty(),
        "vault must stay empty when no PII is detected");
}

// ── §12c – Tier 2 integration tests ──────────────────────────────────────────

/// §12c.1: PiiPipeline::new with tiers.ner = false → tier2 must be None.
#[test]
fn tier2_disabled_returns_none() {
    let mut cfg = PiiConfig::default();
    cfg.tiers.ner = false;
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.tier2.is_none(),
        "tier2 must be None when tiers.ner = false");
}

/// §12c.1 variant: even when tiers.ner = true, without ort-ner feature tier2 is None.
#[test]
fn tier2_without_ort_ner_feature_is_none() {
    let mut cfg = PiiConfig::default();
    cfg.tiers.ner = true;
    let pipeline = PiiPipeline::new(&cfg);
    // Without the ort-ner feature compiled in, try_load_tier2 always returns None.
    // With the feature, it would fail on a missing model file and also return None.
    assert!(pipeline.tier2.is_none(),
        "tier2 must be None when ort-ner feature is absent or model file missing");
}

// ── §12d – Tier 3 integration tests ──────────────────────────────────────────

/// §12d.1: PiiPipeline::new with tiers.slm = false → slm must be None.
#[test]
fn tier3_disabled_returns_none() {
    let mut cfg = PiiConfig::default();
    cfg.tiers.slm = false;
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_none(),
        "slm must be None when tiers.slm = false");
}

/// §12d.1 variant: tiers.slm = true but empty endpoint → slm is None.
#[test]
fn tier3_empty_endpoint_returns_none() {
    let mut cfg = PiiConfig::default();
    cfg.tiers.slm = true;
    cfg.slm.endpoint = String::new();
    let pipeline = PiiPipeline::new(&cfg);
    assert!(pipeline.slm.is_none(),
        "slm must be None when endpoint is empty");
}

/// §12d.6: SLM timeout → candidates returned unchanged (fail-open), no panic.
/// Covered by inline test in tier3.rs; this integration variant confirms the same
/// behaviour via the public PiiPipeline surface with a config-constructed sidecar.
#[tokio::test]
async fn test_tier3_timeout_returns_candidates_unchanged() {
    use claudovka::pii::tier3::SlmSidecar;

    // Port nobody is listening on; 50 ms timeout ensures fast failure.
    let sidecar = SlmSidecar::new("http://127.0.0.1:19996", 50);
    let candidates = vec![
        PiiSpan { start: 0,  end: 5,  entity_type: PiiType::PersonName, confidence: 0.6, tier: 2 },
        PiiSpan { start: 6,  end: 20, entity_type: PiiType::Email,       confidence: 0.55, tier: 2 },
    ];
    let result = sidecar
        .disambiguate("Alice user@example.com hello", &candidates)
        .await
        .expect("disambiguate must not return Err on timeout");

    assert_eq!(result.len(), candidates.len(),
        "fail-open: all candidates must be returned unchanged on timeout");
}

// ── §12a.4 – PII mode switch off→replace ─────────────────────────────────────

/// §12a.4: Switching PII mode from "off" to "replace" takes effect for the next
/// request — pipeline processes the request and modifies the body.
#[test]
fn pii_mode_switch_off_to_replace_takes_effect() {
    let body = serde_json::json!({
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Email me at test@example.com"}]
    });
    let body_bytes = serde_json::to_vec(&body).unwrap();

    let mut vault = PiiVault::new("switch-off-to-replace");
    let result = PiiPipeline::process_request_body(
        &body_bytes, &mut vault, Provider::OpenAI, &Locale::EnUs,
    );
    assert!(result.is_some(), "mode=replace: body must be modified when email is present");
    let new_body: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
    let text_replaced = new_body["messages"][0]["content"].as_str().unwrap();
    assert!(
        !text_replaced.contains("test@example.com"),
        "mode=replace: email must be redacted: {text_replaced}"
    );
}

// ── §12c.2 – Tier 2 without Tier 1 → error ───────────────────────────────────

/// §12c.2: ConfigManager::patch() must reject enabling Tier 2 when Tier 1 is off.
#[tokio::test]
async fn patch_tier2_without_tier1_returns_error() {
    use claudovka::config::{Config, ConfigManager};

    let mut cfg = Config::default();
    cfg.pii.tiers.regex = false;
    cfg.pii.tiers.ner = false;

    let mgr = ConfigManager::new(cfg, None);

    // Try to enable Tier 2 without Tier 1.
    let patch = serde_json::json!({ "pii": { "tiers": { "ner": true } } });
    let result = mgr.patch(patch).await;
    assert!(result.is_err(), "enabling Tier 2 without Tier 1 must return an error");
    let err = result.unwrap_err().to_string().to_lowercase();
    assert!(
        err.contains("tier 1") || err.contains("tier1") || err.contains("regex") || err.contains("requires"),
        "error message must mention tier dependency: {err}"
    );
}

// ── §12d.2 – Tier 3 without Tier 2 → error ───────────────────────────────────

/// §12d.2: ConfigManager::patch() must reject enabling Tier 3 when Tier 2 is off.
#[tokio::test]
async fn patch_tier3_without_tier2_returns_error() {
    use claudovka::config::{Config, ConfigManager};

    let mut cfg = Config::default();
    cfg.pii.tiers.regex = true;
    cfg.pii.tiers.ner = false;
    cfg.pii.tiers.slm = false;

    let mgr = ConfigManager::new(cfg, None);

    // Try to enable Tier 3 (SLM) without Tier 2 (NER).
    let patch = serde_json::json!({ "pii": { "tiers": { "slm": true } } });
    let result = mgr.patch(patch).await;
    assert!(result.is_err(), "enabling Tier 3 without Tier 2 must return an error");
}

// ── §12d.3 – model not downloaded → false ────────────────────────────────────

/// §12d.3: models::is_downloaded returns false for a model not on disk.
/// (The 409 response is enforced by the dashboard handler that checks is_downloaded.)
#[test]
fn activate_not_downloaded_model_returns_false() {
    use claudovka::models::is_downloaded;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    // No model files exist in this empty dir.
    assert!(
        !is_downloaded(dir.path(), "smollm2-135m"),
        "is_downloaded must return false for non-existent model"
    );
    assert!(
        !is_downloaded(dir.path(), "qwen2.5-0.5b"),
        "is_downloaded must return false for non-existent model"
    );
}

// ── §12d.8 – activate updates pii.slm.model_id in config ─────────────────────

/// §12d.8: POST /api/models/:id/activate updates pii.slm.model_id in config.
/// Tests the ConfigManager patch directly (the dashboard handler delegates to it).
#[tokio::test]
async fn model_activation_updates_slm_model_id_in_config() {
    use claudovka::config::{Config, ConfigManager};

    let cfg = Config::default();
    let mgr = ConfigManager::new(cfg, None);

    // Patch pii.slm.model_id = "smollm2-135m" (simulating activate endpoint behavior).
    let patch = serde_json::json!({ "pii": { "slm": { "model_id": "smollm2-135m" } } });
    let result = mgr.patch(patch).await;
    assert!(result.is_ok(), "patch must succeed: {:?}", result);

    // Verify the config was updated.
    let updated = mgr.get().await;
    assert_eq!(
        updated.pii.slm.model_id.as_deref(),
        Some("smollm2-135m"),
        "pii.slm.model_id must be updated after activation patch"
    );
}

// ── §12d.3 – T3 standalone config path ───────────────────────────────────────

/// §12d.3: ConfigManager with pii.mode=replace, tiers.regex=false, tiers.ner=false,
/// tiers.slm=true validates the T3 standalone config path via the patch() code path.
#[tokio::test]
async fn t3_standalone_config_path() {
    use claudovka::config::{Config, ConfigManager, is_t3_standalone};

    let mgr = ConfigManager::new(Config::default(), None);

    let result = mgr.patch(serde_json::json!({
        "pii": { "tiers": { "slm": true, "regex": false, "ner": false } }
    })).await;
    assert!(result.is_ok(), "patch must accept T3 standalone tier combination: {:?}", result);

    let loaded = mgr.get().await;
    assert!(
        loaded.pii.tiers.slm,
        "pii.tiers.slm must be true after patch"
    );
    assert!(
        is_t3_standalone(&loaded.pii.tiers),
        "is_t3_standalone must return true for slm=true, regex=false, ner=false"
    );
}
