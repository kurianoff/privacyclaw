use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub intercept: InterceptConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub network_proxy: NetworkProxyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    pub listen: String,
    pub dashboard: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptConfig {
    pub domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub logs_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".to_string(),
            dashboard: "127.0.0.1:8443".to_string(),
        }
    }
}

impl Default for InterceptConfig {
    fn default() -> Self {
        Self {
            domains: vec![
                "api.anthropic.com".to_string(),
                "api.openai.com".to_string(),
                "generativelanguage.googleapis.com".to_string(),
                "api.mistral.ai".to_string(),
                "api.groq.com".to_string(),
            ],
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            logs_dir: default_logs_dir().to_string_lossy().to_string(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkProxyConfig {
    /// Address to listen on for direct TLS connections (network/transparent mode).
    pub listen: String,
    /// If true, `claudovka start` also starts the network proxy listener.
    pub enabled: bool,
}

impl Default for NetworkProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:4443".to_string(),
            enabled: false,
        }
    }
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            proxy: ProxyConfig::default(),
            intercept: InterceptConfig::default(),
            storage: StorageConfig::default(),
            logging: LoggingConfig::default(),
            network_proxy: NetworkProxyConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let config_path = path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(default_config_path);

        if !config_path.exists() {
            tracing::debug!("No config file at {:?}, using defaults", config_path);
            return Ok(Config::default());
        }

        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config file: {:?}", config_path))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", config_path))?;
        Ok(config)
    }

    pub fn is_intercepted(&self, domain: &str) -> bool {
        self.intercept.domains.iter().any(|d| d == domain)
    }

    pub fn resolved_logs_dir(&self) -> PathBuf {
        expand_tilde(&self.storage.logs_dir)
    }
}

pub fn default_config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claudovka")
}

pub fn default_ca_dir() -> PathBuf {
    default_config_dir().join("ca")
}

fn default_config_path() -> PathBuf {
    default_config_dir().join("config.toml")
}

fn default_logs_dir() -> PathBuf {
    default_config_dir().join("logs")
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}
