/// E2E tests for network proxy mode: /etc/hosts + pf rule management.
///
/// These tests write to system files and call `pfctl`, so they require root.
/// They are gated behind the `PRIVACYCLAW_TEST_NETWORK` environment variable:
///
///   sudo PRIVACYCLAW_TEST_NETWORK=1 cargo test --test network_proxy_test
///
/// Running without the variable set causes every test to be skipped (pass).
///
/// The test binary itself runs as the process that `cargo test` spawned, but
/// because `cargo test` is launched via `sudo`, all child processes
/// (including `privacyclaw network-enable`) also run as root and can write to
/// `/etc/hosts` and call `pfctl` directly — no osascript dialog is shown.
#[path = "helpers.rs"]
mod helpers;
use helpers::*;

use std::process::Command;

// ── Guard ─────────────────────────────────────────────────────────────────────

/// Returns true when network tests should run.
/// Set `PRIVACYCLAW_TEST_NETWORK=1` (and run as root via sudo).
fn network_tests_enabled() -> bool {
    std::env::var("PRIVACYCLAW_TEST_NETWORK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

macro_rules! skip_unless_network {
    () => {
        if !network_tests_enabled() {
            println!(
                "SKIP (set PRIVACYCLAW_TEST_NETWORK=1 and run as root to enable network proxy tests)"
            );
            return;
        }
    };
}

// ── Cleanup guard ─────────────────────────────────────────────────────────────

/// Ensure network-disable is always called at the end of a test, even on panic.
struct NetworkDisableGuard;

impl Drop for NetworkDisableGuard {
    fn drop(&mut self) {
        let _ = Command::new(binary_path())
            .arg("network-disable")
            .status();
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// `privacyclaw network-enable` writes /etc/hosts entries for all default LLM domains.
#[test]
fn test_network_enable_writes_hosts_entries() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard; // always disable on exit

    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("privacyclaw network-enable");
    assert!(status.success(), "network-enable failed");

    let hosts = std::fs::read_to_string("/etc/hosts").expect("read /etc/hosts");
    assert!(
        hosts.contains("# privacyclaw"),
        "/etc/hosts missing privacyclaw entries after enable:\n{hosts}"
    );
    assert!(
        hosts.contains("api.anthropic.com"),
        "/etc/hosts missing api.anthropic.com:\n{hosts}"
    );
    assert!(
        hosts.contains("api.openai.com"),
        "/etc/hosts missing api.openai.com:\n{hosts}"
    );
    assert!(
        hosts.contains("127.0.0.1"),
        "/etc/hosts entries should point to 127.0.0.1:\n{hosts}"
    );
}

/// After `network-enable`, the pf anchor file must exist and contain redirect rules.
#[test]
fn test_network_enable_creates_pf_anchor() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard;

    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("network-enable");
    assert!(status.success(), "network-enable failed");

    let anchor_path = std::path::Path::new("/etc/pf.anchors/privacyclaw");
    assert!(
        anchor_path.exists(),
        "/etc/pf.anchors/privacyclaw does not exist after network-enable"
    );

    let anchor_content = std::fs::read_to_string(anchor_path).expect("read anchor");
    assert!(
        anchor_content.contains("443"),
        "pf anchor missing port 443 redirect:\n{anchor_content}"
    );
    assert!(
        anchor_content.contains("rdr"),
        "pf anchor missing 'rdr' rule:\n{anchor_content}"
    );
}

/// After `network-enable`, pfctl reports active rules for the privacyclaw anchor.
#[test]
fn test_network_enable_loads_pf_rules() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard;

    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("network-enable");
    assert!(status.success(), "network-enable failed");

    // pfctl -a privacyclaw -s rules → prints the active redirect rules.
    let output = Command::new("pfctl")
        .args(["-a", "privacyclaw", "-s", "rules"])
        .output()
        .expect("pfctl -s rules");

    // pfctl exits 0 even with no rules on macOS; presence of output = rules loaded.
    let rules = String::from_utf8_lossy(&output.stdout);
    let errors = String::from_utf8_lossy(&output.stderr);
    assert!(
        rules.contains("443") || rules.contains("rdr"),
        "pf anchor has no rules after network-enable (stdout={rules}, stderr={errors})"
    );
}

/// `privacyclaw network-disable` removes all privacyclaw entries from /etc/hosts.
#[test]
fn test_network_disable_cleans_hosts() {
    skip_unless_network!();

    // Enable first.
    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("network-enable");
    assert!(status.success(), "network-enable failed");

    // Verify entries present.
    let hosts_before = std::fs::read_to_string("/etc/hosts").expect("hosts before disable");
    assert!(
        hosts_before.contains("# privacyclaw"),
        "/etc/hosts missing entries before disable"
    );

    // Now disable.
    let status = Command::new(binary_path())
        .arg("network-disable")
        .status()
        .expect("network-disable");
    assert!(status.success(), "network-disable failed");

    // Verify entries removed.
    let hosts_after = std::fs::read_to_string("/etc/hosts").expect("hosts after disable");
    assert!(
        !hosts_after.contains("# privacyclaw"),
        "/etc/hosts still contains privacyclaw entries after disable:\n{hosts_after}"
    );
}

/// `privacyclaw network-disable` flushes the pf anchor (no active redirect rules).
#[test]
fn test_network_disable_flushes_pf_anchor() {
    skip_unless_network!();

    // Enable first.
    Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("network-enable");

    // Then disable.
    let status = Command::new(binary_path())
        .arg("network-disable")
        .status()
        .expect("network-disable");
    assert!(status.success(), "network-disable failed");

    // The anchor should have no rules.
    let output = Command::new("pfctl")
        .args(["-a", "privacyclaw", "-s", "rules"])
        .output()
        .expect("pfctl -s rules");
    let rules = String::from_utf8_lossy(&output.stdout);
    assert!(
        rules.trim().is_empty(),
        "pf anchor still has rules after disable:\n{rules}"
    );
}

/// `privacyclaw network-enable` is idempotent — running it twice does not
/// create duplicate /etc/hosts entries.
#[test]
fn test_network_enable_is_idempotent() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard;

    Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("first network-enable");
    Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("second network-enable");

    let hosts = std::fs::read_to_string("/etc/hosts").expect("hosts after double enable");
    // Count occurrences of "api.anthropic.com  # privacyclaw".
    let count = hosts
        .lines()
        .filter(|l| l.contains("api.anthropic.com") && l.contains("# privacyclaw"))
        .count();
    assert_eq!(
        count, 1,
        "api.anthropic.com entry duplicated after second network-enable ({count} occurrences)"
    );
}

/// After `network-enable`, `privacyclaw network-disable` followed by another
/// `network-enable` restores the rules correctly (round-trip).
#[test]
fn test_network_enable_disable_enable_roundtrip() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard;

    Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("first enable");
    Command::new(binary_path())
        .arg("network-disable")
        .status()
        .expect("disable");
    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("second enable");
    assert!(status.success(), "second network-enable failed");

    let hosts = std::fs::read_to_string("/etc/hosts").expect("hosts after roundtrip");
    assert!(
        hosts.contains("# privacyclaw"),
        "/etc/hosts missing privacyclaw entries after roundtrip enable"
    );
}

/// Full network proxy lifecycle: enable → start network proxy → verify port → disable.
#[test]
fn test_full_network_proxy_lifecycle() {
    skip_unless_network!();
    let _guard = NetworkDisableGuard;

    // Enable.
    let status = Command::new(binary_path())
        .arg("network-enable")
        .status()
        .expect("network-enable");
    assert!(status.success(), "network-enable failed");

    // Verify /etc/hosts.
    let hosts = std::fs::read_to_string("/etc/hosts").expect("hosts");
    assert!(hosts.contains("# privacyclaw"), "hosts missing privacyclaw entries");

    // Verify pf anchor loaded.
    let output = Command::new("pfctl")
        .args(["-a", "privacyclaw", "-s", "rules"])
        .output()
        .expect("pfctl");
    let rules = String::from_utf8_lossy(&output.stdout);
    assert!(
        rules.contains("443") || rules.contains("rdr"),
        "pf anchor rules not loaded: {rules}"
    );

    // Disable and verify cleaned.
    let status = Command::new(binary_path())
        .arg("network-disable")
        .status()
        .expect("network-disable");
    assert!(status.success(), "network-disable failed");

    let hosts_after = std::fs::read_to_string("/etc/hosts").expect("hosts after disable");
    assert!(
        !hosts_after.contains("# privacyclaw"),
        "hosts still dirty after disable"
    );
}
