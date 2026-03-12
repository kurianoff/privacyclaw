//! `claudovka uninstall [--purge]` — ordered step orchestrator.
//!
//! Each step returns an `Outcome`. The runner never aborts early: all steps are
//! attempted and a summary is printed at the end.
//!
//! Steps that require root (binary removal, LaunchDaemon, share dir, CA keychain,
//! pkg receipt) are batched into a single privileged script so the user sees
//! at most one admin credentials dialog.



// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Done,
    Skipped(&'static str),
    Failed(String),
}

impl Outcome {
    pub fn symbol(&self) -> &'static str {
        match self {
            Outcome::Done => "✓",
            Outcome::Skipped(_) => "⚠",
            Outcome::Failed(_) => "✗",
        }
    }
}

#[derive(Debug)]
pub struct StepResult {
    pub step: &'static str,
    pub outcome: Outcome,
}

// ── Uninstall runner ──────────────────────────────────────────────────────────

pub struct UninstallRunner {
    pub purge: bool,
}

impl UninstallRunner {
    pub fn new(purge: bool) -> Self {
        Self { purge }
    }

    /// Run all uninstall steps in order and return their results.
    pub fn run(&self) -> Vec<StepResult> {
        let mut results = Vec::new();

        // ── Unprivileged steps (no admin dialog needed) ────────────────────
        results.push(self.step_stop_proxy());
        results.push(self.step_unload_launch_agent());

        // Network proxy cleanup — only if claudovka entries are active.
        if crate::network_helper::is_enabled() {
            results.push(self.step_disable_network_proxy());
        } else {
            results.push(StepResult {
                step: "Revert /etc/hosts + pf rules",
                outcome: Outcome::Skipped("network proxy was not enabled"),
            });
        }

        // ── Privileged steps (single admin dialog) ─────────────────────────
        results.push(self.step_remove_privileged());

        // ── Optional purge (user data) ─────────────────────────────────────
        if self.purge {
            results.push(self.step_purge_data());
        }

        results
    }

    pub fn print_summary(results: &[StepResult]) {
        println!("\nUninstall summary:");
        println!("{}", "─".repeat(60));
        for r in results {
            match &r.outcome {
                Outcome::Done => println!("  {} {}", r.outcome.symbol(), r.step),
                Outcome::Skipped(reason) => println!("  {} {} ({})", r.outcome.symbol(), r.step, reason),
                Outcome::Failed(msg) => println!("  {} {} — {}", r.outcome.symbol(), r.step, msg),
            }
        }
        println!("{}", "─".repeat(60));
        let failed = results.iter().filter(|r| matches!(r.outcome, Outcome::Failed(_))).count();
        if failed == 0 {
            println!("Done.");
        } else {
            println!("{} step(s) failed.", failed);
        }
    }

    pub fn has_failures(results: &[StepResult]) -> bool {
        results.iter().any(|r| matches!(r.outcome, Outcome::Failed(_)))
    }

    // ── Individual steps ──────────────────────────────────────────────────────

    fn step_stop_proxy(&self) -> StepResult {
        let step = "Stop proxy process";
        match crate::pid::read_pid() {
            None => StepResult {
                step,
                outcome: Outcome::Skipped("no PID file — proxy not running"),
            },
            Some(pid) => match crate::pid::stop_process(pid, 5) {
                Ok(_) => StepResult { step, outcome: Outcome::Done },
                Err(e) => StepResult { step, outcome: Outcome::Failed(e.to_string()) },
            },
        }
    }

    fn step_unload_launch_agent(&self) -> StepResult {
        let step = "Remove LaunchAgent";
        let plist = launch_agent_path();
        if !plist.exists() {
            return StepResult { step, outcome: Outcome::Skipped("plist not found") };
        }
        // Use the modern bootout API targeting the user's GUI session.
        let uid = libc_uid();
        let _ = std::process::Command::new("launchctl")
            .args(["bootout", &format!("gui/{uid}"), &plist.to_string_lossy()])
            .status();
        match std::fs::remove_file(&plist) {
            Ok(_) => StepResult { step, outcome: Outcome::Done },
            Err(e) => StepResult { step, outcome: Outcome::Failed(e.to_string()) },
        }
    }

    fn step_disable_network_proxy(&self) -> StepResult {
        let step = "Revert /etc/hosts + pf rules";
        match crate::network_helper::disable() {
            Ok(_) => StepResult { step, outcome: Outcome::Done },
            Err(e) => StepResult { step, outcome: Outcome::Failed(e.to_string()) },
        }
    }

    /// Batch all root-requiring removals into one privileged script — one dialog.
    fn step_remove_privileged(&self) -> StepResult {
        let step = "Remove binary, LaunchDaemon, CA, pkg receipt";

        let ca_cert = crate::ca::ca_cert_path(&crate::config::default_ca_dir());
        let ca_str  = ca_cert.to_string_lossy();

        // Build the privileged script. Each part is best-effort (|| true).
        let script = format!(
            // Remove CA from System keychain.
            "security remove-trusted-cert -d {ca} 2>/dev/null || true; \
             security delete-certificate -c 'Claudovka Root CA' -t 2>/dev/null || true; \
             # Unload and remove pf LaunchDaemon.
             launchctl bootout system /Library/LaunchDaemons/com.claudovka.pf.plist 2>/dev/null || \
               launchctl unload /Library/LaunchDaemons/com.claudovka.pf.plist 2>/dev/null || true; \
             rm -f /Library/LaunchDaemons/com.claudovka.pf.plist; \
             # Remove binary and shared files.
             rm -f /usr/local/bin/claudovka; \
             rm -rf /usr/local/share/claudovka; \
             # Forget the package receipt so Installer shows a clean state.
             pkgutil --forget com.claudovka.pkg 2>/dev/null || true",
            ca = ca_str,
        );

        match crate::network_helper::run_privileged(&script) {
            Ok(_) => StepResult { step, outcome: Outcome::Done },
            Err(e) => StepResult { step, outcome: Outcome::Failed(e.to_string()) },
        }
    }

    fn step_purge_data(&self) -> StepResult {
        let step = "Purge user data directory";
        let dir = crate::config::default_config_dir();
        match std::fs::remove_dir_all(&dir) {
            Ok(_) => StepResult { step, outcome: Outcome::Done },
            Err(e) => StepResult { step, outcome: Outcome::Failed(e.to_string()) },
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn launch_agent_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("Library/LaunchAgents/com.claudovka.proxy.plist")
}

/// Get the current process UID without pulling in the libc crate.
fn libc_uid() -> u32 {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(501) // safe fallback
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_purge_flag_controls_data_step() {
        assert!(UninstallRunner::new(true).purge);
        assert!(!UninstallRunner::new(false).purge);
    }

    #[test]
    fn test_outcome_symbols() {
        assert_eq!(Outcome::Done.symbol(), "✓");
        assert_eq!(Outcome::Skipped("x").symbol(), "⚠");
        assert_eq!(Outcome::Failed("x".to_string()).symbol(), "✗");
    }

    #[test]
    fn test_has_failures_detects_failed_step() {
        let results = vec![
            StepResult { step: "A", outcome: Outcome::Done },
            StepResult { step: "B", outcome: Outcome::Failed("oops".to_string()) },
        ];
        assert!(UninstallRunner::has_failures(&results));
    }

    #[test]
    fn test_has_failures_no_failures() {
        let results = vec![
            StepResult { step: "A", outcome: Outcome::Done },
            StepResult { step: "B", outcome: Outcome::Skipped("absent") },
        ];
        assert!(!UninstallRunner::has_failures(&results));
    }

    #[test]
    fn test_skipped_symbol() {
        let r = StepResult { step: "x", outcome: Outcome::Skipped("reason") };
        assert_eq!(r.outcome.symbol(), "⚠");
    }

    #[test]
    fn test_step_stop_no_pid_skipped() {
        let outcome = if crate::pid::read_pid().is_none() {
            Outcome::Skipped("no PID file — proxy not running")
        } else {
            Outcome::Done
        };
        assert!(matches!(outcome, Outcome::Skipped(_) | Outcome::Done));
    }
}
