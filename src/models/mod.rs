use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;

/// Entry in the built-in model catalog.
pub struct ModelInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size_mb: u32,
}

/// A model that is already present on disk.
pub struct InstalledModel {
    pub name: String,
    pub path: std::path::PathBuf,
    pub size_bytes: u64,
}

/// JSON-serializable model entry for the REST API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub size_mb: u32,
    pub downloaded: bool,
    pub active: bool,
    pub download_progress: Option<f64>,
}

/// Tracks in-progress downloads so they can be cancelled.
/// Key: model id. Value: a sender that cancels the download when dropped or messaged.
#[derive(Default, Clone)]
pub struct DownloadTracker {
    inner: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>,
}

impl DownloadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new download. Returns a receiver that fires when cancel() is called.
    pub fn register(&self, id: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.inner.lock().unwrap().insert(id.to_string(), tx);
        rx
    }

    /// Cancel an in-progress download for `id`. No-op if not downloading.
    pub fn cancel(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
        // Dropping the sender signals the receiver.
    }

    /// True if a download is currently registered for `id`.
    pub fn is_downloading(&self, id: &str) -> bool {
        self.inner.lock().unwrap().contains_key(id)
    }

    /// Remove the registration once complete (without cancelling).
    pub fn complete(&self, id: &str) {
        self.inner.lock().unwrap().remove(id);
    }
}

/// Returns the built-in catalog of available models.
pub fn catalog() -> &'static [ModelInfo] {
    static CATALOG: &[ModelInfo] = &[
        ModelInfo {
            id: "smollm2-135m",
            name: "SmolLM2-135M-Instruct",
            description: "Smallest SLM for PII disambiguation (~105 MB, ~300 MB RAM)",
            url: "https://huggingface.co/bartowski/SmolLM2-135M-Instruct-GGUF/resolve/main/SmolLM2-135M-Instruct-Q4_K_M.gguf",
            sha256: "2e8040ceae7815abe0dcb3540b9995eaa1fa0d2ca9e797d0a635ae4433c68c2d",
            size_mb: 105,
        },
        ModelInfo {
            id: "qwen2.5-0.5b",
            name: "Qwen2.5-0.5B-Instruct",
            description: "Compact SLM for PII disambiguation (~400 MB, ~800 MB RAM)",
            url: "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/main/qwen2.5-0.5b-instruct-q4_k_m.gguf",
            sha256: "a0ee18ee2bcb22c2b6c95360b292c2c40a2d7a03",
            size_mb: 400,
        },
        ModelInfo {
            id: "llama-3.2-1b",
            name: "Llama-3.2-1B-Instruct",
            description: "Balanced SLM for PII disambiguation (~700 MB, ~1.2 GB RAM)",
            url: "https://huggingface.co/bartowski/Llama-3.2-1B-Instruct-GGUF/resolve/main/Llama-3.2-1B-Instruct-Q4_K_M.gguf",
            sha256: "ca7732416a22ac248814aadae1fb22505e475a92",
            size_mb: 700,
        },
        ModelInfo {
            id: "phi-3-mini-3.8b",
            name: "Phi-3-mini-4k-instruct",
            description: "High-accuracy SLM for PII disambiguation (~2.3 GB, ~3.5 GB RAM)",
            url: "https://huggingface.co/microsoft/Phi-3-mini-4k-instruct-gguf/resolve/main/Phi-3-mini-4k-instruct-q4.gguf",
            sha256: "c72c1922442b8e09192da8d5e497a2738dec9d1b",
            size_mb: 2300,
        },
    ];
    CATALOG
}

/// Returns the on-disk GGUF path for a catalog entry (preferred) or ONNX path.
pub fn model_path(models_dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    let gguf = models_dir.join(format!("{}.gguf", id));
    if gguf.exists() {
        return gguf;
    }
    models_dir.join(format!("{}.onnx", id))
}

/// Returns true if a model file for `id` exists on disk.
pub fn is_downloaded(models_dir: &std::path::Path, id: &str) -> bool {
    models_dir.join(format!("{}.gguf", id)).exists()
        || models_dir.join(format!("{}.onnx", id)).exists()
}

/// Merges the catalog with disk state, in-progress downloads, and the active model selection.
///
/// A model is considered "downloaded" if a file named `{id}.onnx` or `{id}.gguf`
/// exists inside `models_dir`.
pub fn get_api_entries(
    models_dir: &std::path::Path,
    active_model_id: Option<&str>,
    tracker: Option<&DownloadTracker>,
) -> Vec<ModelEntry> {
    catalog()
        .iter()
        .map(|info| {
            let downloaded = is_downloaded(models_dir, info.id);
            let active = active_model_id.map(|id| id == info.id).unwrap_or(false);
            let download_progress = if !downloaded {
                tracker.and_then(|t| if t.is_downloading(info.id) { Some(-1.0) } else { None })
            } else {
                None
            };
            ModelEntry {
                id: info.id.to_string(),
                name: info.name.to_string(),
                description: info.description.to_string(),
                size_mb: info.size_mb,
                downloaded,
                active,
                download_progress,
            }
        })
        .collect()
}

/// A WS-compatible progress event emitted during model downloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDownloadProgressEvent {
    pub model_id: String,
    /// 0–100, or -1 when total size is unknown.
    pub progress: i32,
    pub bytes_downloaded: u64,
    pub bytes_total: Option<u64>,
}

/// A WS-compatible error event emitted when a download fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDownloadErrorEvent {
    pub model_id: String,
    pub message: String,
}

/// Spawns a background download task for `model_id`.
///
/// Progress is broadcast via `progress_tx`.  If `cancel_rx` fires (or is dropped)
/// the download is aborted and the partial file is deleted.
pub fn start_background_download(
    model_id: String,
    models_dir: std::path::PathBuf,
    tracker: DownloadTracker,
    progress_tx: broadcast::Sender<ModelDownloadProgressEvent>,
    error_tx: broadcast::Sender<ModelDownloadErrorEvent>,
) {
    let cancel_rx = tracker.register(&model_id);
    tokio::spawn(async move {
        let result = tokio::select! {
            r = download_with_progress(&model_id, &models_dir, &progress_tx) => r,
            _ = cancel_rx => Err(anyhow::anyhow!("download cancelled by user")),
        };
        tracker.complete(&model_id);
        if let Err(e) = result {
            // Clean up any partial file.
            for ext in &["gguf", "onnx"] {
                let _ = tokio::fs::remove_file(models_dir.join(format!("{model_id}.{ext}"))).await;
            }
            let _ = error_tx.send(ModelDownloadErrorEvent {
                model_id,
                message: e.to_string(),
            });
        }
    });
}

async fn download_with_progress(
    model_id: &str,
    models_dir: &std::path::Path,
    progress_tx: &broadcast::Sender<ModelDownloadProgressEvent>,
) -> anyhow::Result<()> {
    let info = catalog()
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| anyhow::anyhow!("Model '{}' not found in catalog", model_id))?;

    if info.url.is_empty() {
        anyhow::bail!("Model '{}' has no download URL configured", model_id);
    }

    tokio::fs::create_dir_all(models_dir).await?;

    let filename = info
        .url
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .unwrap_or(model_id);
    let dest = models_dir.join(filename);

    let client = reqwest::Client::new();
    let response = client
        .get(info.url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Network error: {}", e))?;

    if !response.status().is_success() {
        anyhow::bail!("Server returned {}", response.status());
    }

    let total = response.content_length();
    let mut file = tokio::fs::File::create(&dest).await?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    let report_interval: u64 = info.size_mb as u64 * 1024 * 1024 / 50; // ~50 events

    let mut stream = response;
    loop {
        let chunk = stream
            .chunk()
            .await
            .map_err(|e| anyhow::anyhow!("Error reading response: {}", e))?;
        let Some(bytes) = chunk else { break };
        hasher.update(&bytes);
        file.write_all(&bytes).await?;
        downloaded += bytes.len() as u64;

        // Emit progress every ~2 % or at least every 4 MB.
        let threshold = report_interval.max(4 * 1024 * 1024);
        if downloaded - last_reported >= threshold {
            last_reported = downloaded;
            let progress = total
                .filter(|&t| t > 0)
                .map(|t| ((downloaded as f64 / t as f64) * 100.0) as i32)
                .unwrap_or(-1);
            let _ = progress_tx.send(ModelDownloadProgressEvent {
                model_id: model_id.to_string(),
                progress,
                bytes_downloaded: downloaded,
                bytes_total: total,
            });
        }
    }
    file.flush().await?;
    drop(file);

    if !info.sha256.is_empty() {
        let actual = format!("{:x}", hasher.finalize());
        if actual != info.sha256 {
            anyhow::bail!(
                "Checksum mismatch: expected {}, got {}",
                info.sha256,
                actual
            );
        }
    }

    // Emit 100% completion.
    let _ = progress_tx.send(ModelDownloadProgressEvent {
        model_id: model_id.to_string(),
        progress: 100,
        bytes_downloaded: downloaded,
        bytes_total: total,
    });

    Ok(())
}

/// Downloads a model by name into `models_dir`, verifies its checksum, and
/// prints streaming progress to stdout.
///
/// # Errors
/// Returns an error if:
/// - `name` is not found in the catalog
/// - The catalog entry has an empty URL
/// - A network or I/O error occurs (partial file is deleted on failure)
/// - The sha256 checksum does not match (when the catalog entry is non-empty)
pub async fn install(name: &str, models_dir: &std::path::Path) -> anyhow::Result<()> {
    let info = catalog()
        .iter()
        .find(|m| m.id == name || m.name == name)
        .ok_or_else(|| anyhow::anyhow!("Model '{}' not found in catalog", name))?;

    if info.url.is_empty() {
        anyhow::bail!("Model '{}' has no download URL configured", name);
    }

    tokio::fs::create_dir_all(models_dir).await?;

    // Derive a filename from the URL's last path segment, falling back to the
    // model name with a generic extension.
    let filename = info
        .url
        .split('/')
        .next_back()
        .filter(|s| !s.is_empty())
        .unwrap_or(name);
    let dest = models_dir.join(filename);

    let result = download_to_file(info, &dest).await;
    if result.is_err() {
        // Clean up any partial download.
        let _ = tokio::fs::remove_file(&dest).await;
    }
    result
}

async fn download_to_file(info: &ModelInfo, dest: &std::path::Path) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get(info.url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Network error downloading '{}': {}", info.name, e))?;

    if !response.status().is_success() {
        anyhow::bail!(
            "Server returned {} for model '{}'",
            response.status(),
            info.name
        );
    }

    let total = response.content_length();
    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;

    let mut stream = response;
    loop {
        let chunk = stream
            .chunk()
            .await
            .map_err(|e| anyhow::anyhow!("Error reading response body: {}", e))?;
        let Some(bytes) = chunk else { break };

        hasher.update(&bytes);
        file.write_all(&bytes).await?;
        downloaded += bytes.len() as u64;

        match total {
            Some(t) if t > 0 => print!(
                "\r  Downloading {} … {:.1} / {:.1} MB ({:.0}%)",
                info.name,
                downloaded as f64 / 1_048_576.0,
                t as f64 / 1_048_576.0,
                downloaded as f64 / t as f64 * 100.0,
            ),
            _ => print!(
                "\r  Downloading {} … {:.1} MB",
                info.name,
                downloaded as f64 / 1_048_576.0,
            ),
        }
        // Flush stdout so the progress line updates in place.
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    println!(); // newline after progress line
    file.flush().await?;
    drop(file);

    if !info.sha256.is_empty() {
        let actual = format!("{:x}", hasher.finalize());
        if actual != info.sha256 {
            anyhow::bail!(
                "Checksum mismatch for '{}': expected {}, got {}",
                info.name,
                info.sha256,
                actual
            );
        }
        println!("  Checksum OK for {}", info.name);
    }

    println!("  Installed '{}' → {}", info.name, dest.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_api_entries_returns_all_catalog_entries() {
        let dir = std::path::PathBuf::from("/nonexistent_models_dir_for_test");
        let entries = get_api_entries(&dir, None, None);
        assert_eq!(entries.len(), 4, "catalog should have 4 entries");
    }

    #[test]
    fn get_api_entries_marks_downloaded_when_file_exists() {
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let models_dir = tmp.path();

        // Create a fake .gguf file for the first catalog entry
        let first_id = catalog()[0].id;
        std::fs::write(models_dir.join(format!("{}.gguf", first_id)), b"fake").unwrap();

        let entries = get_api_entries(models_dir, None, None);
        let first = entries.iter().find(|e| e.id == first_id).unwrap();
        assert!(first.downloaded, "first entry should be marked as downloaded");

        // All others should not be downloaded
        for entry in entries.iter().filter(|e| e.id != first_id) {
            assert!(!entry.downloaded, "entry {} should not be downloaded", entry.id);
        }
    }

    #[test]
    fn get_api_entries_marks_active_model() {
        let dir = std::path::PathBuf::from("/nonexistent_models_dir_for_test");
        let entries = get_api_entries(&dir, Some("qwen2.5-0.5b"), None);
        let active: Vec<_> = entries.iter().filter(|e| e.active).collect();
        assert_eq!(active.len(), 1, "exactly one model should be active");
        assert_eq!(active[0].id, "qwen2.5-0.5b");
    }

    #[test]
    fn catalog_ids_match_spec() {
        let ids: Vec<_> = catalog().iter().map(|m| m.id).collect();
        assert!(ids.contains(&"smollm2-135m"),    "missing smollm2-135m");
        assert!(ids.contains(&"qwen2.5-0.5b"),   "missing qwen2.5-0.5b");
        assert!(ids.contains(&"llama-3.2-1b"),   "missing llama-3.2-1b");
        assert!(ids.contains(&"phi-3-mini-3.8b"),"missing phi-3-mini-3.8b");
    }

    #[test]
    fn download_tracker_register_cancel_complete() {
        let tracker = DownloadTracker::new();
        assert!(!tracker.is_downloading("foo"));
        let mut rx = tracker.register("foo");
        assert!(tracker.is_downloading("foo"));
        tracker.cancel("foo");
        assert!(!tracker.is_downloading("foo"));
        // The receiver should be closed (sender was dropped by cancel).
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn is_downloaded_detects_gguf_and_onnx() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(!is_downloaded(dir, "mymodel"));
        std::fs::write(dir.join("mymodel.gguf"), b"x").unwrap();
        assert!(is_downloaded(dir, "mymodel"));
    }
}

/// Downloads a catalog model to `models_dir`, verifying its checksum, and
/// renders an `indicatif` progress bar to the terminal.
///
/// Uses a `.tmp` staging file so a failed download never leaves a partial
/// GGUF on disk.
pub async fn download_with_bar(info: &'static ModelInfo, models_dir: &std::path::Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(models_dir).await?;

    let filename = info
        .url
        .rsplit('/')
        .next()
        .unwrap_or(info.id);
    let dest = models_dir.join(filename);
    let tmp = dest.with_extension("tmp");

    tracing::info!(
        model_id = info.id,
        dest = %dest.display(),
        "downloading model"
    );

    let client = reqwest::Client::new();
    let resp = client.get(info.url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("download failed: HTTP {}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template("{msg} [{wide_bar}] {bytes}/{total_bytes} ({eta})")?
            .progress_chars("=>-"),
    );
    pb.set_message(info.name.to_string());

    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut stream = resp;
    let mut downloaded = 0u64;
    let mut hasher = Sha256::new();
    loop {
        let chunk = stream.chunk().await?;
        let Some(bytes) = chunk else { break };
        hasher.update(&bytes);
        file.write_all(&bytes).await?;
        downloaded += bytes.len() as u64;
        pb.set_position(downloaded);
    }
    file.flush().await?;
    drop(file);
    pb.finish_with_message(format!("{} complete", info.name));

    if !info.sha256.is_empty() {
        let actual = format!("{:x}", hasher.finalize());
        if actual != info.sha256 {
            tokio::fs::remove_file(&tmp).await.ok();
            anyhow::bail!(
                "checksum mismatch: expected {}, got {}",
                info.sha256,
                actual
            );
        }
        tracing::info!(model_id = info.id, "checksum ok");
    }

    tokio::fs::rename(&tmp, &dest).await?;
    println!("Model saved to {}", dest.display());
    Ok(())
}

/// Lists all `.onnx` and `.gguf` files present in `models_dir`.
///
/// Returns an empty `Vec` (not an error) when `models_dir` does not exist.
pub fn list_installed(models_dir: &std::path::Path) -> anyhow::Result<Vec<InstalledModel>> {
    if !models_dir.exists() {
        return Ok(Vec::new());
    }

    let mut models = Vec::new();
    for entry in std::fs::read_dir(models_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "onnx" && ext != "gguf" {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let size_bytes = entry.metadata()?.len();
        models.push(InstalledModel {
            name,
            path,
            size_bytes,
        });
    }
    Ok(models)
}
