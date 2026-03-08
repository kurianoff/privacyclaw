mod ca;
mod config;
mod dashboard;
mod parser;
mod proxy;
mod storage;
mod util;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::{default_ca_dir, Config};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

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
    Start,
    /// Start the network-level transparent proxy (requires /etc/hosts + pf redirect)
    NetworkStart,
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
        Commands::Start => cmd_start(cfg).await,
        Commands::NetworkStart => cmd_network_start(cfg).await,
        Commands::SetupNetwork => cmd_setup_network(&cfg),
        Commands::CaPath => cmd_ca_path(&cfg),
        Commands::ResetCa => cmd_reset_ca(&cfg).await,
        Commands::Export { format, output } => cmd_export(&cfg, &format, &output).await,
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

async fn cmd_start(cfg: Config) -> Result<()> {
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

    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);
    let cfg = Arc::new(cfg);

    tracing::warn!("starting claudovka in CONNECT mode");

    let proxy_task = {
        let (c, cc, s, w) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::run(c, cc, s, w).await {
                tracing::error!("Proxy error: {}", e);
            }
        })
    };

    let net_task = if cfg.network_proxy.enabled {
        tracing::warn!("starting network proxy alongside CONNECT proxy");
        let (c, cc, s, w) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::network::run(c, cc, s, w).await {
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

async fn cmd_network_start(cfg: Config) -> Result<()> {
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

    let (ws_tx, _) = broadcast::channel::<dashboard::WsEvent>(1024);
    let cfg = Arc::new(cfg);

    tracing::warn!("starting claudovka in network mode");

    let net_task = {
        let (c, cc, s, w) = (cfg.clone(), cert_cache.clone(), store.clone(), ws_tx.clone());
        tokio::spawn(async move {
            if let Err(e) = proxy::network::run(c, cc, s, w).await {
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
