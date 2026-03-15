//! Manage /etc/hosts entries and macOS pf rules for the network proxy.
//!
//! Pure file-manipulation logic is in the `logic` sub-module and is fully testable
//! without any privilege escalation. The top-level `enable` / `disable` functions
//! perform the actual privileged writes via `osascript` on macOS.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const PRIVACYCLAW_TAG: &str = "# privacyclaw";

// ── Pure logic (testable without root / osascript) ───────────────────────────

/// Build the `/etc/hosts` lines for the given domains.
/// Each domain gets one line: `127.0.0.1  <domain>  # privacyclaw`.
/// Lines are skipped if an identical entry already exists in `existing_content`.
pub fn build_hosts_entries(domains: &[&str], existing_content: &str) -> String {
    let mut out = String::new();
    for domain in domains {
        let line = format!("127.0.0.1  {}  {}", domain, PRIVACYCLAW_TAG);
        // Idempotency: skip if exact line already present.
        if !existing_content.lines().any(|l| l.trim() == line.trim()) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Remove all lines that contain the privacyclaw tag from `content`.
pub fn remove_privacyclaw_lines(content: &str) -> String {
    content
        .lines()
        .filter(|l| !l.contains(PRIVACYCLAW_TAG))
        .fold(String::new(), |mut acc, l| { acc.push_str(l); acc.push('\n'); acc })
}

/// Returns `true` if `content` contains at least one privacyclaw-tagged line.
pub fn has_privacyclaw_entries(content: &str) -> bool {
    content.lines().any(|l| l.contains(PRIVACYCLAW_TAG))
}

/// Build the pf anchor content for the given proxy port.
pub fn build_pf_anchor(proxy_port: u16) -> String {
    format!(
        "rdr pass on lo0 inet  proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port {port}\n\
         rdr pass on lo0 inet6 proto tcp from any to ::1       port 443 -> ::1       port {port}\n",
        port = proxy_port
    )
}

/// Build the pf.conf include lines for the anchor.
pub fn build_pf_conf_include() -> String {
    format!(
        "rdr-anchor \"privacyclaw\"  {tag}\nload anchor \"privacyclaw\" from \"/etc/pf.anchors/privacyclaw\"  {tag}\n",
        tag = PRIVACYCLAW_TAG
    )
}

/// Remove privacyclaw include lines from pf.conf content.
pub fn remove_pf_conf_include(content: &str) -> String {
    remove_privacyclaw_lines(content)
}

/// Insert privacyclaw anchor declarations into pf.conf content at the correct position.
///
/// The `rdr-anchor` (translation) and `load anchor` lines are inserted immediately
/// before the first `anchor "..."` (filter) line, satisfying pf's required rule order:
/// options → normalization → queueing → translation → filtering.
///
/// Idempotent: returns content unchanged if the privacyclaw tag is already present.
pub fn insert_pf_conf_include(content: &str) -> String {
    if content.contains(PRIVACYCLAW_TAG) {
        return content.to_string();
    }

    let include = build_pf_conf_include();
    let mut result = String::new();
    let mut inserted = false;

    for line in content.lines() {
        // "anchor " (with trailing space or tab) identifies a filter anchor.
        // This distinguishes it from rdr-anchor / nat-anchor / dummynet-anchor.
        let trimmed = line.trim_start();
        if !inserted && (trimmed.starts_with("anchor ") || trimmed.starts_with("anchor\t")) {
            result.push_str(&include);
            inserted = true;
        }
        result.push_str(line);
        result.push('\n');
    }

    // Fallback: pf.conf has no filter anchor line — append at end.
    if !inserted {
        result.push_str(&include);
    }

    result
}

// ── Backup helpers ────────────────────────────────────────────────────────────

pub fn backup_dir() -> PathBuf {
    crate::config::default_config_dir().join("backup")
}

pub fn hosts_backup_path() -> PathBuf {
    backup_dir().join("hosts.bak")
}

pub fn pf_conf_backup_path() -> PathBuf {
    backup_dir().join("pf.conf.bak")
}

/// Snapshot a file to `dest` if `dest` does not already exist.
fn snapshot_file(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(()); // already backed up
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dest)
        .with_context(|| format!("failed to snapshot {:?} → {:?}", src, dest))?;
    Ok(())
}

// ── osascript privileged runner ───────────────────────────────────────────────

/// Returns true when the current process is running as root (UID 0).
/// Works on macOS and Linux without any external crate.
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
        .unwrap_or(false)
}

/// Run a shell script with elevated privileges.
///
/// - If the process is **already root** (e.g. launched via `sudo`): run directly.
/// - Otherwise on **macOS**: request admin credentials via `osascript`.
/// - On **Linux / CI**: execute directly (assumes root or permissive environment).
#[cfg(target_os = "macos")]
pub(crate) fn run_privileged(script: &str) -> Result<()> {
    if is_root() {
        // Already root — skip the osascript dialog and run directly.
        let status = std::process::Command::new("sh")
            .args(["-c", script])
            .status()
            .context("failed to run privileged script as root")?;
        if !status.success() {
            anyhow::bail!("privileged script exited with status {}", status);
        }
        return Ok(());
    }
    // Interactive mode: request admin credentials via a single dialog.
    let escaped = script.replace('\\', "\\\\").replace('"', "\\\"");
    let apple_script = format!(
        "do shell script \"{}\" with administrator privileges",
        escaped
    );
    let status = std::process::Command::new("osascript")
        .args(["-e", &apple_script])
        .status()
        .context("failed to launch osascript")?;
    if !status.success() {
        anyhow::bail!("osascript exited with status {}", status);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn run_privileged(script: &str) -> Result<()> {
    // On Linux / CI: execute directly (assumes already root or test environment).
    let status = std::process::Command::new("sh")
        .args(["-c", script])
        .status()
        .context("failed to run privileged script")?;
    if !status.success() {
        anyhow::bail!("privileged script exited with status {}", status);
    }
    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Enable network proxy: insert privacyclaw anchor into pf.conf (before the filter
/// anchor line) and write /etc/hosts entries. All writes go through one privileged
/// script so the user sees a single admin dialog.
pub fn enable(domains: &[&str], proxy_port: u16) -> Result<()> {
    snapshot_file(Path::new("/etc/hosts"), &hosts_backup_path())?;
    snapshot_file(Path::new("/etc/pf.conf"), &pf_conf_backup_path())?;

    let hosts_entries = build_hosts_entries(
        domains,
        &std::fs::read_to_string("/etc/hosts").unwrap_or_default(),
    );
    let pf_anchor = build_pf_anchor(proxy_port);
    // Insert privacyclaw's rdr-anchor before the first filter anchor line.
    let new_pf_conf = insert_pf_conf_include(
        &std::fs::read_to_string("/etc/pf.conf").unwrap_or_default(),
    );

    let anchor_path = "/etc/pf.anchors/privacyclaw";
    let script = format!(
        "printf '{anchor}' > {anchor_path} && \
         printf '{pf_conf}' > /etc/pf.conf && \
         pfctl -ef /etc/pf.conf 2>/dev/null; \
         pfctl -a privacyclaw -f {anchor_path} && \
         printf '{hosts}' >> /etc/hosts",
        anchor = pf_anchor.replace('\'', "'\\''"),
        anchor_path = anchor_path,
        pf_conf = new_pf_conf.replace('\'', "'\\''"),
        hosts = hosts_entries.replace('\'', "'\\''"),
    );

    run_privileged(&script)?;
    tracing::warn!(domains = ?domains, proxy_port, "network proxy enabled");
    Ok(())
}

/// Disable network proxy: flush the privacyclaw pf anchor, restore pf.conf, and
/// remove /etc/hosts entries.
pub fn disable() -> Result<()> {
    let hosts_content = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
    let pf_conf_content = std::fs::read_to_string("/etc/pf.conf").unwrap_or_default();
    let cleaned_hosts = remove_privacyclaw_lines(&hosts_content);
    let cleaned_pf_conf = remove_pf_conf_include(&pf_conf_content);

    let script = format!(
        "pfctl -a privacyclaw -F all 2>/dev/null; \
         printf '{pf_conf}' > /etc/pf.conf && \
         rm -f /etc/pf.anchors/privacyclaw && \
         pfctl -ef /etc/pf.conf 2>/dev/null; \
         printf '{hosts}' > /etc/hosts",
        pf_conf = cleaned_pf_conf.replace('\'', "'\\''"),
        hosts = cleaned_hosts.replace('\'', "'\\''"),
    );

    run_privileged(&script)?;

    let _ = std::fs::remove_file(hosts_backup_path());
    let _ = std::fs::remove_file(pf_conf_backup_path());

    tracing::warn!("network proxy disabled");
    Ok(())
}

/// Returns true if privacyclaw entries are currently present in /etc/hosts.
pub fn is_enabled() -> bool {
    std::fs::read_to_string("/etc/hosts")
        .map(|c| has_privacyclaw_entries(&c))
        .unwrap_or(false)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // 6.T1 — build_hosts_entries produces correct entries, no duplicates.
    #[test]
    fn test_build_hosts_entries_no_duplicates() {
        let domains = ["api.anthropic.com", "api.openai.com"];
        let result = build_hosts_entries(&domains, "");
        assert!(result.contains("127.0.0.1  api.anthropic.com  # privacyclaw"));
        assert!(result.contains("127.0.0.1  api.openai.com  # privacyclaw"));
        // Two domains → two lines.
        assert_eq!(result.lines().count(), 2);
    }

    // 6.T1 — no extra lines for unrelated domains.
    #[test]
    fn test_build_hosts_entries_only_given_domains() {
        let result = build_hosts_entries(&["api.anthropic.com"], "");
        assert!(!result.contains("openai"));
    }

    // 6.T4 — idempotency: existing entry is not duplicated.
    #[test]
    fn test_build_hosts_entries_idempotent() {
        let existing = "127.0.0.1  api.anthropic.com  # privacyclaw\n";
        let result = build_hosts_entries(&["api.anthropic.com", "api.openai.com"], existing);
        // anthropic is already present → only openai added.
        assert!(!result.contains("api.anthropic.com"));
        assert!(result.contains("api.openai.com"));
        assert_eq!(result.lines().count(), 1);
    }

    // 6.T2 — remove_privacyclaw_lines removes tagged lines, preserves others.
    #[test]
    fn test_remove_privacyclaw_lines() {
        let content = "127.0.0.1 localhost\n\
                       127.0.0.1  api.anthropic.com  # privacyclaw\n\
                       255.255.255.255 broadcasthost\n\
                       127.0.0.1  api.openai.com  # privacyclaw\n";
        let result = remove_privacyclaw_lines(content);
        assert!(result.contains("127.0.0.1 localhost"));
        assert!(result.contains("broadcasthost"));
        assert!(!result.contains("api.anthropic.com"));
        assert!(!result.contains("api.openai.com"));
        assert!(!result.contains(PRIVACYCLAW_TAG));
    }

    // 6.T3 — has_privacyclaw_entries returns true when tag present.
    #[test]
    fn test_has_privacyclaw_entries_true() {
        let content = "127.0.0.1  api.anthropic.com  # privacyclaw\n";
        assert!(has_privacyclaw_entries(content));
    }

    // 6.T3 — has_privacyclaw_entries returns false when tag absent.
    #[test]
    fn test_has_privacyclaw_entries_false() {
        let content = "127.0.0.1 localhost\n255.255.255.255 broadcasthost\n";
        assert!(!has_privacyclaw_entries(content));
    }

    // 6.T5 — domain list change: add domain → new entry; remove domain → entry gone.
    #[test]
    fn test_domain_list_change() {
        let original_domains = ["api.anthropic.com"];
        let existing = build_hosts_entries(&original_domains, "");

        // Add api.openai.com.
        let updated = existing.clone() + &build_hosts_entries(&["api.anthropic.com", "api.openai.com"], &existing);
        assert!(updated.contains("api.openai.com"));

        // Remove api.anthropic.com by rebuilding from scratch (simulate config change).
        let new_domains = ["api.openai.com"];
        let cleaned = remove_privacyclaw_lines(&existing);
        let rebuilt = build_hosts_entries(&new_domains, &cleaned);
        assert!(!rebuilt.contains("api.anthropic.com"));
        assert!(rebuilt.contains("api.openai.com"));
    }

    // 6.T6 — build_pf_anchor produces correct rdr rule for given port.
    #[test]
    fn test_build_pf_anchor_contains_port() {
        let anchor = build_pf_anchor(16441);
        assert!(anchor.contains("port 443 -> 127.0.0.1 port 16441"));
        assert!(anchor.contains("rdr pass on lo0 inet"));
        assert!(anchor.contains("rdr pass on lo0 inet6"));
    }

    #[test]
    fn test_build_pf_anchor_custom_port() {
        let anchor = build_pf_anchor(9999);
        assert!(anchor.contains("port 9999"));
        assert!(!anchor.contains("port 16441"));
    }

    // pf.conf include round-trip: add then remove leaves no privacyclaw lines.
    #[test]
    fn test_pf_conf_include_roundtrip() {
        let original = "scrub-anchor \"com.apple/*\"\nnat-anchor \"com.apple/*\"\n";
        let with_include = original.to_string() + &build_pf_conf_include();
        assert!(with_include.contains(PRIVACYCLAW_TAG));
        let restored = remove_pf_conf_include(&with_include);
        assert!(!restored.contains(PRIVACYCLAW_TAG));
        assert!(restored.contains("scrub-anchor"));
    }

    // insert_pf_conf_include — privacyclaw rdr-anchor is placed before filter anchor.
    #[test]
    fn test_insert_pf_conf_include_position() {
        let original = "scrub-anchor \"com.apple/*\"\n\
                        nat-anchor \"com.apple/*\"\n\
                        rdr-anchor \"com.apple/*\"\n\
                        anchor \"com.apple/*\"\n\
                        load anchor \"com.apple/*\" from \"/etc/pf.anchors/com.apple\"\n";
        let result = insert_pf_conf_include(original);

        // Compare line numbers so we're not confused by substring matches
        // (e.g. "anchor" appearing inside "rdr-anchor").
        let line_of = |needle: &str| -> usize {
            result
                .lines()
                .enumerate()
                .find(|(_, l)| l.trim_start().starts_with(needle))
                .map(|(i, _)| i)
                .unwrap_or(usize::MAX)
        };
        let privacyclaw_line = line_of("rdr-anchor \"privacyclaw\"");
        let filter_anchor_line = line_of("anchor \"com.apple/*\"");
        assert!(
            privacyclaw_line < filter_anchor_line,
            "privacyclaw rdr-anchor (line {privacyclaw_line}) must precede filter anchor (line {filter_anchor_line})"
        );
        assert!(result.contains(PRIVACYCLAW_TAG));
        // Original lines all preserved.
        assert!(result.contains("scrub-anchor"));
        assert!(result.contains("load anchor \"com.apple/*\""));
    }

    // insert_pf_conf_include — idempotent: calling twice produces identical output.
    #[test]
    fn test_insert_pf_conf_include_idempotent() {
        let original = "scrub-anchor \"com.apple/*\"\n\
                        anchor \"com.apple/*\"\n";
        let once = insert_pf_conf_include(original);
        let twice = insert_pf_conf_include(&once);
        assert_eq!(once, twice, "insert_pf_conf_include must be idempotent");
        // Exactly two tagged lines (rdr-anchor + load anchor).
        assert_eq!(once.matches(PRIVACYCLAW_TAG).count(), 2);
    }

    // insert_pf_conf_include — no filter anchor line → appends at end.
    #[test]
    fn test_insert_pf_conf_include_no_anchor_line() {
        let original = "scrub-anchor \"com.apple/*\"\n";
        let result = insert_pf_conf_include(original);
        assert!(result.contains(PRIVACYCLAW_TAG));
        assert!(result.contains("scrub-anchor"));
    }

    // Full round-trip: insert then remove leaves original content intact.
    #[test]
    fn test_pf_conf_insert_remove_roundtrip() {
        let original = "scrub-anchor \"com.apple/*\"\n\
                        nat-anchor \"com.apple/*\"\n\
                        rdr-anchor \"com.apple/*\"\n\
                        anchor \"com.apple/*\"\n\
                        load anchor \"com.apple/*\" from \"/etc/pf.anchors/com.apple\"\n";
        let with_include = insert_pf_conf_include(original);
        let restored = remove_pf_conf_include(&with_include);
        assert!(!restored.contains(PRIVACYCLAW_TAG));
        assert!(restored.contains("scrub-anchor"));
        assert!(restored.contains("load anchor \"com.apple/*\""));
    }
}
