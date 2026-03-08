mod ca;
mod config;
mod dashboard;
mod models;
mod parser;
mod pii;
mod proxy;
mod storage;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{default_ca_dir, Config};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use crate::pii::{PiiCtx, PiiContext, PiiMode};
use crate::pii::vault::VaultRegistry;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "claudovka", about = "Local MITM privacy proxy for LLM API traffic", version)]
struct Cli {
    /// Path to config file
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate CA certificate and print setup instructions
    Init {
        /// Attempt to install CA into OS trust store
        #[arg(long)]
        install_ca: bool,
    },
    /// Start the CONNECT proxy and dashboard (configure apps with HTTPS_PROXY)
    Start {
        /// Enable PII replace mode (Tier 1+2)
        #[arg(long)]
        pii: bool,
        /// Also enable Tier 3 SLM sidecar
        #[arg(long)]
        pii_llm: bool,
    },
    /// Start the network-level transparent proxy (requires /etc/hosts + pf redirect)
    NetworkStart {
        #[arg(long)]
        pii: bool,
        #[arg(long)]
        pii_llm: bool,
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
}

#[derive(Subcommand)]
enum ModelsAction {
    /// Install a model by name
    Install { name: String },
    /// List installed models
    List,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install ring as the default rustls CryptoProvider before any TLS is used.
    // Required when multiple providers (ring + aws-lc-rs) are present as transitive deps.
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok(); // ok() — harmless if already installed

    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref()).context("Failed to load config")?;

    // Init tracing
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.logging.level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    match cli.command {
        Commands::Init { install_ca } => cmd_init(&cfg, install_ca).await,
        Commands::Start { pii, pii_llm } => cmd_start(cfg, pii, pii_llm).await,
        Commands::NetworkStart { pii, pii_llm } => cmd_network_start(cfg, pii, pii_llm).await,
        Commands::SetupNetwork => cmd_setup_network(&cfg),
        Commands::CaPath => cmd_ca_path(&cfg),
        Commands::ResetCa => cmd_reset_ca(&cfg).await,
        Commands::Export { format, output } => cmd_export(&cfg, &format, &output).await,
        Commands::TestPii { text, locale, format } => cmd_test_pii(text, locale, format).await,
        Commands::Models { action } => cmd_models(action).await,
        Commands::Benchmark { tier } => cmd_benchmark(tier).await,
    }
}

async fn cmd_init(cfg: &Config, install_ca: bool) -> Result<()> {
    let ca_dir = default_ca_dir();

    match ca::load_ca(&ca_dir)? {
        Some(_) => {
            println!("CA already exists at: {:?}", ca_dir.join("ca.pem"));
            println!("Use `claudovka reset-ca` to regenerate.");
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
        println!("\nTo trust the CA, run: claudovka init --install-ca");
        println!("Or manually install: {:?}", ca_dir.join("ca.pem"));
    }

    println!("\nSetup complete. Start the CONNECT proxy with:");
    println!("  claudovka start");
    println!("\nOr start the network-level proxy with:");
    println!("  claudovka setup-network   # print /etc/hosts + pf instructions");
    println!("  claudovka network-start");
    println!("\nFor CONNECT mode, configure your client:");
    println!("  export HTTPS_PROXY=http://{}", cfg.proxy.listen);
    Ok(())
}

async fn cmd_start(cfg: Config, pii_flag: bool, _pii_llm: bool) -> Result<()> {
    let ca_dir = default_ca_dir();
    let bundle = ca::load_ca(&ca_dir)?
        .context("CA not initialized. Run `claudovka init` first.")?;

    println!("MITM proxy (CONNECT) on {}", cfg.proxy.listen);
    println!("Dashboard at http://{}", cfg.proxy.dashboard);
    println!("Intercepting: {}", cfg.intercept.domains.join(", "));
    println!("\nConfigure your client:");
    println!("  export HTTPS_PROXY=http://{}", cfg.proxy.listen);
    println!("\nPress Ctrl+C to stop.\n");

    let logs_dir = cfg.resolved_logs_dir();
    let store = storage::Store::open(&logs_dir)
        .with_context(|| format!("Failed to open log dir: {:?}", logs_dir))?;
    tracing::info!(logs_dir = %logs_dir.display(), "store opened");

    let cert_cache = ca::cert_gen::CertCache::new(bundle);
    tracing::info!("cert cache initialised");

    let pii_mode = if pii_flag || cfg.pii.mode == "replace" {
        PiiMode::Replace
    } else if cfg.pii.mode == "detect-only" {
        PiiMode::DetectOnly
    } else {
        PiiMode::Off
    };

    let pii: PiiCtx = if pii_mode != PiiMode::Off {
        let ttl = Duration::from_secs(cfg.pii.vault_ttl_hours * 3600);
        let locale = crate::pii::locale::Locale::from_str_opt(&cfg.pii.locale)
            .unwrap_or_default();
        Some(Arc::new(PiiContext {
            registry: Arc::new(VaultRegistry::new(ttl)),
            locale,
            mode: pii_mode,
        }))
    } else {
        None
    };

    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);
    let cfg = Arc::new(cfg);

    tracing::warn!("starting claudovka in CONNECT mode");

    let proxy_task = {
        let (c, cc, s, w, p) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone(), pii.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::run(c, cc, s, w, p).await {
                tracing::error!("Proxy error: {}", e);
            }
        })
    };

    let net_task = if cfg.network_proxy.enabled {
        tracing::warn!("starting network proxy alongside CONNECT proxy");
        let (c, cc, s, w, p) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone(), pii.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::network::run(c, cc, s, w, p).await {
                tracing::error!("Network proxy error: {}", e);
            }
        })
    } else {
        tokio::spawn(async { })
    };

    let dashboard_task = {
        let addr = cfg.proxy.dashboard.clone();
        let (s, w) = (store.clone(), ws_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = dashboard::run(&addr, s, w).await {
                tracing::error!("Dashboard error: {}", e);
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
        _ = tokio::signal::ctrl_c() => {
            tracing::warn!("shutting down claudovka");
            println!("\nShutting down.");
        }
    }
    Ok(())
}

async fn cmd_network_start(cfg: Config, pii_flag: bool, _pii_llm: bool) -> Result<()> {
    let ca_dir = default_ca_dir();
    let bundle = ca::load_ca(&ca_dir)?
        .context("CA not initialized. Run `claudovka init` first.")?;

    println!("Network proxy on {}", cfg.network_proxy.listen);
    println!("Dashboard at http://{}", cfg.proxy.dashboard);
    println!("Intercepting: {}", cfg.intercept.domains.join(", "));
    println!("\nPress Ctrl+C to stop.\n");

    let logs_dir = cfg.resolved_logs_dir();
    let store = storage::Store::open(&logs_dir)
        .with_context(|| format!("Failed to open log dir: {:?}", logs_dir))?;
    tracing::info!(logs_dir = %logs_dir.display(), "store opened");

    let cert_cache = ca::cert_gen::CertCache::new(bundle);
    tracing::info!("cert cache initialised");

    let pii_mode = if pii_flag || cfg.pii.mode == "replace" {
        PiiMode::Replace
    } else if cfg.pii.mode == "detect-only" {
        PiiMode::DetectOnly
    } else {
        PiiMode::Off
    };

    let pii: PiiCtx = if pii_mode != PiiMode::Off {
        let ttl = Duration::from_secs(cfg.pii.vault_ttl_hours * 3600);
        let locale = crate::pii::locale::Locale::from_str_opt(&cfg.pii.locale)
            .unwrap_or_default();
        Some(Arc::new(PiiContext {
            registry: Arc::new(VaultRegistry::new(ttl)),
            locale,
            mode: pii_mode,
        }))
    } else {
        None
    };

    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);
    let cfg = Arc::new(cfg);

    tracing::warn!("starting claudovka in network mode");

    let net_task = {
        let (c, cc, s, w, p) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone(), pii.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::network::run(c, cc, s, w, p).await {
                tracing::error!("Network proxy error: {}", e);
            }
        })
    };

    let dashboard_task = {
        let addr = cfg.proxy.dashboard.clone();
        let (s, w) = (store.clone(), ws_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = dashboard::run(&addr, s, w).await {
                tracing::error!("Dashboard error: {}", e);
            }
        })
    };

    let rotation_task = {
        let s = store.clone();
        tokio::spawn(rotation_loop(s))
    };

    tokio::select! {
        _ = net_task => {}
        _ = dashboard_task => {}
        _ = rotation_task => {}
        _ = tokio::signal::ctrl_c() => {
            tracing::warn!("shutting down claudovka");
            println!("\nShutting down.");
        }
    }
    Ok(())
}

/// Sleeps until UTC midnight, rotates old log files, then repeats.
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
            Err(e) => tracing::warn!("Log rotation failed: {}", e),
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
    println!("# Redirect port 443 → {} without root on claudovka itself.", port);
    println!("# Add to /etc/pf.anchors/claudovka:\n");
    println!("rdr pass on lo0 proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port {}", port);
    println!("\n# Then load the anchor (run once as root):");
    println!("echo 'rdr-anchor \"claudovka\"' | sudo tee -a /etc/pf.conf");
    println!("echo 'load anchor \"claudovka\" from \"/etc/pf.anchors/claudovka\"' | sudo tee -a /etc/pf.conf");
    println!("sudo pfctl -ef /etc/pf.conf\n");

    println!("# ── NODE_EXTRA_CA_CERTS (for Node.js / Claude Code) ─────────");
    println!("export NODE_EXTRA_CA_CERTS=\"{}\"", ca::ca_cert_path(&default_ca_dir()).display());

    println!("\n# ── Start network proxy ─────────────────────────────────────");
    println!("claudovka network-start");
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
    println!("Run `claudovka init --install-ca` to install into OS trust store.");
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
            let synthetic = SyntheticGenerator::get_or_create(&mut vault, original, &span.entity_type, &locale);
            results.push(serde_json::json!({
                "type": span.entity_type.label(),
                "original": original,
                "synthetic": synthetic,
                "tier": span.tier,
                "confidence": span.confidence,
            }));
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        if spans.is_empty() {
            println!("No PII detected.");
        } else {
            println!("{:<20} {:<40} {:<30} {:<6} {}", "Type", "Original", "Synthetic", "Tier", "Confidence");
            println!("{}", "-".repeat(105));
            let mut vault = PiiVault::new("cli-test");
            for span in &spans {
                let original = &text[span.start..span.end];
                let synthetic = SyntheticGenerator::get_or_create(&mut vault, original, &span.entity_type, &locale);
                println!("{:<20} {:<40} {:<30} {:<6} {:.2}",
                    span.entity_type.label(),
                    if original.len() > 38 { &original[..38] } else { original },
                    if synthetic.len() > 28 { &synthetic[..28] } else { &synthetic },
                    span.tier,
                    span.confidence);
            }
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
                println!("No models installed. Use: claudovka models install <name>");
                println!("\nAvailable models:");
                for m in crate::models::catalog() {
                    println!("  {} — {} ({} MB)", m.name, m.description, m.size_mb);
                }
            } else {
                println!("{:<30} {:<10} {}", "Name", "Size", "Path");
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
