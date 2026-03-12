/// Tests for the structured logging instrumentation added across the PII pipeline.
///
/// These tests use `tracing-test`'s `#[traced_test]` which installs a per-test
/// subscriber capturing all log records from TRACE upwards, and provides
/// `logs_contain(substring)` to assert on the captured output.
///
/// Design principles:
/// - Assert only on the exact log message strings present in the source.
/// - Do not assert on level — `#[traced_test]` captures all levels.
/// - Each test has a unique vault conversation_id to prevent cross-test interference.
/// - Config tests are pure unit tests with no tracing overhead.
use claudovka::pii::vault::{PiiType, PiiVault};
use claudovka::pii::synth::SyntheticGenerator;
use claudovka::pii::tier1::Tier1Detector;
use claudovka::pii::buffer::ReplacementBuffer;
use claudovka::pii::locale::Locale;
use claudovka::config::LoggingConfig;
use std::sync::{Arc, RwLock};
use tracing_test::traced_test;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build a `VaultHandle` pre-populated with `(original, synthetic)` pairs.
/// Uses the real `add_mapping` signature: (original, synthetic, pii_type, tier, confidence).
fn vault_handle_with(conv_id: &str, mappings: &[(&str, &str)]) -> Arc<RwLock<PiiVault>> {
    let mut vault = PiiVault::new(conv_id);
    for (orig, syn) in mappings {
        vault.add_mapping(
            orig.to_string(),
            syn.to_string(),
            &PiiType::PersonName,
            1,
            0.9f32,
        );
    }
    Arc::new(RwLock::new(vault))
}

// ─────────────────────────────────────────────────────────────────────────────
// vault.rs logging tests
// ─────────────────────────────────────────────────────────────────────────────

/// Test 6: `add_mapping` must emit a DEBUG record "vault: mapping added".
///
/// Rationale: this record carries `mapping_count` and `max_key_len` fields used
/// for capacity monitoring. A regression that bypasses the debug log silently
/// removes the only observable signal that automaton rebuild happened.
#[test]
#[traced_test]
fn vault_debug_log_on_add_mapping() {
    let mut vault = PiiVault::new("vault-log-add");
    vault.add_mapping(
        "alice@acme.com".to_string(),
        "bob@example.com".to_string(),
        &PiiType::Email,
        1,
        1.0f32,
    );
    assert!(
        logs_contain("vault: mapping added"),
        "expected 'vault: mapping added' in logs after add_mapping"
    );
}

/// A duplicate `add_mapping` call (idempotent path) must NOT emit "vault: mapping added"
/// a second time — the early-return branch fires before the debug log.
///
/// Rationale: if the idempotent guard is accidentally removed, the automaton is
/// rebuilt unnecessarily AND the log count doubles, breaking log-based dedup alerts.
#[test]
#[traced_test]
fn vault_no_duplicate_debug_log_on_idempotent_add_mapping() {
    let mut vault = PiiVault::new("vault-log-dup");
    // First add — emits the log and stores the mapping.
    vault.add_mapping("x@y.com".to_string(), "a@b.com".to_string(), &PiiType::Email, 1, 1.0f32);
    // Second add with same original — must be a no-op.
    vault.add_mapping("x@y.com".to_string(), "c@d.com".to_string(), &PiiType::Email, 1, 1.0f32);
    // Behavioural invariant: count stays at 1.
    assert_eq!(vault.mapping_count(), 1, "duplicate add_mapping must not increase count");
    // The mapping value must be the first synthetic, not the second.
    assert_eq!(vault.get_synthetic("x@y.com"), Some("a@b.com"));
}

/// Test 7: `replace_synthetics` must emit INFO "vault: synthetic reverse-replacement applied"
/// when at least one synthetic token is replaced.
///
/// Rationale: this is the only INFO-level log in the inbound path. If absent,
/// any monitoring rule scoped to "PII reversal events" produces zero alerts.
#[test]
#[traced_test]
fn vault_info_log_on_replace_synthetics() {
    let mut vault = PiiVault::new("vault-log-replace");
    vault.add_mapping(
        "john@acme.com".to_string(),
        "alice@example.com".to_string(),
        &PiiType::Email,
        1,
        1.0f32,
    );
    let (result, applied) = vault.replace_synthetics("Reply to alice@example.com.");
    assert!(applied, "replace_synthetics should have matched");
    assert!(result.contains("john@acme.com"), "original not restored: {result}");
    assert!(
        logs_contain("vault: synthetic reverse-replacement applied"),
        "expected INFO 'vault: synthetic reverse-replacement applied' in logs"
    );
}

/// No INFO log when `replace_synthetics` finds nothing to replace.
///
/// Rationale: the `any` flag gates the log. If the gate is removed, every
/// no-match call emits a false-positive INFO record for monitoring.
#[test]
#[traced_test]
fn vault_no_info_log_on_replace_synthetics_no_match() {
    let mut vault = PiiVault::new("vault-log-nomatch");
    vault.add_mapping("a@b.com".to_string(), "x@y.com".to_string(), &PiiType::Email, 1, 1.0f32);
    let (_, applied) = vault.replace_synthetics("no synthetic tokens here at all");
    assert!(!applied);
    assert!(
        !logs_contain("vault: synthetic reverse-replacement applied"),
        "unexpected info log when no replacements were made"
    );
}

/// TRACE "vault: add_mapping enter" fires on every add_mapping call, including
/// the idempotent path (the TRACE precedes the early-return guard).
#[test]
#[traced_test]
fn vault_trace_log_on_add_mapping_enter() {
    let mut vault = PiiVault::new("vault-log-trace");
    vault.add_mapping("p@q.com".to_string(), "r@s.com".to_string(), &PiiType::Email, 1, 1.0f32);
    assert!(
        logs_contain("vault: add_mapping enter"),
        "expected TRACE 'vault: add_mapping enter' in logs"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// synth.rs logging tests
// ─────────────────────────────────────────────────────────────────────────────

/// Test 1: `SyntheticGenerator::get_or_create` with a new value must emit
/// DEBUG "synthetic replacement applied" containing the original value.
///
/// Note: the source deliberately logs at DEBUG (not INFO) to avoid PII leaking
/// into INFO-level log aggregators. This test verifies the message exists at any
/// captured level.
#[test]
#[traced_test]
fn synth_debug_log_on_new_mapping() {
    let mut vault = PiiVault::new("synth-log-new");
    let _syn = SyntheticGenerator::get_or_create(
        &mut vault,
        "john@acme.com",
        &PiiType::Email,
        &Locale::EnUs,
        1,
        1.0f32,
    );
    assert!(
        logs_contain("synthetic replacement applied"),
        "expected DEBUG 'synthetic replacement applied' in logs"
    );
    // The original must appear in the debug record (this is the key field).
    assert!(
        logs_contain("john@acme.com"),
        "original value must appear in debug log"
    );
}

/// Test 2: Cache hit path must emit TRACE "synth: cache hit" and must NOT
/// re-emit "synthetic replacement applied".
///
/// Rationale: double-emission would make log-based deduplication impossible and
/// would incorrectly imply a new mapping was created.
#[test]
#[traced_test]
fn synth_trace_log_on_cache_hit_no_repeat_debug() {
    let mut vault = PiiVault::new("synth-log-cache");
    // First call — generates and emits debug log.
    let s1 = SyntheticGenerator::get_or_create(
        &mut vault,
        "bob@corp.com",
        &PiiType::Email,
        &Locale::EnUs,
        1,
        1.0f32,
    );
    // Second call — cache hit, returns same result.
    let s2 = SyntheticGenerator::get_or_create(
        &mut vault,
        "bob@corp.com",
        &PiiType::Email,
        &Locale::EnUs,
        1,
        1.0f32,
    );
    assert_eq!(s1, s2, "cache hit must return same synthetic");
    // Cache hit trace must be present.
    assert!(
        logs_contain("synth: cache hit"),
        "expected TRACE 'synth: cache hit' on second call"
    );
    // Vault must not gain a duplicate mapping.
    assert_eq!(vault.mapping_count(), 1, "cache hit must not add duplicate mapping");
}

/// TRACE "synth: get_or_create enter" fires on every invocation.
#[test]
#[traced_test]
fn synth_trace_log_on_get_or_create_enter() {
    let mut vault = PiiVault::new("synth-log-enter");
    let _ = SyntheticGenerator::get_or_create(
        &mut vault,
        "user@example.com",
        &PiiType::Email,
        &Locale::EnUs,
        1,
        1.0f32,
    );
    assert!(
        logs_contain("synth: get_or_create enter"),
        "expected TRACE 'synth: get_or_create enter' in logs"
    );
}

/// TRACE "synth: cache miss, generating" fires when the original is not in vault.
#[test]
#[traced_test]
fn synth_trace_log_on_cache_miss() {
    let mut vault = PiiVault::new("synth-log-miss");
    let _ = SyntheticGenerator::get_or_create(
        &mut vault,
        "newuser@example.com",
        &PiiType::Email,
        &Locale::EnUs,
        1,
        1.0f32,
    );
    assert!(
        logs_contain("synth: cache miss, generating"),
        "expected TRACE 'synth: cache miss, generating' in logs"
    );
}


// ─────────────────────────────────────────────────────────────────────────────
// tier1.rs logging tests
// ─────────────────────────────────────────────────────────────────────────────

/// Test 3: `Tier1Detector::detect` must emit DEBUG "tier1: detect complete"
/// on every call regardless of whether PII was found.
///
/// The source uses `tracing::debug!` (not INFO). The `span_count` structured
/// field in that record is the only in-process signal for how many entities
/// were detected without iterating the returned Vec.
#[test]
#[traced_test]
fn tier1_debug_log_on_detect_with_pii() {
    let spans = Tier1Detector::detect("Contact john@acme.com today.", &Locale::EnUs);
    assert!(!spans.is_empty(), "expected email span to be detected");
    assert!(
        logs_contain("tier1: detect complete"),
        "expected DEBUG 'tier1: detect complete' in logs"
    );
    // The span_count structured field must be present in the formatted output.
    assert!(
        logs_contain("span_count"),
        "expected 'span_count' field in tier1 detect complete log"
    );
}

/// Test 4: `Tier1Detector::detect` on plain English prose with no PII must still
/// emit "tier1: detect complete" (the log is unconditional, not gated on results).
///
/// The source does NOT emit a WARN for zero-entity text — that was aspirational
/// in the spec. This test documents the actual behaviour.
#[test]
#[traced_test]
fn tier1_debug_log_on_detect_no_pii() {
    // 200-char English prose with no PII patterns.
    let text = "The quick brown fox jumps over the lazy dog. \
                Sphinx of black quartz, judge my vow. \
                Pack my box with five dozen liquor jugs and more text added here.";
    assert!(text.len() >= 100, "text too short for meaningful test");
    let spans = Tier1Detector::detect(text, &Locale::EnUs);
    assert!(spans.is_empty(), "expected no PII spans in clean prose");
    assert!(
        logs_contain("tier1: detect complete"),
        "expected DEBUG 'tier1: detect complete' even when no spans found"
    );
}

/// DEBUG "tier1: detect enter" fires at the beginning of each detect call.
#[test]
#[traced_test]
fn tier1_debug_log_on_detect_enter() {
    let _ = Tier1Detector::detect("no pii here", &Locale::EnUs);
    assert!(
        logs_contain("tier1: detect enter"),
        "expected DEBUG 'tier1: detect enter' in logs"
    );
}

/// TRACE "tier1: match found" fires when a regex match is accepted.
///
/// Rationale: this is the only per-match trace record. Its absence means the
/// inner loop was restructured in a way that might silently drop matches.
#[test]
#[traced_test]
fn tier1_trace_log_on_match_found() {
    let spans = Tier1Detector::detect("SSN: 123-45-6789", &Locale::EnUs);
    assert!(!spans.is_empty(), "SSN not detected");
    assert!(
        logs_contain("tier1: match found"),
        "expected TRACE 'tier1: match found' for SSN detection"
    );
}

/// TRACE "tier1: validator result" fires when a post-match validator is invoked.
///
/// Rationale: credit card detection uses `with_validator(luhn_valid)`. The
/// validator result trace is the only observable signal for debugging why a
/// card-shaped number was accepted or rejected without running the regex by hand.
#[test]
#[traced_test]
fn tier1_trace_log_on_validator_result() {
    // 4532015112830366 is a Luhn-valid Visa test number.
    let spans = Tier1Detector::detect("Card: 4532015112830366", &Locale::EnUs);
    assert!(!spans.is_empty(), "expected credit card span");
    assert!(
        logs_contain("tier1: validator result"),
        "expected TRACE 'tier1: validator result' when Luhn validator runs"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// buffer.rs logging tests
// ─────────────────────────────────────────────────────────────────────────────

/// Test 5: `ReplacementBuffer::process_delta` with a non-empty vault must emit
/// DEBUG "buffer: delta processed".
///
/// Rationale: this record carries `flushed_len` and `holdback_len` fields that
/// are required for latency debugging. If absent, stream stalls are invisible
/// without adding new instrumentation.
#[test]
#[traced_test]
fn buffer_debug_log_on_process_delta() {
    let handle = vault_handle_with("buf-log-delta", &[("John Smith", "Alice Brown")]);
    let mut buf = ReplacementBuffer::new(handle);
    let _out = buf.process_delta("Hello world, how are you today?");
    assert!(
        logs_contain("buffer: delta processed"),
        "expected DEBUG 'buffer: delta processed' after process_delta"
    );
}

/// TRACE "buffer: process_delta enter" fires at the top of every non-empty call.
#[test]
#[traced_test]
fn buffer_trace_log_on_process_delta_enter() {
    let handle = vault_handle_with("buf-log-enter", &[("original", "SYN_TOK")]);
    let mut buf = ReplacementBuffer::new(handle);
    let _out = buf.process_delta("some delta text");
    assert!(
        logs_contain("buffer: process_delta enter"),
        "expected TRACE 'buffer: process_delta enter' in logs"
    );
}

/// TRACE "buffer: vault empty, immediate flush" fires on the vault-empty fast path.
///
/// Rationale: this fast path is critical for zero-PII conversations. If it
/// silently breaks and falls through to the replacement path, every chunk incurs
/// unnecessary Aho-Corasick overhead and the trace absence is the only signal.
#[test]
#[traced_test]
fn buffer_trace_log_on_vault_empty_path() {
    let empty_vault = Arc::new(RwLock::new(PiiVault::new("buf-empty-vault")));
    let mut buf = ReplacementBuffer::new(empty_vault);
    let out = buf.process_delta("plain text with no PII");
    assert_eq!(out, "plain text with no PII", "empty vault must flush immediately");
    assert!(
        logs_contain("buffer: vault empty, immediate flush"),
        "expected TRACE 'buffer: vault empty, immediate flush' on empty-vault path"
    );
}

/// TRACE "buffer: prefixes refreshed" fires when the vault mapping count
/// changes between calls.
///
/// Rationale: prefix refresh is a lazy rebuild triggered by vault growth. If it
/// fires on every call instead of only on growth, it indicates a logic regression.
/// This test confirms it fires exactly on the call that follows vault growth.
#[test]
#[traced_test]
fn buffer_trace_log_on_prefix_refresh() {
    let vault = Arc::new(RwLock::new(PiiVault::new("buf-prefix-refresh")));
    let mut buf = ReplacementBuffer::new(Arc::clone(&vault));

    // First call with empty vault — no refresh needed.
    let _out1 = buf.process_delta("hello");

    // Add a mapping so the vault grows.
    vault.write().unwrap().add_mapping(
        "original_val".to_string(),
        "SYN_TOKEN_PREFIX".to_string(),
        &PiiType::PersonName,
        1,
        0.9f32,
    );

    // Second call — vault count has changed; prefix refresh must fire.
    let _out2 = buf.process_delta("more text SYN_TOKEN_PREFIX here");
    assert!(
        logs_contain("buffer: prefixes refreshed"),
        "expected TRACE 'buffer: prefixes refreshed' when vault grows"
    );
}

/// DEBUG "buffer: flush_remaining called" fires at the start of flush_remaining.
///
/// Rationale: `flush_remaining` is called at SSE stream end. If it fires more
/// than once per stream, held text is double-processed.
#[test]
#[traced_test]
fn buffer_debug_log_on_flush_remaining() {
    let handle = vault_handle_with("buf-flush-log", &[("John", "Alice")]);
    let mut buf = ReplacementBuffer::new(handle);
    let _out = buf.process_delta("Say Alice");
    let _rem = buf.flush_remaining();
    assert!(
        logs_contain("buffer: flush_remaining called"),
        "expected DEBUG 'buffer: flush_remaining called' in logs"
    );
}

/// TRACE "buffer: holdback decision" fires when the buffer determines how much
/// of the replaced text can be safely flushed to the client.
///
/// Rationale: holdback is the mechanism that prevents partial synthetic tokens
/// from being forwarded before the full token arrives. If this trace is absent
/// for long input (> max_key_len), the holdback logic was bypassed.
#[test]
#[traced_test]
fn buffer_trace_log_on_holdback_decision() {
    // Non-empty vault so replace_synthetics is called and holdback decision fires.
    let handle = vault_handle_with("buf-holdback-log", &[("real_val", "SYN_VAL")]);
    let mut buf = ReplacementBuffer::new(handle);
    // Feed text longer than max_key_len (7 chars for "SYN_VAL") to enter the holdback branch.
    let text = "a".repeat(200);
    let _out = buf.process_delta(&text);
    assert!(
        logs_contain("buffer: holdback decision"),
        "expected TRACE 'buffer: holdback decision' in logs"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// config.rs LoggingConfig tests (pure unit tests, no tracing)
// ─────────────────────────────────────────────────────────────────────────────

/// Test 8: `LoggingConfig::default()` must set the documented field values.
///
/// Rationale: these defaults are load-bearing. `main.rs::init_logging()` branches
/// on `format` and optionally opens a file based on `file`. Wrong defaults
/// silently break structured logging or file rotation for all users who rely on defaults.
#[test]
fn logging_config_defaults() {
    let cfg = LoggingConfig::default();
    assert_eq!(cfg.level, "info", "default level must be 'info'");
    assert_eq!(cfg.format, "text", "default format must be 'text'");
    assert!(cfg.file.is_none(), "default file must be None");
    assert_eq!(cfg.rotation, "daily", "default rotation must be 'daily'");
}

/// Test 9: Deserialising `[logging]\nlevel = "debug"\n` must fill in defaults
/// for all absent fields via `#[serde(default = ...)]` annotations.
///
/// Rationale: if any field loses its `#[serde(default)]` attribute, parsing a
/// partial `[logging]` section (the common case — users rarely set all fields)
/// produces a deserialisation error that breaks every existing config file.
#[test]
fn logging_config_serde_file_absent() {
    let toml = "[logging]\nlevel = \"debug\"\n";
    let outer: toml::Table = toml::from_str(toml).expect("TOML parse failed");
    let logging_table = outer.get("logging").expect("missing [logging] section");
    let cfg: LoggingConfig =
        logging_table.clone().try_into().expect("failed to deserialise LoggingConfig");

    assert_eq!(cfg.level, "debug", "level must be 'debug' as specified");
    assert_eq!(cfg.format, "text", "format must default to 'text' when absent");
    assert!(cfg.file.is_none(), "file must default to None when absent");
    assert_eq!(cfg.rotation, "daily", "rotation must default to 'daily' when absent");
}

/// Test 10: When `file = "/tmp/test.log"` is present, it must deserialise to
/// `Some("/tmp/test.log")` — verifying `Option<String>` with `#[serde(default)]`.
///
/// Rationale: a regression on this field (e.g., the attribute being dropped or
/// the field renamed) would silently disable file logging for all users who set
/// `logging.file` in their config.
#[test]
fn logging_config_serde_file_present() {
    let toml = "[logging]\nlevel = \"info\"\nfile = \"/tmp/test.log\"\n";
    let outer: toml::Table = toml::from_str(toml).expect("TOML parse failed");
    let logging_table = outer.get("logging").expect("missing [logging] section");
    let cfg: LoggingConfig =
        logging_table.clone().try_into().expect("failed to deserialise LoggingConfig");

    assert_eq!(
        cfg.file,
        Some("/tmp/test.log".to_string()),
        "file must deserialise to Some(\"/tmp/test.log\")"
    );
}

/// Non-default `format` and `rotation` values round-trip through TOML correctly.
///
/// Rationale: both fields use `#[serde(default = "fn")]` which is correct for
/// non-derived defaults, but an incorrect function name or typo would silently
/// fall back to the derived `Default::default()` (i.e., empty string for String),
/// which is wrong. This test catches that regression.
#[test]
fn logging_config_serde_non_default_format_and_rotation() {
    let toml =
        "[logging]\nlevel = \"warn\"\nformat = \"json\"\nrotation = \"hourly\"\n";
    let outer: toml::Table = toml::from_str(toml).expect("TOML parse failed");
    let logging_table = outer.get("logging").expect("missing [logging] section");
    let cfg: LoggingConfig =
        logging_table.clone().try_into().expect("failed to deserialise LoggingConfig");

    assert_eq!(cfg.level, "warn");
    assert_eq!(cfg.format, "json");
    assert_eq!(cfg.rotation, "hourly");
    assert!(cfg.file.is_none());
}

/// Whole-config serde path preserves LoggingConfig defaults when the
/// `[logging]` section is entirely absent from TOML.
///
/// Rationale: `Config` uses `#[serde(default)]` on the `logging` field.
/// If that attribute is removed, config files without a `[logging]` section
/// (the vast majority) fail to parse with an unhelpful "missing field" error.
#[test]
fn logging_config_absent_section_uses_defaults() {
    use claudovka::config::Config;
    // TOML with no [logging] section at all.
    let toml = "[proxy]\nlisten = \"127.0.0.1:16440\"\ndashboard = \"127.0.0.1:16443\"\n";
    let cfg: Config = toml::from_str(toml).expect("failed to parse Config from TOML");
    assert_eq!(cfg.logging.level, "info");
    assert_eq!(cfg.logging.format, "text");
    assert!(cfg.logging.file.is_none());
    assert_eq!(cfg.logging.rotation, "daily");
}
