use anyhow::Result;
use clap::Subcommand;
use std::path::Path;
use std::sync::Arc;

use crate::config::{Config, ConfigManager};
use crate::models::{catalog, is_downloaded, model_path};

// ── Public CLI type ───────────────────────────────────────────────────────────

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Print current config as TOML
    Show,
    /// Set a single dotted config key (KEY=VALUE)
    Set {
        /// Assignment in the form KEY=VALUE (e.g. pii.mode=replace)
        assignment: String,
    },
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

pub async fn cmd_config(
    cfg: Config,
    cfg_mgr: Arc<ConfigManager>,
    action: Option<ConfigAction>,
    protection_level: Option<String>,
    model: Option<String>,
) -> Result<()> {
    if let Some(ref level) = protection_level {
        return apply_protection_level(level, model.as_deref(), &cfg, &cfg_mgr).await;
    }
    match action {
        None => run_wizard(cfg, cfg_mgr).await,
        Some(ConfigAction::Show) => {
            show_config(&cfg);
            Ok(())
        }
        Some(ConfigAction::Set { assignment }) => set_config(cfg_mgr, &assignment).await,
    }
}

// ── protection_level_to_patch ─────────────────────────────────────────────────

/// Pure function: map a protection level string to a JSON patch for pii.mode + pii.tiers.
///
/// Does not handle model selection, download, or llama-server — those are
/// handled by `apply_protection_level`.
pub fn protection_level_to_patch(level: &str) -> anyhow::Result<serde_json::Value> {
    let patch = match level {
        "off" => serde_json::json!({
            "pii": {
                "mode": "off",
                "tiers": { "regex": false, "ner": false, "slm": false }
            }
        }),
        "detect" => serde_json::json!({
            "pii": {
                "mode": "detect-only",
                "tiers": { "regex": true, "ner": false, "slm": false }
            }
        }),
        "1" => serde_json::json!({
            "pii": {
                "mode": "replace",
                "tiers": { "regex": true, "ner": false, "slm": false }
            }
        }),
        "2" => serde_json::json!({
            "pii": {
                "mode": "replace",
                "tiers": { "regex": true, "ner": true, "slm": false }
            }
        }),
        "3" => serde_json::json!({
            "pii": {
                "mode": "replace",
                "tiers": { "regex": true, "ner": true, "slm": true }
            }
        }),
        "intelligent" => serde_json::json!({
            "pii": {
                "mode": "replace",
                "tiers": { "regex": false, "ner": false, "slm": true }
            }
        }),
        other => anyhow::bail!(
            "unknown protection level: {}; expected off, detect, 1, 2, 3, or intelligent",
            other
        ),
    };
    Ok(patch)
}

// ── apply_protection_level ────────────────────────────────────────────────────

/// Apply `--protection-level` (and optional `--model`) non-interactively.
async fn apply_protection_level(
    level: &str,
    model_arg: Option<&str>,
    cfg: &Config,
    cfg_mgr: &Arc<ConfigManager>,
) -> Result<()> {
    tracing::debug!(level, "apply_protection_level: level parsed");

    let needs_model = matches!(level, "3" | "intelligent");
    let mut patch = protection_level_to_patch(level)?;

    let models_dir = cfg.resolved_models_dir();

    if needs_model {
        // Resolve which model to use.
        let default_id = catalog()
            .iter()
            .find(|m| m.id.contains("phi"))
            .map(|m| m.id)
            .unwrap_or_else(|| catalog()[0].id);

        let model_id_str = model_arg.unwrap_or(default_id);
        tracing::debug!(model_id = model_id_str, "apply_protection_level: model resolved");

        // Determine info and path: file path takes priority over catalog id.
        let (info, model_path_buf) = if Path::new(model_id_str).is_file() {
            tracing::debug!(path = model_id_str, "apply_protection_level: model is a file path");
            (None, std::path::PathBuf::from(model_id_str))
        } else {
            let found = catalog()
                .iter()
                .find(|m| m.id == model_id_str)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "model '{}' not found in catalog; use a catalog id or a file path",
                        model_id_str
                    )
                })?;
            let mpath = model_path(&models_dir, found.id);
            (Some(found), mpath)
        };

        // Download if needed.
        if let Some(info_ref) = info {
            if !is_downloaded(&models_dir, info_ref.id) {
                println!(
                    "Downloading {} ({} MB)...",
                    info_ref.name, info_ref.size_mb
                );
                tracing::info!(model_id = info_ref.id, "model download started");
                crate::models::download_with_bar(info_ref, &models_dir).await?;
                tracing::info!(model_id = info_ref.id, "model downloaded");
            }
        }

        // Record model id in patch.
        let catalog_id = info.map(|i| i.id).unwrap_or(model_id_str);
        patch["pii"]["slm"]["model_id"] =
            serde_json::Value::String(catalog_id.to_string());
        tracing::debug!(model_id = catalog_id, "apply_protection_level: model_id added to patch");

        // Print the manual llama-server command; claudovka start manages the
        // sidecar lifecycle automatically once the config is saved.
        let port: u16 = 16442;
        println!(
            "\nTo start the SLM sidecar (required for T3):\n  llama-server --model {} --port {} --ctx-size 2048",
            model_path_buf.display(),
            port
        );
        println!("Or simply run `claudovka start` — it will launch llama-server automatically.");
        tracing::info!(
            model = %model_path_buf.display(),
            port,
            "apply_protection_level: llama-server command printed"
        );
    }

    cfg_mgr.patch(patch).await?;
    cfg_mgr.save_to_disk().await?;
    tracing::warn!(level, "protection level applied");
    tracing::info!("config saved to disk");
    println!("Protection level '{}' applied.", level);
    Ok(())
}

// ── show ──────────────────────────────────────────────────────────────────────

pub fn show_config(cfg: &Config) {
    let toml_str = toml::to_string_pretty(cfg).unwrap_or_else(|e| format!("# error: {e}"));
    print!("{}", toml_str);
}

// ── set ───────────────────────────────────────────────────────────────────────

async fn set_config(cfg_mgr: Arc<ConfigManager>, assignment: &str) -> Result<()> {
    let eq = assignment
        .find('=')
        .ok_or_else(|| anyhow::anyhow!("expected KEY=VALUE, got: {}", assignment))?;
    let key = &assignment[..eq];
    let raw_val = &assignment[eq + 1..];

    tracing::debug!(key, raw_val, "set_config: parsing assignment");

    let patch = dot_path_to_json(key, raw_val)?;
    let result = cfg_mgr.patch(patch).await?;
    cfg_mgr.save_to_disk().await?;

    tracing::info!(
        changed = ?result.changed_keys,
        restart_required = result.restart_required,
        "config updated"
    );

    println!("Set {} = {}", key, raw_val);
    if result.restart_required {
        println!("Note: restart claudovka for this change to take effect.");
    }
    Ok(())
}

// ── dot_path_to_json ─────────────────────────────────────────────────────────

/// Convert a dotted key path and a raw string value into a nested JSON object.
///
/// Type inference: `"true"`/`"false"` → bool, integer strings → i64, else string.
pub fn dot_path_to_json(key: &str, raw_val: &str) -> Result<serde_json::Value> {
    let leaf_val: serde_json::Value = if let Ok(b) = raw_val.parse::<bool>() {
        b.into()
    } else if let Ok(n) = raw_val.parse::<i64>() {
        n.into()
    } else {
        raw_val.into()
    };
    tracing::debug!(key, leaf = ?leaf_val, "dot_path_to_json: leaf inferred");
    let result = key
        .split('.')
        .rev()
        .fold(leaf_val, |acc, seg| serde_json::json!({ seg: acc }));
    Ok(result)
}

// ── wizard ───────────────────────────────────────────────────────────────────

async fn run_wizard(cfg: Config, cfg_mgr: Arc<ConfigManager>) -> Result<()> {
    use dialoguer::{theme::ColorfulTheme, Confirm};

    let theme = ColorfulTheme::default();
    let models_dir = cfg.resolved_models_dir();

    print_current_config(&cfg);

    let (chosen_mode, tier1_enabled, tier2_enabled, tier3_enabled) =
        collect_pii_settings(&cfg, &theme)?;

    let chosen_model_id = if tier3_enabled {
        collect_model_selection(&cfg, &theme, &models_dir).await?
    } else {
        cfg.pii.slm.model_id.clone()
    };

    println!("\nChanges to write:");
    println!("  pii.mode         = {}", chosen_mode);
    println!("  pii.tiers.regex  = {}", tier1_enabled);
    println!("  pii.tiers.ner    = {}", tier2_enabled);
    println!("  pii.tiers.slm    = {}", tier3_enabled);
    if let Some(ref mid) = chosen_model_id {
        println!("  pii.slm.model_id = {}", mid);
    }

    let confirmed = Confirm::with_theme(&theme)
        .with_prompt("Save?")
        .default(true)
        .interact()?;

    if !confirmed {
        println!("Aborted — no changes written.");
        return Ok(());
    }

    apply_wizard_patch(
        &cfg_mgr,
        chosen_mode,
        tier1_enabled,
        tier2_enabled,
        tier3_enabled,
        chosen_model_id.as_deref(),
    )
    .await?;

    if tier3_enabled {
        maybe_launch_llama_server(
            &theme,
            chosen_model_id.as_deref().unwrap_or(""),
            &models_dir,
        )
        .await?;
    }

    Ok(())
}

// ── wizard helpers ────────────────────────────────────────────────────────────

/// Print the config file path and current PII settings to stdout.
fn print_current_config(cfg: &Config) {
    let config_dir = crate::config::default_config_dir();
    println!(
        "\nCurrent config ({})",
        config_dir.join("config.toml").display()
    );
    println!("  pii.mode         = {}", cfg.pii.mode);
    println!("  pii.tiers.regex  = {}", cfg.pii.tiers.regex);
    println!("  pii.tiers.ner    = {}", cfg.pii.tiers.ner);
    println!("  pii.tiers.slm    = {}", cfg.pii.tiers.slm);
    println!();
}

/// Prompt the user to choose PII mode and which tiers to enable.
///
/// Returns `(mode, tier1_enabled, tier2_enabled, tier3_enabled)`.
fn collect_pii_settings(
    cfg: &Config,
    theme: &dialoguer::theme::ColorfulTheme,
) -> Result<(&'static str, bool, bool, bool)> {
    use dialoguer::{Confirm, Select};

    let mode_choices = &["off", "detect-only", "replace"];
    let current_mode_idx = mode_choices
        .iter()
        .position(|&m| m == cfg.pii.mode)
        .unwrap_or(0);

    let mode_idx = Select::with_theme(theme)
        .with_prompt("PII mode")
        .items(mode_choices)
        .default(current_mode_idx)
        .interact()?;
    let chosen_mode = mode_choices[mode_idx];
    tracing::debug!(mode = chosen_mode, "wizard: PII mode selected");

    let tier1_enabled = Confirm::with_theme(theme)
        .with_prompt("Enable Tier 1: regex detection?")
        .default(cfg.pii.tiers.regex)
        .interact()?;
    tracing::debug!(tier1 = tier1_enabled, "wizard: Tier 1 selected");

    let tier2_enabled = if tier1_enabled {
        Confirm::with_theme(theme)
            .with_prompt("Enable Tier 2: NER detection?")
            .default(cfg.pii.tiers.ner)
            .interact()?
    } else {
        tracing::debug!("wizard: Tier 2 skipped (Tier 1 disabled)");
        false
    };

    let tier3_prompt = if tier1_enabled && tier2_enabled {
        "Enable Tier 3: SLM disambiguation (full pipeline)?"
    } else {
        "Enable Tier 3: SLM standalone mode?"
    };
    let tier3_enabled = Confirm::with_theme(theme)
        .with_prompt(tier3_prompt)
        .default(cfg.pii.tiers.slm)
        .interact()?;
    tracing::debug!(tier3 = tier3_enabled, "wizard: Tier 3 selected");

    Ok((chosen_mode, tier1_enabled, tier2_enabled, tier3_enabled))
}

/// Show a model picker and, when the chosen model is absent, offer to download it.
///
/// Returns the selected model id.
async fn collect_model_selection(
    cfg: &Config,
    theme: &dialoguer::theme::ColorfulTheme,
    models_dir: &Path,
) -> Result<Option<String>> {
    use dialoguer::{Confirm, Select};

    let cat = catalog();
    let labels: Vec<String> = cat
        .iter()
        .map(|m| {
            let tag = if is_downloaded(models_dir, m.id) { " [downloaded]" } else { "" };
            format!("{} — {} ({} MB){}", m.name, m.description, m.size_mb, tag)
        })
        .collect();

    let current_id = cfg.pii.slm.model_id.as_deref();
    let default_idx = cat
        .iter()
        .position(|m| current_id == Some(m.id))
        .unwrap_or(0);

    let model_idx = Select::with_theme(theme)
        .with_prompt("Select SLM model")
        .items(&labels)
        .default(default_idx)
        .interact()?;

    let selected = &cat[model_idx];
    tracing::debug!(model_id = selected.id, "wizard: model selected");

    if !is_downloaded(models_dir, selected.id) {
        let do_download = Confirm::with_theme(theme)
            .with_prompt(format!("Download {} now?", selected.name))
            .default(false)
            .interact()?;
        if do_download {
            crate::models::download_with_bar(selected, models_dir).await?;
        }
    }

    Ok(Some(selected.id.to_string()))
}

/// Build and apply the wizard's patch, then persist to disk.
async fn apply_wizard_patch(
    cfg_mgr: &Arc<ConfigManager>,
    mode: &str,
    tier1: bool,
    tier2: bool,
    tier3: bool,
    model_id: Option<&str>,
) -> Result<()> {
    let mut patch = serde_json::json!({
        "pii": {
            "mode": mode,
            "tiers": {
                "regex": tier1,
                "ner": tier2,
                "slm": tier3,
            }
        }
    });
    if let Some(mid) = model_id {
        patch["pii"]["slm"]["model_id"] = serde_json::Value::String(mid.to_string());
    }

    cfg_mgr.patch(patch).await?;
    cfg_mgr.save_to_disk().await?;
    tracing::info!("wizard: config saved to disk");
    println!("Configuration saved.");
    Ok(())
}

/// If the model file exists, offer to start `llama-server` immediately.
async fn maybe_launch_llama_server(
    theme: &dialoguer::theme::ColorfulTheme,
    model_id: &str,
    models_dir: &Path,
) -> Result<()> {
    use dialoguer::Confirm;

    let mpath = model_path(models_dir, model_id);
    if !mpath.exists() {
        return Ok(());
    }

    let start_now = Confirm::with_theme(theme)
        .with_prompt("Start llama-server now?")
        .default(false)
        .interact()?;

    if start_now {
        let bin = find_llama_server_bin().await?;
        let port: u16 = 16442;
        tracing::warn!(
            bin = %bin.display(),
            model = %mpath.display(),
            port,
            "wizard: starting llama-server"
        );
        let _sidecar =
            crate::pii::tier3::SidecarProcess::start(&bin, &mpath, port, 30u64)?;
        println!("llama-server started on port {}. Press Ctrl+C to stop.", port);
        tokio::signal::ctrl_c().await?;
        println!("\nStopped.");
    }

    Ok(())
}

/// Locate the `llama-server` binary via `which`.
async fn find_llama_server_bin() -> Result<std::path::PathBuf> {
    let output = tokio::process::Command::new("which")
        .arg("llama-server")
        .output()
        .await?;
    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok(std::path::PathBuf::from(path_str));
    }
    anyhow::bail!("llama-server not found in PATH. Install it first.");
}
