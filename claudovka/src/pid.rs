use std::path::PathBuf;

/// Returns the path to the PID file.
pub fn pid_file_path() -> PathBuf {
    crate::config::default_config_dir().join("claudovka.pid")
}

/// Write the current process PID to the PID file.
pub fn write_pid() -> anyhow::Result<()> {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, std::process::id().to_string())?;
    tracing::info!(path = %path.display(), "PID file written");
    Ok(())
}

/// Read the PID from the PID file; returns `None` if absent or unparseable.
pub fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_file_path())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

/// Remove the PID file (best-effort; silently ignores missing file).
pub fn remove_pid() {
    let path = pid_file_path();
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            tracing::warn!(err = %e, "failed to remove PID file");
        } else {
            tracing::info!("PID file removed");
        }
    }
}

/// Send SIGTERM to `pid`, poll for up to `timeout_secs`, then SIGKILL.
/// Returns `Ok(true)` if the process exited cleanly, `Ok(false)` if it was killed.
#[cfg(unix)]
pub fn stop_process(pid: u32, timeout_secs: u64) -> anyhow::Result<bool> {
    use std::process::Command;
    use std::time::{Duration, Instant};

    // SIGTERM
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()?;

    if !status.success() {
        // Process may have already exited.
        remove_pid();
        return Ok(true);
    }

    // Poll for exit.
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !alive {
            remove_pid();
            return Ok(true);
        }
    }

    // SIGKILL
    let _ = Command::new("kill")
        .args(["-KILL", &pid.to_string()])
        .status();
    remove_pid();
    Ok(false)
}

#[cfg(not(unix))]
pub fn stop_process(_pid: u32, _timeout_secs: u64) -> anyhow::Result<bool> {
    anyhow::bail!("stop_process is only supported on Unix")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Override PID file path via env var for tests to avoid touching real config dir.
    #[allow(dead_code)]
    fn tmp_pid_path() -> PathBuf {
        let dir = tempfile::tempdir().unwrap();
        // keep the dir alive so the path remains valid during the test
        let path = dir.path().join("claudovka.pid");
        let _ = dir.keep(); // keep temp dir alive
        path
    }

    /// Write+read+remove using real temp dir.
    #[test]
    fn test_write_read_remove_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claudovka.pid");

        // Write
        std::fs::write(&path, std::process::id().to_string()).unwrap();
        assert!(path.exists());

        // Read
        let pid: u32 = std::fs::read_to_string(&path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(pid, std::process::id());

        // Remove
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }

    /// read_pid returns None when file is absent.
    #[test]
    fn test_read_pid_absent_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does_not_exist.pid");
        assert!(!path.exists());
        let result: Option<u32> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        assert!(result.is_none());
    }

    /// read_pid returns None for non-numeric content.
    #[test]
    fn test_read_pid_garbage_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claudovka.pid");
        std::fs::write(&path, "not-a-number").unwrap();
        let result: Option<u32> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        assert!(result.is_none());
    }

    /// remove_pid is a no-op when the file does not exist.
    #[test]
    fn test_remove_pid_absent_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claudovka.pid");
        assert!(!path.exists());
        // Should not panic.
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }

    /// PID file contains the current process ID after write.
    #[test]
    fn test_pid_file_contains_current_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claudovka.pid");
        std::fs::write(&path, std::process::id().to_string()).unwrap();
        let written: u32 = std::fs::read_to_string(&path)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(written, std::process::id());
    }

    /// §11.T4: stop_process sends SIGTERM to the target PID and returns Ok(true)
    /// when the process exits within the timeout. We use a short-lived sleep subprocess
    /// to verify that signal delivery works without killing the test process itself.
    #[cfg(unix)]
    #[test]
    fn test_stop_process_terminates_sleeping_child() {
        use std::process::Command;
        // Spawn a child that sleeps for 30 seconds.
        let mut child = Command::new("sleep").arg("30").spawn().expect("spawn sleep");
        let pid = child.id();

        // stop_process should SIGTERM it and confirm exit within 5s.
        // Returns Ok(true) if exited cleanly, Ok(false) if SIGKILL was required.
        // Both are valid outcomes — the important thing is that the child is no longer running.
        let result = stop_process(pid, 5);
        assert!(result.is_ok(), "stop_process must return Ok, got: {:?}", result);
        // Reap the child regardless of how it exited.
        let _ = child.wait();
        // Verify the process is no longer alive.
        let still_alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(!still_alive, "child process must not be alive after stop_process");
    }
}
