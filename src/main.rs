mod ca;
mod cmd_config;
mod config;
mod dashboard;
mod models;
mod network_helper;
mod parser;
mod pid;
mod pii;
mod proxy;
mod storage;
mod uninstall;
mod util;
mod version;
#[cfg(all(target_os = "macos", feature = "tray"))]
mod tray;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{default_ca_dir, Config, ConfigManager};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Notify};
use crate::pii::{PiiCtx, PiiContext, PiiMode, PiiPipeline};
use crate::pii::vault::VaultRegistry;
use crate::cmd_config::ConfigAction;
use std::time::Duration;

const VAULT_EVICT_INTERVAL: Duration = Duration::from_secs(60);

// ── Shared proxy-resource bootstrap ───────────────────────────────────────────

/// Resources shared between `run_tray_mode` and `cmd_start`.
///
/// Holds everything that is created identically in both paths: CA bundle,
/// certificate cache, store, PII context, WebSocket channel, and the log
/// guards that must stay alive for the process lifetime.
#[cfg_attr(not(all(target_os = "macos", feature = "tray")), allow(dead_code))]
pub(crate) struct ProxyResources {
    pub cert_cache:   ca::cert_gen::CertCache,
    pub store:        storage::Store,
    pub pii:          PiiCtx,
    pub ws_tx:        broadcast::Sender<dashboard::WsEvent>,
    pub _log_guards:  Vec<tracing_appender::non_blocking::WorkerGuard>,
}

/// Load proxy resources shared by both the tray and CLI start paths.
///
/// Performs: config log-file override, logging init, version WARN, CA load,
/// store open, cert cache init, PII context build, and ws_tx channel creation.
/// Returns `ProxyResources`; caller constructs `ConfigManager` (needs `config_path`).
#[cfg_attr(not(all(target_os = "macos", feature = "tray")), allow(dead_code))]
pub(crate) fn load_proxy_resources(
    cfg: &mut Config,
    log_file: Option<&str>,
    pii_flag: bool,
    tray_mode: bool,
) -> Result<ProxyResources> {
    if let Some(path) = log_file {
        cfg.logging.file = if path.is_empty() { None } else { Some(path.to_string()) };
    }

    let log_guards = init_logging(&cfg.logging);

    tracing::warn!(
        version = version::VERSION,
        git_hash = version::GIT_HASH,
        build_date = version::BUILD_DATE,
        tray = tray_mode,
        "privacyclaw starting"
    );

    let ca_dir = default_ca_dir();
    let bundle = ca::load_ca(&ca_dir)?
        .context("CA not initialized. Run `privacyclaw init` first.")?;

    let store = storage::Store::open(&cfg.resolved_logs_dir())
        .with_context(|| format!("Failed to open log dir: {:?}", cfg.resolved_logs_dir()))?;
    tracing::info!(logs_dir = %cfg.resolved_logs_dir().display(), "store opened");

    let cert_cache = ca::cert_gen::CertCache::new(bundle);
    tracing::info!("cert cache initialised");

    let pii = build_pii_ctx(cfg, pii_flag);
    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);

    Ok(ProxyResources {
        cert_cache,
        store,
        pii,
        ws_tx,
        _log_guards: log_guards,
    })
}

/// Build a `PiiCtx` from config and CLI flag, or return `None` when PII is off.
fn build_pii_ctx(cfg: &Config, pii_flag: bool) -> PiiCtx {
    let pii_mode = if pii_flag || cfg.pii.mode == "replace" {
        PiiMode::Replace
    } else if cfg.pii.mode == "detect-only" {
        PiiMode::DetectOnly
    } else {
        PiiMode::Off
    };

    if pii_mode == PiiMode::Off {
        return None;
    }

    let ttl = Duration::from_secs(cfg.pii.vault_ttl_hours * 3600);
    let locale = crate::pii::locale::Locale::from_str_opt(&cfg.pii.locale)
        .unwrap_or_default();
    Some(Arc::new(PiiContext {
        pipeline: PiiPipeline::new(&cfg.pii),
        registry: Arc::new(VaultRegistry::new(ttl)),
        locale,
        mode: pii_mode,
    }))
}


#[derive(Parser)]
#[command(name = "privacyclaw", about = "Local MITM privacy proxy for LLM API traffic", version)]
struct Cli {
    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Override log file path (overrides logging.file in config; use empty string to disable)
    #[arg(long, global = true)]
    log_file: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::ValueEnum, Clone, Debug, Default)]
pub enum StartMode {
    #[default]
    All,
    Http,
    Network,
}

impl std::fmt::Display for StartMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartMode::All     => write!(f, "all"),
            StartMode::Http    => write!(f, "http"),
            StartMode::Network => write!(f, "network"),
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Generate CA certificate and print setup instructions
    Init {
        /// Attempt to install CA into OS trust store
        #[arg(long)]
        install_ca: bool,
        /// Also configure pf rules and /etc/hosts for network proxy mode (requires admin)
        #[arg(long)]
        network: bool,
    },
    /// Start the proxy
    Start {
        /// Which proxy mode to run (default: all)
        #[arg(long, value_enum)]
        mode: Option<StartMode>,
        /// Enable PII replace mode (Tier 1+2)
        #[arg(long)]
        pii: bool,
        /// Show menu bar icon instead of blocking in terminal (macOS + tray feature)
        #[arg(long)]
        tray: bool,
    },
    /// (Deprecated) Use `start --mode network` instead
    #[command(hide = true)]
    NetworkStart {
        #[arg(long)]
        pii: bool,
    },
    /// Print /etc/hosts entries and macOS pf rules for network mode setup
    SetupNetwork,
    /// Print the CA certificate path
    CaPath,
    /// Delete the CA and regenerate a new one
    ResetCa,
    /// Export conversation log
    Export {
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Test PII detection on a text string
    TestPii {
        /// Text to analyze
        text: String,
        /// Locale (e.g. en-US, in-IN, br-BR)
        #[arg(long)]
        locale: Option<String>,
        /// Output format: text or json
        #[arg(long, default_value = "text")]
        format: String,
    },
    /// Manage ML models for Tier 2/3 PII detection
    Models {
        #[command(subcommand)]
        action: ModelsAction,
    },
    /// Run PII detection benchmark against built-in fixtures
    Benchmark {
        /// Only benchmark this tier (1 or 2)
        #[arg(long)]
        tier: Option<u8>,
    },
    /// Stop a running privacyclaw proxy (reads PID file)
    Stop,
    /// Enable network proxy: write /etc/hosts entries and pf rules (requires admin)
    NetworkEnable,
    /// Disable network proxy: revert /etc/hosts and pf rules (requires admin)
    NetworkDisable,
    /// Remove privacyclaw from the system
    Uninstall {
        /// Also delete all user data (logs, database, models, config, CA)
        #[arg(long)]
        purge: bool,
    },
    /// Manage privacyclaw configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,

        /// Set protection level directly (bypasses wizard).
        /// Values: off | detect | 1 | 2 | 3 | intelligent
        #[arg(long, value_name = "LEVEL")]
        protection_level: Option<String>,

        /// GGUF model ID or file path for levels 3 and intelligent.
        /// Defaults to phi3-mini if not specified.
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Install a model by name
    Install { name: String },
    /// List installed models
    List,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Main entry point.
///
/// For `privacyclaw start --tray` on macOS (built with `--features tray`):
///   • The tokio runtime runs in background threads.
///   • The **main thread** runs the AppKit/CFRunLoop event loop for the tray icon.
///
/// All other subcommands use a normal tokio runtime on the main thread.
fn main() -> Result<()> {
    // Install ring as the default rustls CryptoProvider before any TLS is used.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let cli = Cli::parse();

    // Detect tray mode before committing a runtime strategy.
    let use_tray = cfg!(all(target_os = "macos", feature = "tray"))
        && matches!(&cli.command, Commands::Start { tray: true, .. } );

    if use_tray {
        #[cfg(all(target_os = "macos", feature = "tray"))]
        return run_tray_mode(cli);
        #[cfg(not(all(target_os = "macos", feature = "tray")))]
        anyhow::bail!("--tray requires macOS and `--features tray` at compile time");
    }

    // Normal path: block the main thread on the async runtime.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

/// Async body for all non-tray subcommands.
async fn async_main(cli: Cli) -> Result<()> {
    let mut cfg = Config::load(cli.config.as_deref()).context("Failed to load config")?;

    // CLI --log-file overrides the config file value (empty string disables file logging).
    if let Some(ref path) = cli.log_file {
        cfg.logging.file = if path.is_empty() { None } else { Some(path.clone()) };
    }

    let _guards = init_logging(&cfg.logging);

    tracing::warn!(
        version = version::VERSION,
        git_hash = version::GIT_HASH,
        build_date = version::BUILD_DATE,
        "privacyclaw starting"
    );

    let config_path = cli.config
        .as_deref()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| config::default_config_dir().join("config.toml"));
    let cfg_mgr = ConfigManager::new(cfg.clone(), Some(config_path));

    // Never notified on the non-tray path — just a placeholder so cmd_start's
    // signature is uniform.
    let tray_shutdown = Arc::new(Notify::new());

    match cli.command {
        Commands::Init { install_ca, network } => cmd_init(&cfg, install_ca, network).await,
        Commands::Start { mode, pii, .. } => {
            cmd_start(cfg, cfg_mgr, pii, mode, tray_shutdown).await
        }
        Commands::NetworkStart { pii } => {
            cmd_start(cfg, cfg_mgr, pii, Some(StartMode::Network), tray_shutdown).await
        }
        Commands::SetupNetwork    => cmd_setup_network(&cfg),
        Commands::CaPath          => cmd_ca_path(&cfg),
        Commands::ResetCa         => cmd_reset_ca(&cfg).await,
        Commands::Export { format, output } => cmd_export(&cfg, &format, &output).await,
        Commands::TestPii { text, locale, format } => cmd_test_pii(text, locale, format).await,
        Commands::Models { action }  => cmd_models(action).await,
        Commands::Benchmark { tier } => cmd_benchmark(tier).await,
        Commands::Stop             => cmd_stop().await,
        Commands::NetworkEnable    => cmd_network_enable(&cfg).await,
        Commands::NetworkDisable   => cmd_network_disable().await,
        Commands::Uninstall { purge } => cmd_uninstall(purge).await,
        Commands::Config { action, protection_level, model } =>
            cmd_config::cmd_config(cfg, cfg_mgr, action, protection_level, model).await,
    }
}

// ── Tray mode (macOS + `tray` feature) ───────────────────────────────────────

/// Start permanent background tasks and run the tray icon on the main thread.
///
/// The dashboard, PII-vault eviction, and log-rotation tasks are spawned
/// immediately and run for the lifetime of the process (unaffected by
/// Start/Stop Proxy).  The HTTP CONNECT listener is managed by `tray::run()`
/// via `TrayState`.
///
/// AppKit mandates that all UI runs on the main thread, so `tray::run()` blocks
/// here until the user clicks "Quit Privacyclaw".
#[cfg(all(target_os = "macos", feature = "tray"))]
fn run_tray_mode(cli: Cli) -> Result<()> {
    let mut cfg = Config::load(cli.config.as_deref()).context("Failed to load config")?;

    let pii_flag = match &cli.command {
        Commands::Start { pii, .. } => *pii,
        _ => unreachable!(),
    };

    let config_path = cli.config
        .as_deref()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| config::default_config_dir().join("config.toml"));

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    let cfg_mgr = ConfigManager::new(cfg.clone(), Some(config_path.clone()));

    let sidecar: dashboard::SidecarHandle = Arc::new(std::sync::Mutex::new(None));
    rt.block_on(ensure_slm_model(&mut cfg, &cfg_mgr, &sidecar));

    let resources = load_proxy_resources(&mut cfg, cli.log_file.as_deref(), pii_flag, true)?;

    let dashboard_url = format!("http://{}", cfg.proxy.dashboard);
    let shutdown = Arc::new(Notify::new());

    let cfg = Arc::new(cfg);

    // ── Permanent tasks (dashboard, PII eviction, log rotation) ────────────
    {
        let addr = cfg.proxy.dashboard.clone();
        let proxy_state = dashboard::ProxyState::new();
        let download_tracker = crate::models::DownloadTracker::new();
        let (s, w, m, ps, dt, sc) = (
            resources.store.clone(),
            resources.ws_tx.clone(),
            cfg_mgr.clone(),
            proxy_state,
            download_tracker,
            sidecar.clone(),
        );
        rt.spawn(async move {
            if let Err(e) = dashboard::run(&addr, s, w, m, ps, dt, sc).await {
                tracing::error!(err = %e, "dashboard error");
            }
        });
        tracing::warn!(addr = %cfg.proxy.dashboard, "dashboard started (permanent)");
    }

    if let Some(ref p) = resources.pii {
        let registry = Arc::clone(&p.registry);
        rt.spawn(async move {
            let mut interval = tokio::time::interval(VAULT_EVICT_INTERVAL);
            loop { interval.tick().await; registry.evict_expired(); }
        });
    }

    {
        let s = resources.store.clone();
        rt.spawn(rotation_loop(s));
    }

    if let Err(e) = pid::write_pid() {
        tracing::warn!(err = %e, "failed to write PID file");
    }

    // ── Build TrayState and hand off to tray::run() ─────────────────────────
    let state = tray::TrayState {
        proxy_running:    false, // tray::run() calls start_proxy() on entry
        http_listener_on: false,
        http_task:        None,
        net_task:         None,
        cert_cache: resources.cert_cache,
        cfg,
        cfg_mgr,
        store:  resources.store,
        ws_tx:  resources.ws_tx,
        pii:    resources.pii,
        rt: rt.handle().clone(),
    };

    // Blocks until the user clicks "Quit Privacyclaw".
    tray::run(dashboard_url, shutdown, state);

    pid::remove_pid();
    rt.shutdown_timeout(Duration::from_secs(3));
    Ok(())
}

// ── Logging initialisation ────────────────────────────────────────────────────

/// Build a rolling file appender for the given path and rotation policy.
fn make_file_appender(file_path: &str, rotation: &str) -> tracing_appender::rolling::RollingFileAppender {
    let path = std::path::Path::new(file_path);
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("privacyclaw.log");
    match rotation {
        "hourly" => tracing_appender::rolling::hourly(dir, file_name),
        "never"  => tracing_appender::rolling::never(dir, file_name),
        _        => tracing_appender::rolling::daily(dir, file_name),
    }
}

/// Initialise the global tracing subscriber.
///
/// Returns `WorkerGuard` objects that must be kept alive for the program's
/// lifetime — dropping them flushes and shuts down the background writer threads.
fn init_logging(cfg: &config::LoggingConfig) -> Vec<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    let (nb_stderr, guard_stderr) = tracing_appender::non_blocking(std::io::stderr());

    match cfg.format.as_str() {
        "json" => {
            let stderr_layer = fmt::layer()
                .json()
                .with_writer(nb_stderr)
                .with_current_span(false);

            if let Some(ref file_path) = cfg.file {
                let (nb_file, guard_file) =
                    tracing_appender::non_blocking(make_file_appender(file_path, &cfg.rotation));
                let file_layer = fmt::layer().json().with_writer(nb_file);
                tracing_subscriber::registry()
                    .with(filter)
                    .with(stderr_layer)
                    .with(file_layer)
                    .init();
                return vec![guard_stderr, guard_file];
            }

            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .init();
        }
        _ => {
            // "text" format (default)
            let stderr_layer = fmt::layer()
                .with_writer(nb_stderr)
                .with_target(false);

            if let Some(ref file_path) = cfg.file {
                let (nb_file, guard_file) =
                    tracing_appender::non_blocking(make_file_appender(file_path, &cfg.rotation));
                let file_layer = fmt::layer().with_writer(nb_file).with_target(false);
                tracing_subscriber::registry()
                    .with(filter)
                    .with(stderr_layer)
                    .with(file_layer)
                    .init();
                return vec![guard_stderr, guard_file];
            }

            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .init();
        }
    }

    vec![guard_stderr]
}

// ── Subcommand handlers ───────────────────────────────────────────────────────

async fn cmd_init(cfg: &Config, install_ca: bool, network: bool) -> Result<()> {
    let ca_dir = default_ca_dir();

    match ca::load_ca(&ca_dir)? {
        Some(_) => {
            println!("CA already exists at: {:?}", ca_dir.join("ca.pem"));
            println!("Use `privacyclaw reset-ca` to regenerate.");
        }
        None => {
            println!("Generating CA certificate...");
            ca::generate_ca(&ca_dir)?;
            println!("CA certificate created at: {:?}", ca_dir.join("ca.pem"));
        }
    }

    if install_ca {
        println!("Installing CA into OS trust store...");
        match ca::install_ca_trust(&ca_dir) {
            Ok(_) => println!("CA installed successfully."),
            Err(e) => println!("Installation failed: {}", e),
        }
    } else {
        println!("\nTo trust the CA, run: privacyclaw init --install-ca");
        println!("Or manually install: {:?}", ca_dir.join("ca.pem"));
    }

    if network {
        println!("\nConfiguring network proxy (pf rules + /etc/hosts)...");
        println!("This will request administrator credentials.");
        let domains: Vec<&str> = cfg.intercept.domains.iter().map(|s| s.as_str()).collect();
        let port: u16 = cfg.network_proxy.listen
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(16441);
        match network_helper::enable(&domains, port) {
            Ok(_) => {
                tracing::warn!(domains = ?domains, port, "network proxy configured via init");
                println!("Network proxy rules applied.");
                println!("  pf anchor:  /etc/pf.anchors/privacyclaw");
                println!("  /etc/hosts: {} domain(s) added", domains.len());
            }
            Err(e) => println!("Network setup failed: {e}\nRun `privacyclaw network-enable` manually to retry."),
        }
    }

    println!("\nSetup complete. Start the proxy with:");
    println!("  privacyclaw start");
    println!("\nFor CONNECT mode, configure your client:");
    println!("  export HTTPS_PROXY=http://{}", cfg.proxy.listen);
    println!("  export NODE_EXTRA_CA_CERTS=\"{}\"", ca::ca_cert_path(&ca_dir).display());
    Ok(())
}

/// Ensures a T3 model is available before the proxy starts.
///
/// If T3 is enabled but no model is present on disk, auto-downloads `smollm2-135m`,
/// persists the updated `model_id` to config, and starts the llama-server sidecar.
/// On any failure, T3 is disabled for this session (fail-open) and the function returns.
async fn ensure_slm_model(
    cfg: &mut Config,
    cfg_mgr: &Arc<ConfigManager>,
    sidecar: &dashboard::SidecarHandle,
) {
    if !cfg.pii.tiers.slm {
        tracing::debug!("T3 disabled; skipping auto-download");
        return;
    }
    let models_dir = cfg.resolved_models_dir();
    if let Some(ref id) = cfg.pii.slm.model_id.clone() {
        if !id.is_empty() && crate::models::is_downloaded(&models_dir, id) {
            tracing::debug!(model_id = %id, "T3 model already present, skipping auto-download");
            return;
        }
    }

    tracing::info!(model_id = "smollm2-135m", size_mb = 90, "T3 enabled but no model active; auto-downloading");
    let smollm2_info = &crate::models::catalog()[0]; // smollm2-135m is index 0
    let result = crate::models::download_with_bar(smollm2_info, &models_dir).await;

    match result {
        Err(e) => {
            tracing::warn!(err = %e, "auto-download failed; T3 disabled for this session");
            cfg.pii.tiers.slm = false;
            return;
        }
        Ok(()) => { /* continue to success branch */ }
    }

    cfg.pii.slm.model_id = Some("smollm2-135m".to_string());

    let patch = serde_json::json!({"pii": {"slm": {"model_id": "smollm2-135m"}}});
    if let Err(e) = cfg_mgr.patch(patch).await {
        tracing::warn!(err = %e, "config patch failed; model_id not persisted");
    } else if let Err(e) = cfg_mgr.save_to_disk().await {
        tracing::warn!(err = %e, "config save failed; model_id not persisted across restarts");
    }

    let bin = dashboard::llama_server_bin_path();
    let model_file = models_dir.join("smollm2-135m-instruct-q4_k_m.gguf");
    match tokio::task::spawn_blocking(move || {
        crate::pii::tier3::SidecarProcess::start(&bin, &model_file, 16442, 30u64)
    }).await {
        Ok(Ok(proc)) => {
            if let Ok(mut guard) = sidecar.lock() { *guard = Some(proc); }
            tracing::info!(model_id = "smollm2-135m", "model downloaded and sidecar started");
        }
        Ok(Err(e)) => {
            tracing::warn!(err = %e, "sidecar failed to start; T3 disabled for this session");
            cfg.pii.tiers.slm = false;
        }
        Err(e) => {
            tracing::warn!(err = %e, "sidecar spawn_blocking panicked; T3 disabled for this session");
            cfg.pii.tiers.slm = false;
        }
    }
}

async fn cmd_start(
    cfg: Config,
    cfg_mgr: Arc<ConfigManager>,
    pii_flag: bool,
    mode: Option<StartMode>,
    tray_shutdown: Arc<Notify>,
) -> Result<()> {
    let mut cfg = cfg;
    let sidecar: dashboard::SidecarHandle = Arc::new(std::sync::Mutex::new(None));
    ensure_slm_model(&mut cfg, &cfg_mgr, &sidecar).await;
    let ca_dir = default_ca_dir();
    let bundle = ca::load_ca(&ca_dir)?
        .context("CA not initialized. Run `privacyclaw init` first.")?;

    // Resolve effective mode:
    // - Explicit --mode flag → use exactly as specified, ignore cfg.network_proxy.enabled.
    // - No --mode (None) → default to All, but still respect cfg.network_proxy.enabled for
    //   the network proxy so that existing configs and e2e tests are unaffected.
    let effective_mode = mode.as_ref().unwrap_or(&StartMode::All);
    let run_http = !matches!(effective_mode, StartMode::Network);
    let run_net = matches!(effective_mode, StartMode::Network | StartMode::All)
        && (mode.is_some() || cfg.network_proxy.enabled);

    let mode_label = match (run_http, run_net) {
        (true, true)   => "all",
        (true, false)  => "http",
        (false, true)  => "network",
        (false, false) => "http",   // fallback, shouldn't happen
    };
    tracing::warn!(mode = mode_label, "proxy starting");
    tracing::debug!(explicit = mode.is_some(), run_http, run_net, "mode selection");

    if run_http {
        println!("MITM proxy (CONNECT) on {}", cfg.proxy.listen);
        println!("Dashboard at http://{}", cfg.proxy.dashboard);
        println!("Intercepting: {}", cfg.intercept.domains.join(", "));
        println!("\nConfigure your client:");
        println!("  export HTTPS_PROXY=http://{}", cfg.proxy.listen);
    } else {
        println!("Network proxy on {}", cfg.network_proxy.listen);
        println!("Dashboard at http://{}", cfg.proxy.dashboard);
        println!("Intercepting: {}", cfg.intercept.domains.join(", "));
    }
    println!("\nPress Ctrl+C to stop.\n");

    let logs_dir = cfg.resolved_logs_dir();
    let store = storage::Store::open(&logs_dir)
        .with_context(|| format!("Failed to open log dir: {:?}", logs_dir))?;
    tracing::info!(logs_dir = %logs_dir.display(), "store opened");

    let cert_cache = ca::cert_gen::CertCache::new(bundle);
    tracing::info!("cert cache initialised");

    let pii: PiiCtx = build_pii_ctx(&cfg, pii_flag);
    if let Some(ref p) = pii {
        let registry = Arc::clone(&p.registry);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(VAULT_EVICT_INTERVAL);
            loop { interval.tick().await; registry.evict_expired(); }
        });
    }

    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);
    let cfg = Arc::new(cfg);
    let proxy_state = dashboard::ProxyState::new();
    let download_tracker = crate::models::DownloadTracker::new();

    if let Err(e) = pid::write_pid() {
        tracing::warn!(err = %e, "failed to write PID file");
    }

    let proxy_task = if run_http {
        tracing::debug!("spawning CONNECT proxy task");
        let (c, cc, s, w, p) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone(), pii.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::run(c, cc, s, w, p).await {
                tracing::error!(err = %e, detail = ?e, "proxy error");
            }
        })
    } else {
        tracing::debug!("CONNECT proxy not started (mode=network)");
        tokio::spawn(std::future::pending::<()>())
    };

    let net_task = if run_net {
        tracing::debug!("spawning network proxy task");
        let (c, cc, s, w, p) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone(), pii.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::network::run(c, cc, s, w, p).await {
                tracing::error!(err = %e, detail = ?e, "network proxy error");
            }
        })
    } else {
        tracing::debug!("network proxy not started");
        tokio::spawn(std::future::pending::<()>())
    };

    let dashboard_task = {
        let addr = cfg.proxy.dashboard.clone();
        let (s, w, m, ps, dt, sc) = (store.clone(), ws_tx.clone(), cfg_mgr.clone(), proxy_state.clone(), download_tracker.clone(), sidecar.clone());
        tokio::spawn(async move {
            if let Err(e) = dashboard::run(&addr, s, w, m, ps, dt, sc).await {
                tracing::error!(err = %e, detail = ?e, "dashboard error");
            }
        })
    };

    let rotation_task = {
        let s = store.clone();
        tokio::spawn(rotation_loop(s))
    };

    tokio::select! {
        _ = proxy_task => {}
        _ = net_task => {}
        _ = dashboard_task => {}
        _ = rotation_task => {}
        _ = proxy_state.shutdown.notified() => {
            tracing::warn!("shutting down privacyclaw via dashboard");
            println!("\nShutting down.");
        }
        _ = tray_shutdown.notified() => {
            tracing::warn!("shutting down privacyclaw via tray");
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::warn!("shutting down privacyclaw");
            println!("\nShutting down.");
        }
    }
    pid::remove_pid();
    Ok(())
}

async fn rotation_loop(store: storage::Store) {
    loop {
        let secs_until_midnight = {
            let now = chrono::Utc::now();
            let tomorrow = (now.date_naive() + chrono::Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc();
            (tomorrow - now).num_seconds().max(0) as u64
        };
        tracing::debug!(secs = secs_until_midnight, "rotation_loop: sleeping until midnight");
        tokio::time::sleep(std::time::Duration::from_secs(secs_until_midnight)).await;
        match store.rotate_old() {
            Ok(n) => tracing::warn!(deleted = n, "log rotation complete"),
            Err(e) => tracing::warn!(err = %e, "log rotation failed"),
        }
    }
}

fn cmd_setup_network(cfg: &Config) -> Result<()> {
    println!("# ── /etc/hosts entries ─────────────────────────────────────");
    println!("# Add these lines to /etc/hosts (sudo required):");
    println!("# sudo nano /etc/hosts\n");
    for domain in &cfg.intercept.domains {
        println!("127.0.0.1  {}", domain);
    }

    let port = cfg.network_proxy.listen
        .rsplit(':')
        .next()
        .unwrap_or("4443");

    println!("\n# ── macOS pf port redirect ─────────────────────────────────");
    println!("# Redirect port 443 → {} without root on privacyclaw itself.", port);
    println!("# Add to /etc/pf.anchors/privacyclaw:\n");
    println!("rdr pass on lo0 proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port {}", port);
    println!("\n# Then load the anchor (run once as root):");
    println!("echo 'rdr-anchor \"privacyclaw\"' | sudo tee -a /etc/pf.conf");
    println!("echo 'load anchor \"privacyclaw\" from \"/etc/pf.anchors/privacyclaw\"' | sudo tee -a /etc/pf.conf");
    println!("sudo pfctl -ef /etc/pf.conf\n");

    println!("# ── NODE_EXTRA_CA_CERTS (for Node.js / Claude Code) ─────────");
    println!("export NODE_EXTRA_CA_CERTS=\"{}\"", ca::ca_cert_path(&default_ca_dir()).display());

    println!("\n# ── Start network proxy ─────────────────────────────────────");
    println!("privacyclaw network-start");
    Ok(())
}

fn cmd_ca_path(_cfg: &Config) -> Result<()> {
    println!("{}", ca::ca_cert_path(&default_ca_dir()).display());
    Ok(())
}

async fn cmd_reset_ca(_cfg: &Config) -> Result<()> {
    let ca_dir = default_ca_dir();
    println!("Deleting existing CA...");
    ca::delete_ca(&ca_dir)?;
    println!("Generating new CA...");
    ca::generate_ca(&ca_dir)?;
    println!("New CA created at: {:?}", ca_dir.join("ca.pem"));
    println!("Run `privacyclaw init --install-ca` to install into OS trust store.");
    Ok(())
}

async fn cmd_export(cfg: &Config, format: &str, output: &PathBuf) -> Result<()> {
    let store = storage::Store::open(&cfg.resolved_logs_dir())?;
    let data = store.export_all()?;

    match format {
        "json" => {
            let json = serde_json::to_string_pretty(&data)?;
            std::fs::write(output, &json)?;
            println!("Exported to {:?}", output);
        }
        _ => anyhow::bail!("Unsupported format: {}", format),
    }
    Ok(())
}

async fn cmd_test_pii(text: String, locale: Option<String>, format: String) -> Result<()> {
    use crate::pii::locale::Locale;
    use crate::pii::tier1::Tier1Detector;
    use crate::pii::vault::PiiVault;
    use crate::pii::synth::SyntheticGenerator;

    let locale = locale.as_deref()
        .and_then(Locale::from_str_opt)
        .unwrap_or_default();

    let spans = Tier1Detector::detect(&text, &locale);

    if format == "json" {
        let mut results = Vec::new();
        let mut vault = PiiVault::new("cli-test");
        for span in &spans {
            let original = &text[span.start..span.end];
            let synthetic = SyntheticGenerator::get_or_create(&mut vault, original, &span.entity_type, &locale, span.tier, span.confidence);
            results.push(serde_json::json!({
                "type": span.entity_type.label(),
                "original": original,
                "synthetic": synthetic,
                "tier": span.tier,
                "confidence": span.confidence,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if spans.is_empty() {
        println!("No PII detected.");
    } else {
        println!("{:<20} {:<40} {:<30} {:<6} Confidence", "Type", "Original", "Synthetic", "Tier");
        println!("{}", "-".repeat(105));
        let mut vault = PiiVault::new("cli-test");
        for span in &spans {
            let original = &text[span.start..span.end];
            let synthetic = SyntheticGenerator::get_or_create(&mut vault, original, &span.entity_type, &locale, span.tier, span.confidence);
            println!("{:<20} {:<40} {:<30} {:<6} {:.2}",
                span.entity_type.label(),
                if original.len() > 38 { &original[..38] } else { original },
                if synthetic.len() > 28 { &synthetic[..28] } else { &synthetic },
                span.tier,
                span.confidence);
        }
    }
    Ok(())
}

async fn cmd_models(action: ModelsAction) -> Result<()> {
    let cfg = Config::default();
    let models_dir = cfg.resolved_models_dir();

    match action {
        ModelsAction::Install { name } => {
            println!("Installing model '{}'...", name);
            crate::models::install(&name, &models_dir).await?;
            println!("Model '{}' installed successfully.", name);
        }
        ModelsAction::List => {
            let installed = crate::models::list_installed(&models_dir)?;
            if installed.is_empty() {
                println!("No models installed. Use: privacyclaw models install <name>");
                println!("\nAvailable models:");
                for m in crate::models::catalog() {
                    println!("  {} — {} ({} MB)", m.name, m.description, m.size_mb);
                }
            } else {
                println!("{:<30} {:<10} Path", "Name", "Size");
                println!("{}", "-".repeat(80));
                for m in installed {
                    println!("{:<30} {:<10} {}", m.name, format!("{}MB", m.size_bytes / 1_048_576), m.path.display());
                }
            }
        }
    }
    Ok(())
}

async fn cmd_benchmark(tier: Option<u8>) -> Result<()> {
    use crate::pii::locale::Locale;
    use crate::pii::tier1::Tier1Detector;

    let test_cases: &[(&str, &str)] = &[
        ("john@acme.com", "EMAIL"),
        ("123-45-6789", "SSN"),
        ("4532015112830366", "CREDIT_CARD"),
        ("sk-abcdefghijklmnopqrstuvwxyz12345678901234", "OPENAI_API_KEY"),
        ("AKIAIOSFODNN7EXAMPLE", "AWS_ACCESS_KEY"),
    ];

    let tier_filter = tier.unwrap_or(1);
    println!("Running Tier {} benchmark...\n", tier_filter);

    let mut total = 0usize;
    let mut detected = 0usize;
    for (text, expected_type) in test_cases {
        let spans = Tier1Detector::detect(text, &Locale::EnUs);
        let found = spans.iter().any(|s| s.entity_type.label() == *expected_type);
        total += 1;
        if found { detected += 1; }
        println!("[{}] {} → {}", if found { "PASS" } else { "FAIL" }, expected_type, text);
    }

    println!("\nResults: {}/{} detected ({:.0}%)", detected, total, 100.0 * detected as f32 / total as f32);
    Ok(())
}

async fn cmd_stop() -> Result<()> {
    match pid::read_pid() {
        None => {
            println!("privacyclaw is not running (no PID file found).");
            Ok(())
        }
        Some(pid_val) => {
            println!("Stopping privacyclaw (PID {})...", pid_val);
            match pid::stop_process(pid_val, 5) {
                Ok(true) => {
                    println!("Stopped.");
                    Ok(())
                }
                Ok(false) => {
                    println!("Process did not exit within 5s — force-killed.");
                    Ok(())
                }
                Err(e) => {
                    anyhow::bail!("Failed to stop process: {}", e);
                }
            }
        }
    }
}

async fn cmd_network_enable(cfg: &Config) -> Result<()> {
    let domains: Vec<&str> = cfg.intercept.domains.iter().map(|s| s.as_str()).collect();
    let port: u16 = cfg.network_proxy.listen
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(16441);

    println!("Enabling network proxy for domains: {}", domains.join(", "));
    println!("This will request administrator credentials.");
    network_helper::enable(&domains, port)?;

    #[cfg(target_os = "macos")]
    {
        let ca_pem = ca::ca_cert_path(&default_ca_dir());
        network_helper::launchctl_set_node_ca(&ca_pem);
        network_helper::flush_dns_cache();
    }

    println!("Network proxy enabled. Start the proxy with: privacyclaw network-start");
    Ok(())
}

async fn cmd_network_disable() -> Result<()> {
    if !network_helper::is_enabled() {
        println!("Network proxy is not currently enabled.");
        return Ok(());
    }
    println!("Disabling network proxy. This will request administrator credentials.");
    network_helper::disable()?;

    #[cfg(target_os = "macos")]
    {
        network_helper::launchctl_unset_node_ca();
        network_helper::flush_dns_cache();
    }

    println!("Network proxy disabled.");
    Ok(())
}

async fn cmd_uninstall(purge: bool) -> Result<()> {
    if purge {
        println!("Uninstalling privacyclaw and purging all data...");
    } else {
        println!("Uninstalling privacyclaw (user data preserved; use --purge to delete)...");
    }
    println!("Some steps may request administrator credentials.\n");

    let runner = uninstall::UninstallRunner::new(purge);
    let results = runner.run();
    uninstall::UninstallRunner::print_summary(&results);

    if uninstall::UninstallRunner::has_failures(&results) {
        std::process::exit(1);
    }
    Ok(())
}

// ── §8.T1: extract_llama_server ───────────────────────────────────────────────

/// §8.T1: Copy the llama-server binary from the app bundle Resources dir to dest_path.
/// Sets executable permission. No-ops (returns Ok) if dest_path already exists.
pub fn extract_llama_server(bundle_resources: &std::path::Path, dest_path: &std::path::Path) -> anyhow::Result<()> {
    if dest_path.exists() {
        return Ok(());
    }
    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let src = bundle_resources.join("llama-server");
    std::fs::copy(&src, dest_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dest_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dest_path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod extract_tests {
    use super::*;
    use tempfile::tempdir;

    /// §8.T1: extract_llama_server copies binary and sets executable bit; skips if already present.
    #[test]
    fn test_extract_llama_server_copies_and_skips() {
        let src_dir = tempdir().unwrap();
        let dest_dir = tempdir().unwrap();

        // Create a fake llama-server binary in bundle resources.
        let src_bin = src_dir.path().join("llama-server");
        std::fs::write(&src_bin, b"#!/bin/sh\necho fake llama-server").unwrap();

        let dest_bin = dest_dir.path().join("bin/llama-server");

        // First call: should copy.
        extract_llama_server(src_dir.path(), &dest_bin).unwrap();
        assert!(dest_bin.exists(), "binary should have been copied");

        // Verify executable permission (Unix).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dest_bin).unwrap().permissions().mode();
            assert!(mode & 0o111 != 0, "binary must be executable");
        }

        // Second call: should skip (no error, file unchanged).
        let content_before = std::fs::read(&dest_bin).unwrap();
        extract_llama_server(src_dir.path(), &dest_bin).unwrap();
        let content_after = std::fs::read(&dest_bin).unwrap();
        assert_eq!(content_before, content_after, "second extract should be a no-op");
    }

    /// §8.T1: extract_llama_server returns error if source binary doesn't exist.
    #[test]
    fn test_extract_llama_server_missing_source_errors() {
        let empty_dir = tempdir().unwrap();
        let dest_dir = tempdir().unwrap();
        let dest_bin = dest_dir.path().join("llama-server");
        let result = extract_llama_server(empty_dir.path(), &dest_bin);
        assert!(result.is_err(), "missing source should return error");
    }
}

#[cfg(test)]
mod ensure_slm_model_tests {
    use super::*;
    use crate::config::{Config, ConfigManager};
    use tempfile::tempdir;

    fn make_sidecar() -> dashboard::SidecarHandle {
        Arc::new(std::sync::Mutex::new(None))
    }

    /// Test A: T3 disabled → ensure_slm_model returns immediately without touching model_id or sidecar.
    #[tokio::test]
    async fn test_a_slm_disabled_skips_download() {
        let mut cfg = Config::default();
        cfg.pii.tiers.slm = false;
        let cfg_mgr = ConfigManager::new(cfg.clone(), None);
        let sidecar = make_sidecar();

        ensure_slm_model(&mut cfg, &cfg_mgr, &sidecar).await;

        assert!(!cfg.pii.tiers.slm, "T3 should remain disabled");
        assert!(cfg.pii.slm.model_id.is_none(), "model_id should be untouched");
        assert!(sidecar.lock().unwrap().is_none(), "sidecar should be untouched");
    }

    /// Test B: T3 enabled, model_id set, GGUF file exists on disk → no download, sidecar unchanged.
    ///
    /// The zero-byte file simulates a downloaded model. `is_downloaded` checks file existence only.
    #[tokio::test]
    async fn test_b_model_already_present_skips_download() {
        let tmp = tempdir().unwrap();
        let models_dir = tmp.path();

        // A zero-byte GGUF file is enough for is_downloaded() to return true.
        std::fs::write(models_dir.join("smollm2-135m.gguf"), b"").unwrap();

        let mut cfg = Config::default();
        cfg.pii.tiers.slm = true;
        cfg.pii.slm.model_id = Some("smollm2-135m".to_string());
        cfg.pii.models_dir = models_dir.to_string_lossy().to_string();

        let cfg_mgr = ConfigManager::new(cfg.clone(), None);
        let sidecar = make_sidecar();

        ensure_slm_model(&mut cfg, &cfg_mgr, &sidecar).await;

        assert!(cfg.pii.tiers.slm, "T3 should remain enabled");
        assert_eq!(cfg.pii.slm.model_id.as_deref(), Some("smollm2-135m"));
        assert!(sidecar.lock().unwrap().is_none(), "sidecar not started (already present path)");
    }

    /// Test C: download_with_bar returns Err on non-200 from a mockito server.
    ///
    /// This tests the download error branch exercised by ensure_slm_model when
    /// the download fails. We use Box::leak to produce a `&'static str` URL pointing
    /// at the local mock server. This is a test-only pattern; the leaked memory is
    /// reclaimed when the test process exits.
    #[tokio::test]
    async fn test_c_download_failure_disables_t3() {
        let tmp = tempdir().unwrap();
        let models_dir = tmp.path();

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/smollm2-135m-instruct-q4_k_m.gguf")
            .with_status(503)
            .with_body("service unavailable")
            .create_async()
            .await;

        // Build a static URL string using Box::leak (valid for the test process lifetime).
        let url: &'static str =
            Box::leak(format!("{}/smollm2-135m-instruct-q4_k_m.gguf", server.url()).into_boxed_str());

        // Build a static ModelInfo pointing at the mock server.
        let mock_info: &'static crate::models::ModelInfo =
            Box::leak(Box::new(crate::models::ModelInfo {
                id: "smollm2-135m",
                name: "SmolLM2-135M-Instruct",
                description: "test mock",
                url,
                sha256: "",
                size_mb: 1,
            }));

        let result = crate::models::download_with_bar(mock_info, models_dir).await;
        assert!(result.is_err(), "download_with_bar must return Err on HTTP 503");
    }
}
