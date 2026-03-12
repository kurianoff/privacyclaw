/// E2E tests for `claudovka init`.
///
/// These tests spawn the real binary and verify the on-disk state it produces.
/// `init` is idempotent so running it multiple times in CI is safe.
#[path = "helpers.rs"]
mod helpers;
use helpers::*;

use std::process::Command;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn run_init() {
    let status = Command::new(binary_path())
        .arg("init")
        .status()
        .expect("claudovka init");
    assert!(status.success(), "claudovka init exited with failure");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// After `claudovka init`, ca.pem and ca.key.pem must exist in the CA directory.
#[test]
fn test_init_creates_ca_files() {
    run_init();
    let ca_dir = default_ca_dir();
    assert!(
        ca_dir.join("ca.pem").exists(),
        "ca.pem not found in {:?}",
        ca_dir
    );
    assert!(
        ca_dir.join("ca.key.pem").exists(),
        "ca.key.pem not found in {:?}",
        ca_dir
    );
}

/// ca.pem must contain a valid PEM certificate block.
#[test]
fn test_ca_pem_contains_valid_certificate_block() {
    run_init();
    let pem = std::fs::read_to_string(default_ca_dir().join("ca.pem"))
        .expect("read ca.pem");
    assert!(
        pem.contains("-----BEGIN CERTIFICATE-----"),
        "ca.pem missing BEGIN CERTIFICATE header"
    );
    assert!(
        pem.contains("-----END CERTIFICATE-----"),
        "ca.pem missing END CERTIFICATE footer"
    );
}

/// ca.key.pem must contain a private key PEM block.
#[test]
fn test_ca_key_pem_contains_private_key_block() {
    run_init();
    let key_pem = std::fs::read_to_string(default_ca_dir().join("ca.key.pem"))
        .expect("read ca.key.pem");
    assert!(
        key_pem.contains("-----BEGIN") && key_pem.contains("PRIVATE KEY-----"),
        "ca.key.pem does not look like a private key PEM"
    );
}

/// Running `claudovka init` twice must NOT overwrite an existing CA.
/// The PEM bytes must be identical after the second call.
#[test]
fn test_init_is_idempotent() {
    run_init();
    let ca_pem_path = default_ca_dir().join("ca.pem");
    let before = std::fs::read(&ca_pem_path).expect("read ca.pem before");
    run_init();
    let after = std::fs::read(&ca_pem_path).expect("read ca.pem after");
    assert_eq!(
        before, after,
        "claudovka init regenerated the CA on a second call (should be idempotent)"
    );
}

/// `claudovka ca-path` must exit 0 and print a path ending in `.pem`.
#[test]
fn test_ca_path_command() {
    run_init();
    let output = Command::new(binary_path())
        .arg("ca-path")
        .output()
        .expect("claudovka ca-path");
    assert!(
        output.status.success(),
        "ca-path exited non-zero: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The command may emit structured log lines before the path.
    // Find the first line that looks like a filesystem path (ends with .pem).
    let path_str = stdout
        .lines()
        .map(|l| l.trim())
        .find(|l| l.ends_with(".pem") && !l.contains("WARN") && !l.contains("INFO"))
        .expect("ca-path produced no .pem line in output");
    assert!(
        std::path::Path::new(path_str).exists(),
        "ca-path reported path does not exist: {path_str}"
    );
}

/// `claudovka --version` exits 0 and prints a semantic version number.
#[test]
fn test_version_flag() {
    let output = Command::new(binary_path())
        .arg("--version")
        .output()
        .expect("claudovka --version");
    assert!(output.status.success());
    let out = String::from_utf8_lossy(&output.stdout);
    // Expect something like "claudovka 0.2.0" or just "0.2.0"
    assert!(
        out.contains('.'),
        "version output does not look like a semver: {out}"
    );
}

/// `claudovka setup-network` prints /etc/hosts entries for known LLM domains.
#[test]
fn test_setup_network_prints_hosts_entries() {
    run_init();
    let output = Command::new(binary_path())
        .arg("setup-network")
        .output()
        .expect("claudovka setup-network");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("127.0.0.1"),
        "setup-network output missing 127.0.0.1"
    );
    assert!(
        stdout.contains("api.anthropic.com"),
        "setup-network output missing api.anthropic.com"
    );
}

/// `claudovka test-pii` detects an email address in a plain-text string.
#[test]
fn test_pii_detection_email() {
    let output = Command::new(binary_path())
        .args(["test-pii", "Reach me at jane.doe@example.com anytime."])
        .output()
        .expect("claudovka test-pii");
    assert!(
        output.status.success(),
        "test-pii exited non-zero: {:?}",
        output.status
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(
        stdout.contains("email"),
        "test-pii did not detect EMAIL in output: {stdout}"
    );
}

/// `claudovka test-pii` detects a US phone number.
#[test]
fn test_pii_detection_phone() {
    let output = Command::new(binary_path())
        .args(["test-pii", "Call me at 555-867-5309 to discuss."])
        .output()
        .expect("claudovka test-pii");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_lowercase();
    assert!(
        stdout.contains("phone"),
        "test-pii did not detect PHONE: {stdout}"
    );
}

/// `claudovka stop` when nothing is running must exit gracefully (no panic, no hard error).
#[test]
fn test_stop_when_not_running_is_graceful() {
    // Remove any lingering PID file first so we get a clean "not running" path.
    let pid_path = {
        let home = std::env::var("HOME").expect("HOME");
        std::path::PathBuf::from(home).join(".config/claudovka/claudovka.pid")
    };
    let _ = std::fs::remove_file(&pid_path); // ignore if absent

    let output = Command::new(binary_path())
        .arg("stop")
        .output()
        .expect("claudovka stop");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}").to_lowercase();

    // Either exits 0 or prints a "not running" message instead of panicking.
    assert!(
        output.status.success()
            || combined.contains("not running")
            || combined.contains("no pid"),
        "stop exited unexpectedly: status={}, output={combined}",
        output.status
    );
}

/// `claudovka uninstall --help` mentions --purge.
#[test]
fn test_uninstall_help_mentions_purge() {
    let output = Command::new(binary_path())
        .args(["uninstall", "--help"])
        .output()
        .expect("claudovka uninstall --help");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
    .to_lowercase();
    assert!(
        combined.contains("purge"),
        "uninstall --help does not mention --purge: {combined}"
    );
}
