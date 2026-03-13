use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

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
    #[serde(default)]
    pub pii: PiiConfig,
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
    #[serde(default = "default_log_format")]
    pub format: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default = "default_log_rotation")]
    pub rotation: String,
}

fn default_log_format() -> String {
    "text".to_string()
}

fn default_log_rotation() -> String {
    "daily".to_string()
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:16440".to_string(),
            dashboard: "127.0.0.1:16443".to_string(),
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
            format: default_log_format(),
            file: None,
            rotation: default_log_rotation(),
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
            listen: "127.0.0.1:16441".to_string(),
            enabled: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiConfig {
    /// "off" | "detect-only" | "replace"
    pub mode: String,
    pub tiers: PiiTiersConfig,
    pub ner: PiiNerConfig,
    pub slm: PiiSlmConfig,
    pub vault_ttl_hours: u64,
    pub locale: String,
    pub models_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiTiersConfig {
    pub regex: bool,
    pub ner: bool,
    pub slm: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiNerConfig {
    pub model_path: String,
    pub confidence_threshold: f32,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiSlmConfig {
    pub endpoint: String,
    pub timeout_ms: u64,
    /// Spans below this confidence are sent to the SLM for disambiguation.
    pub confidence_threshold: f32,
    /// ID of the active SLM model (corresponds to a catalog entry).
    #[serde(default)]
    pub model_id: Option<String>,
}

impl Default for PiiTiersConfig {
    fn default() -> Self {
        Self {
            regex: true,
            ner: false,
            slm: false,
        }
    }
}

impl Default for PiiNerConfig {
    fn default() -> Self {
        Self {
            model_path: default_models_dir().to_string_lossy().to_string(),
            confidence_threshold: 0.5,
            timeout_ms: 500,
        }
    }
}

impl Default for PiiSlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:16442".to_string(),
            timeout_ms: 5000,
            confidence_threshold: 0.7,
            model_id: None,
        }
    }
}

impl Default for PiiConfig {
    fn default() -> Self {
        Self {
            mode: "off".to_string(),
            tiers: PiiTiersConfig::default(),
            ner: PiiNerConfig::default(),
            slm: PiiSlmConfig::default(),
            vault_ttl_hours: 24,
            locale: "en-US".to_string(),
            models_dir: default_models_dir().to_string_lossy().to_string(),
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
            pii: PiiConfig::default(),
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
        let mut cfg: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {:?}", config_path))?;

        disable_ner_if_model_missing(&mut cfg);

        Ok(cfg)
    }

    pub fn is_intercepted(&self, domain: &str) -> bool {
        self.intercept.domains.iter().any(|d| d == domain)
    }

    pub fn resolved_logs_dir(&self) -> PathBuf {
        expand_tilde(&self.storage.logs_dir)
    }

    pub fn resolved_models_dir(&self) -> PathBuf {
        expand_tilde(&self.pii.models_dir)
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

pub fn default_models_dir() -> PathBuf {
    default_config_dir().join("models")
}

pub fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

// ── Config Hot-Reload ────────────────────────────────────────────────────────

/// Result returned by `ConfigManager::patch()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchResult {
    pub ok: bool,
    /// True when the change requires a process restart to take effect (e.g. port change).
    pub restart_required: bool,
    /// Dot-separated paths of config keys that changed (e.g. `"pii.mode"`, `"proxy.listen"`).
    pub changed_keys: Vec<String>,
}

/// Thread-safe config holder with hot-patch support.
pub struct ConfigManager {
    inner: RwLock<Config>,
    config_path: Option<PathBuf>,
}

impl ConfigManager {
    pub fn new(config: Config, config_path: Option<PathBuf>) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(config),
            config_path,
        })
    }

    /// Return a snapshot clone of the current config.
    pub async fn get(&self) -> Config {
        self.inner.read().await.clone()
    }

    /// Apply a partial JSON patch to the current config.
    ///
    /// Returns `Err` if the patch would violate PII tier dependency rules or if
    /// the resulting value cannot be deserialised back to `Config`.
    pub async fn patch(&self, patch: serde_json::Value) -> anyhow::Result<PatchResult> {
        let mut guard = self.inner.write().await;

        // Serialize current config → JSON Value for deep-merging.
        let mut current_json = serde_json::to_value(&*guard)
            .map_err(|e| anyhow::anyhow!("failed to serialise config: {e}"))?;

        // Deep-merge and collect changed key paths.
        let changed_keys = json_deep_merge(&mut current_json, &patch);

        // Deserialise back to Config.
        let new_config: Config = serde_json::from_value(current_json)
            .map_err(|e| anyhow::anyhow!("invalid config after patch: {e}"))?;

        // Validate PII tier dependency rules.
        validate_pii_tiers(&new_config.pii.tiers)?;

        // Detect port / address changes that require a restart.
        let restart_required = requires_restart(&guard, &new_config);

        *guard = new_config;

        Ok(PatchResult { ok: true, restart_required, changed_keys })
    }

    /// Persist the current config to disk (if a path is known).
    pub async fn save_to_disk(&self) -> anyhow::Result<()> {
        if let Some(ref path) = self.config_path {
            let guard = self.inner.read().await;
            let toml_str = toml::to_string_pretty(&*guard)
                .map_err(|e| anyhow::anyhow!("failed to serialise config to TOML: {e}"))?;
            std::fs::write(path, toml_str)
                .map_err(|e| anyhow::anyhow!("failed to write config file: {e}"))?;
        }
        Ok(())
    }
}

/// Recursively merge `patch` into `base`, returning dot-separated paths of keys that changed.
fn json_deep_merge(base: &mut serde_json::Value, patch: &serde_json::Value) -> Vec<String> {
    fn recurse(
        base: &mut serde_json::Value,
        patch: &serde_json::Value,
        path: &str,
        changed: &mut Vec<String>,
    ) {
        match (base, patch) {
            (serde_json::Value::Object(b), serde_json::Value::Object(p)) => {
                for (key, val) in p {
                    let child_path = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    let entry = b.entry(key.clone()).or_insert(serde_json::Value::Null);
                    recurse(entry, val, &child_path, changed);
                }
            }
            (base_val, patch_val) => {
                if base_val != patch_val {
                    changed.push(path.to_string());
                    *base_val = patch_val.clone();
                }
            }
        }
    }

    let mut changed = Vec::new();
    recurse(base, patch, "", &mut changed);
    changed
}

/// Disable Tier 2 (NER) at startup when the model file is absent.
///
/// This prevents a confusing runtime failure when the user sets `tiers.ner = true`
/// in replace mode but has not yet downloaded the model. The warning logged here
/// is the only indication that NER was silently disabled.
fn disable_ner_if_model_missing(cfg: &mut Config) {
    if cfg.pii.mode == "replace" && cfg.pii.tiers.ner {
        let model_path = std::path::Path::new(&cfg.pii.ner.model_path);
        if !model_path.exists() {
            tracing::warn!(
                model_path = %cfg.pii.ner.model_path,
                "pii.tiers.ner = true but model not found; disabling Tier 2 (NER)"
            );
            cfg.pii.tiers.ner = false;
        }
    }
}

/// Enforce PII tier dependency rules.
///
/// Allowed combinations:
///   - Any subset where each tier's dependencies are satisfied.
///   - T3 standalone: `{regex:false, ner:false, slm:true}` — SLM runs without T1/T2.
fn validate_pii_tiers(tiers: &PiiTiersConfig) -> anyhow::Result<()> {
    // Tier 2 (NER) always requires Tier 1 (regex), even in standalone paths.
    if tiers.ner && !tiers.regex {
        anyhow::bail!("pii.tiers.ner requires pii.tiers.regex = true (Tier 2 depends on Tier 1)");
    }
    // Tier 3 (SLM) requires both T1 and T2 when not in standalone mode.
    // Standalone ({regex:false, ner:false, slm:true}) is explicitly allowed.
    if tiers.slm && !is_t3_standalone(tiers) && (!tiers.regex || !tiers.ner) {
        anyhow::bail!(
            "pii.tiers.slm requires pii.tiers.regex = true and pii.tiers.ner = true (Tier 3 depends on Tier 1 + Tier 2)"
        );
    }
    Ok(())
}

/// Returns `true` when Tier 3 standalone mode is active: SLM enabled with
/// both Tier 1 (regex) and Tier 2 (NER) disabled.
pub fn is_t3_standalone(tiers: &PiiTiersConfig) -> bool {
    tiers.slm && !tiers.regex && !tiers.ner
}

/// Returns true if any field that requires a process restart has changed.
fn requires_restart(old: &Config, new: &Config) -> bool {
    old.proxy.listen != new.proxy.listen
        || old.proxy.dashboard != new.proxy.dashboard
        || old.network_proxy.listen != new.network_proxy.listen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_mgr() -> Arc<ConfigManager> {
        ConfigManager::new(Config::default(), None)
    }

    #[tokio::test]
    async fn patch_pii_mode_changes_key() {
        let mgr = default_mgr();
        let result = mgr
            .patch(serde_json::json!({ "pii": { "mode": "detect-only" } }))
            .await
            .unwrap();
        assert!(result.ok);
        assert!(!result.restart_required);
        assert!(result.changed_keys.contains(&"pii.mode".to_string()));
        assert_eq!(mgr.get().await.pii.mode, "detect-only");
    }

    #[tokio::test]
    async fn patch_port_sets_restart_required() {
        let mgr = default_mgr();
        let result = mgr
            .patch(serde_json::json!({ "proxy": { "listen": "127.0.0.1:19999" } }))
            .await
            .unwrap();
        assert!(result.restart_required);
        assert!(result.changed_keys.contains(&"proxy.listen".to_string()));
    }

    #[tokio::test]
    async fn patch_same_value_no_changed_keys() {
        let mgr = default_mgr();
        let current_mode = mgr.get().await.pii.mode.clone();
        let result = mgr
            .patch(serde_json::json!({ "pii": { "mode": current_mode } }))
            .await
            .unwrap();
        assert!(result.changed_keys.is_empty());
        assert!(!result.restart_required);
    }

    #[tokio::test]
    async fn patch_tier2_without_tier1_is_error() {
        let mgr = default_mgr();
        // Default: regex=true, so first disable regex then enable ner in one patch
        let err = mgr
            .patch(serde_json::json!({ "pii": { "tiers": { "regex": false, "ner": true } } }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Tier 2 depends on Tier 1"));
    }

    #[tokio::test]
    async fn patch_tier3_without_tier2_is_error() {
        // {regex:true, ner:false, slm:true} — T3 present but T2 absent, T1 present.
        // This must be rejected by the T3-depends-on-T1+T2 rule (validate_pii_tiers line ~403).
        let mgr = default_mgr();
        let err = mgr
            .patch(serde_json::json!({ "pii": { "tiers": { "regex": true, "ner": false, "slm": true } } }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Tier 3 depends on Tier 1 + Tier 2"));
    }

    #[tokio::test]
    async fn patch_tier3_with_all_tiers_ok() {
        let mgr = default_mgr();
        let result = mgr
            .patch(serde_json::json!({ "pii": { "tiers": { "regex": true, "ner": true, "slm": true } } }))
            .await
            .unwrap();
        assert!(result.ok);
        let cfg = mgr.get().await;
        assert!(cfg.pii.tiers.slm);
    }

    /// A.4: Hotreload — patch pii.mode from "off" to "replace" → config reflects change.
    #[tokio::test]
    async fn test_config_hotreload_off_to_replace() {
        let mut base = Config::default();
        base.pii.mode = "off".to_string();
        let mgr = ConfigManager::new(base, None);

        assert_eq!(mgr.get().await.pii.mode, "off");

        let result = mgr
            .patch(serde_json::json!({ "pii": { "mode": "replace" } }))
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.changed_keys.contains(&"pii.mode".to_string()),
            "pii.mode must appear in changed_keys, got {:?}", result.changed_keys);
        assert_eq!(mgr.get().await.pii.mode, "replace",
            "config must reflect new mode after patch");
    }

    #[tokio::test]
    async fn patch_tier3_standalone_is_allowed() {
        let mgr = default_mgr();
        let result = mgr
            .patch(serde_json::json!({ "pii": { "tiers": { "regex": false, "ner": false, "slm": true } } }))
            .await
            .unwrap();
        assert!(result.ok);
        let cfg = mgr.get().await;
        assert!(!cfg.pii.tiers.regex);
        assert!(!cfg.pii.tiers.ner);
        assert!(cfg.pii.tiers.slm);
    }

    #[test]
    fn is_t3_standalone_true_for_standalone_combo() {
        let tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
        assert!(is_t3_standalone(&tiers));
    }

    #[test]
    fn is_t3_standalone_false_for_full_stack() {
        let tiers = PiiTiersConfig { regex: true, ner: true, slm: true };
        assert!(!is_t3_standalone(&tiers));
    }

    /// is_t3_standalone returns false when slm=false (all off).
    #[test]
    fn is_t3_standalone_false_no_slm() {
        let tiers = PiiTiersConfig { regex: false, ner: false, slm: false };
        assert!(!is_t3_standalone(&tiers),
            "is_t3_standalone must be false when slm=false");
    }

    /// validate_pii_tiers accepts the T3 standalone combination without error.
    #[test]
    fn validate_pii_tiers_standalone_accepted() {
        let tiers = PiiTiersConfig { regex: false, ner: false, slm: true };
        assert!(validate_pii_tiers(&tiers).is_ok(),
            "T3 standalone {{regex:false, ner:false, slm:true}} must pass validation");
    }

    /// validate_pii_tiers still rejects T2 without T1 after the T3 standalone relaxation.
    #[test]
    fn validate_pii_tiers_t2_without_t1_still_rejected() {
        let tiers = PiiTiersConfig { regex: false, ner: true, slm: false };
        let err = validate_pii_tiers(&tiers).unwrap_err();
        assert!(err.to_string().contains("Tier 2 depends on Tier 1"),
            "error must mention 'Tier 2 depends on Tier 1', got: {err}");
    }

    /// E.3.4: patch pii.tiers.ner=true is accepted and reflected in config.
    #[tokio::test]
    async fn test_patch_config_ner_flag_persists() {
        let mgr = default_mgr();
        assert!(!mgr.get().await.pii.tiers.ner);

        let result = mgr
            .patch(serde_json::json!({ "pii": { "tiers": { "ner": true } } }))
            .await
            .unwrap();

        assert!(result.ok);
        assert!(result.changed_keys.contains(&"pii.tiers.ner".to_string()),
            "pii.tiers.ner must appear in changed_keys, got {:?}", result.changed_keys);
        assert!(mgr.get().await.pii.tiers.ner, "ner flag must be true after patch");
    }
}
