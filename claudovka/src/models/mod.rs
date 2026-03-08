use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

/// Entry in the built-in model catalog.
pub struct ModelInfo {
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

/// Returns the built-in catalog of available models.
pub fn catalog() -> &'static [ModelInfo] {
    static CATALOG: &[ModelInfo] = &[
        ModelInfo {
            name: "gliner-pii-base",
            description: "GLiNER NER model for PII detection",
            url: "https://huggingface.co/urchade/gliner_medium-v2.1/resolve/main/onnx/model.onnx",
            sha256: "",
            size_mb: 260,
        },
        ModelInfo {
            name: "anonymizer-slm",
            description: "Small language model for PII disambiguation (GGUF)",
            url: "",
            sha256: "",
            size_mb: 1800,
        },
    ];
    CATALOG
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
        .find(|m| m.name == name)
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
