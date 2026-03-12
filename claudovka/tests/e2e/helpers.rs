#![allow(dead_code)]
/// Shared test infrastructure for e2e process-level tests.
///
/// Each helper is pure std — no external crates required.
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

// ── Binary path ──────────────────────────────────────────────────────────────

/// Path to the claudovka binary being tested.
/// `CARGO_BIN_EXE_claudovka` is set by Cargo when running `cargo test`.
pub fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_claudovka"))
}

/// Default CA directory used by claudovka (matches `default_ca_dir()` in config.rs).
/// On macOS `dirs::config_dir()` returns `~/Library/Application Support/`;
/// on Linux it returns `~/.config/`.
pub fn default_ca_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME env var");
    #[cfg(target_os = "macos")]
    let base = PathBuf::from(&home).join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let base = PathBuf::from(&home).join(".config");
    base.join("claudovka/ca")
}

// ── Port allocation ───────────────────────────────────────────────────────────

/// Global atomic counter so parallel test threads get distinct port pairs
/// without a TOCTOU race.  Starts at 20_000 (well above ephemeral range and
/// away from claudovka's default ports 16440/16441/16443).
static NEXT_PORT: AtomicU16 = AtomicU16::new(20_000);

/// Reserve a TCP port that no other test in this binary will use.
pub fn alloc_port() -> u16 {
    NEXT_PORT.fetch_add(1, Ordering::SeqCst)
}

/// Reserve two consecutive ports (proxy + dashboard) atomically.
pub fn alloc_port_pair() -> (u16, u16) {
    let p = NEXT_PORT.fetch_add(2, Ordering::SeqCst);
    (p, p + 1)
}

/// Legacy: bind :0, get OS-assigned port, release.  Still useful when a single
/// test needs a one-off listener.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind port 0");
    listener.local_addr().expect("local_addr").port()
}

/// Poll until `port` on localhost accepts TCP connections, or `timeout` elapses.
/// Returns true if the port became ready, false if the timeout expired.
pub fn wait_for_port(port: u16, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(format!("127.0.0.1:{port}")).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

// ── Config writer ─────────────────────────────────────────────────────────────

/// Write a minimal claudovka config.toml into `dir`.
/// The config points the proxy and dashboard at the given ports and
/// keeps all storage under `dir` so tests don't touch the real data directory.
pub fn write_test_config(dir: &Path, proxy_port: u16, dashboard_port: u16) -> PathBuf {
    let logs_dir = dir.join("logs").to_string_lossy().to_string();
    let content = format!(
        "[proxy]\nlisten = \"127.0.0.1:{proxy_port}\"\ndashboard = \"127.0.0.1:{dashboard_port}\"\n\n[storage]\nlogs_dir = \"{logs_dir}\"\n"
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, content).expect("write test config");
    path
}

// ── Bare HTTP client ──────────────────────────────────────────────────────────

/// Send a GET request to `http://127.0.0.1:<port><path>` and return `(status, body)`.
pub fn http_get(port: u16, path: &str) -> Result<(u16, String), String> {
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("connect to :{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    let status = raw
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let body = raw
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// Send a POST request to `http://127.0.0.1:<port><path>` and return `(status, body)`.
pub fn http_post(port: u16, path: &str, body: &str) -> Result<(u16, String), String> {
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("connect to :{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| e.to_string())?;
    let status = raw
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    let resp_body = raw
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, resp_body))
}

// ── One-time init ─────────────────────────────────────────────────────────────

/// Run `claudovka init` exactly once per test-binary invocation.
/// Concurrent callers block until init completes (OnceLock guarantees this).
pub fn ensure_init() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let status = Command::new(binary_path())
            .arg("init")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("claudovka init");
        assert!(status.success(), "claudovka init failed in ensure_init");
    });
}

// ── ProxyHandle ───────────────────────────────────────────────────────────────

/// A running claudovka proxy process with isolated config.
/// The process is killed (and temp dir cleaned up) when this handle is dropped.
pub struct ProxyHandle {
    child: Child,
    pub proxy_port: u16,
    pub dashboard_port: u16,
    tmp_dir: Option<tempfile::TempDir>,
}

impl ProxyHandle {
    /// Spawn `claudovka start` with a fresh temp config (unique ports, temp storage).
    /// Blocks until the dashboard port accepts connections (up to 15 s).
    pub fn start() -> Self {
        // Run init once per test binary, not once per test, to avoid races.
        ensure_init();

        let tmp = tempfile::TempDir::new().expect("TempDir");
        let (proxy_port, dashboard_port) = alloc_port_pair();
        let config_path = write_test_config(tmp.path(), proxy_port, dashboard_port);

        let child = Command::new(binary_path())
            .args(["--config", config_path.to_str().unwrap(), "start"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn claudovka start");

        assert!(
            wait_for_port(dashboard_port, Duration::from_secs(15)),
            "dashboard port {dashboard_port} did not open within 15 s"
        );

        ProxyHandle {
            child,
            proxy_port,
            dashboard_port,
            tmp_dir: Some(tmp),
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // tmp_dir is dropped here, cleaning up the temp config
        drop(self.tmp_dir.take());
    }
}
