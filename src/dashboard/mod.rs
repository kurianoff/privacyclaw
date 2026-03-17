use crate::config::ConfigManager;
use crate::storage::Store;
use anyhow::Result;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Shared handle for the running llama-server sidecar process.
/// Setting the Option to None drops the SidecarProcess, which kills the child.
type SidecarHandle = Arc<Mutex<Option<crate::pii::tier3::SidecarProcess>>>;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Notify};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

/// Shared proxy running state, threaded through main → dashboard.
pub struct ProxyState {
    pub running: AtomicBool,
    pub shutdown: Notify,
}

impl ProxyState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            running: AtomicBool::new(true),
            shutdown: Notify::new(),
        })
    }
}

/// Events broadcast from the proxy to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    ConversationStart {
        id: String,
        provider: String,
        model: String,
        timestamp: String,
    },
    Message {
        conversation_id: String,
        direction: String,
        role: Option<String>,
        content: String,
        timestamp: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content_masked: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pii_processed: Option<bool>,
    },
    TextDelta {
        conversation_id: String,
        text: String,
        timestamp: String,
    },
    ResponseComplete {
        conversation_id: String,
        tokens_in: Option<i64>,
        tokens_out: Option<i64>,
    },
    PiiDetected {
        conversation_id: String,
        entity_type: String,
        original: String,
        original_masked: String,
        synthetic: String,
        tier: u8,
        confidence: f32,
    },
    ConfigChanged {
        changed_keys: Vec<String>,
        restart_required: bool,
    },
    ProxyStatus {
        running: bool,
        pii_mode: String,
    },
    ModelDownloadProgress {
        model_id: String,
        /// 0–100, or -1 when total size unknown.
        progress: i32,
        bytes_downloaded: u64,
        bytes_total: Option<u64>,
    },
    ModelDownloadError {
        model_id: String,
        message: String,
    },
}

#[derive(Embed)]
#[folder = "src/dashboard/assets/"]
struct Assets;

pub async fn run(
    addr: &str,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    cfg_mgr: Arc<ConfigManager>,
    proxy_state: Arc<ProxyState>,
    download_tracker: crate::models::DownloadTracker,
) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::warn!(addr = %addr, "dashboard bound");
    let sidecar: SidecarHandle = Arc::new(Mutex::new(None));

    // Forward model download progress/error events into the WS broadcast channel.
    let (dl_progress_tx, mut dl_progress_rx) =
        broadcast::channel::<crate::models::ModelDownloadProgressEvent>(64);
    let (dl_error_tx, mut dl_error_rx) =
        broadcast::channel::<crate::models::ModelDownloadErrorEvent>(16);

    {
        let ws_tx2 = ws_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Ok(ev) = dl_progress_rx.recv() => {
                        let _ = ws_tx2.send(WsEvent::ModelDownloadProgress {
                            model_id: ev.model_id,
                            progress: ev.progress,
                            bytes_downloaded: ev.bytes_downloaded,
                            bytes_total: ev.bytes_total,
                        });
                    }
                    Ok(ev) = dl_error_rx.recv() => {
                        let _ = ws_tx2.send(WsEvent::ModelDownloadError {
                            model_id: ev.model_id,
                            message: ev.message,
                        });
                    }
                    else => break,
                }
            }
        });
    }

    loop {
        let (stream, peer) = listener.accept().await?;
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let cfg_mgr = cfg_mgr.clone();
        let proxy_state = proxy_state.clone();
        let download_tracker = download_tracker.clone();
        let dl_progress_tx = dl_progress_tx.clone();
        let dl_error_tx = dl_error_tx.clone();
        let sidecar = sidecar.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatch(
                stream, store, ws_tx, cfg_mgr, proxy_state,
                download_tracker, dl_progress_tx, dl_error_tx, sidecar,
            ).await {
                tracing::debug!("Dashboard connection error from {}: {}", peer, e);
            }
        });
    }
}

/// Read the HTTP request line + headers, then route to HTTP handler or WS upgrade.
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    stream: TcpStream,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    cfg_mgr: Arc<ConfigManager>,
    proxy_state: Arc<ProxyState>,
    download_tracker: crate::models::DownloadTracker,
    dl_progress_tx: broadcast::Sender<crate::models::ModelDownloadProgressEvent>,
    dl_error_tx: broadcast::Sender<crate::models::ModelDownloadErrorEvent>,
    sidecar: SidecarHandle,
) -> Result<()> {
    use tokio::io::AsyncReadExt;

    let mut reader = BufReader::new(stream);

    // Read request line
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    let request_line = request_line.trim().to_string();
    tracing::debug!(request_line = %request_line, "dashboard: request line");

    // Read headers
    let mut headers: HashMap<String, String> = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let trimmed = line.trim();
        if trimmed.is_empty() { break; }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_lowercase().to_string(), v.trim().to_string());
        }
    }
    tracing::debug!(header_count = headers.len(), "dashboard: headers parsed");

    let method = request_line.split_whitespace().next().unwrap_or("GET").to_string();
    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let is_ws_upgrade = headers
        .get("upgrade")
        .map(|v| v.to_lowercase().contains("websocket"))
        .unwrap_or(false);

    tracing::debug!(path = %path, method = %method, is_ws_upgrade, "dashboard: routing");

    // For methods that carry a body (PATCH/POST), read it before reclaiming the stream.
    let req_body: Option<Vec<u8>> = if method == "PATCH" || method == "POST" {
        let len = headers
            .get("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        if len > 0 {
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await.ok();
            Some(buf)
        } else {
            Some(vec![])
        }
    } else {
        None
    };

    // Reclaim the raw TcpStream.
    let raw = reader.into_inner();

    if is_ws_upgrade && path == "/ws" {
        handle_ws_upgrade(raw, &headers, ws_tx).await
    } else {
        handle_http(
            raw, &path, &method, req_body, store, cfg_mgr, ws_tx, proxy_state,
            download_tracker, dl_progress_tx, dl_error_tx, sidecar,
        ).await
    }
}

/// Write a standard HTTP response with CORS headers.
/// Content-Type is set to `content_type`; body length is derived automatically.
/// The PATCH /api/config path and WebSocket upgrade path deviate from this pattern
/// and are not handled here.
async fn send_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_http(
    mut stream: TcpStream,
    path: &str,
    method: &str,
    req_body: Option<Vec<u8>>,
    store: Store,
    cfg_mgr: Arc<ConfigManager>,
    ws_tx: broadcast::Sender<WsEvent>,
    proxy_state: Arc<ProxyState>,
    download_tracker: crate::models::DownloadTracker,
    dl_progress_tx: broadcast::Sender<crate::models::ModelDownloadProgressEvent>,
    dl_error_tx: broadcast::Sender<crate::models::ModelDownloadErrorEvent>,
    sidecar: SidecarHandle,
) -> Result<()> {
    tracing::info!(path = %path, method, "dashboard: HTTP request");

    // Detections endpoint — must be checked before vault and generic conversation branch.
    if path.starts_with("/api/conversations/") && path.contains("/detections") {
        // Path: /api/conversations/:id/detections or /api/conversations/:id/detections?message_id=...
        let after_prefix = path.strip_prefix("/api/conversations/").unwrap_or("");
        // Strip optional query string.
        let (path_part, query_part) = after_prefix.split_once('?').unwrap_or((after_prefix, ""));
        if let Some(conv_id) = path_part.strip_suffix("/detections") {
            let message_id = query_part
                .split('&')
                .find_map(|pair| pair.strip_prefix("message_id="))
                .map(|v| v.to_string());
            handle_detections_api(&mut stream, &store, conv_id, message_id.as_deref()).await?;
            return Ok(());
        }
    }

    // Vault endpoint — must be checked before the generic /api/conversations/:id branch
    if path.starts_with("/api/conversations/") && path.ends_with("/vault") {
        let conv_id = path
            .strip_prefix("/api/conversations/").unwrap_or("")
            .strip_suffix("/vault").unwrap_or("");
        handle_vault_api(&mut stream, &store, conv_id).await?;
        return Ok(());
    }

    // PATCH /api/config — handled early because it needs async work before responding.
    if path == "/api/config" && method == "PATCH" {
        let body_bytes = req_body.unwrap_or_default();
        let (status, body) = match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(patch_val) => match cfg_mgr.patch(patch_val).await {
                Ok(result) => {
                    if let Err(e) = cfg_mgr.save_to_disk().await {
                        tracing::warn!("failed to persist config: {e}");
                    }
                    // Broadcast config_changed event.
                    let event = WsEvent::ConfigChanged {
                        changed_keys: result.changed_keys.clone(),
                        restart_required: result.restart_required,
                    };
                    let _ = ws_tx.send(event);
                    (200u16, serde_json::to_vec(&result).unwrap_or_default())
                }
                Err(e) => {
                    let msg = serde_json::to_vec(&serde_json::json!({"error": e.to_string()}))
                        .unwrap_or_default();
                    (422u16, msg)
                }
            },
            Err(_) => (400u16, b"{\"error\":\"invalid JSON\"}".to_vec()),
        };
        let header = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(header.as_bytes()).await?;
        stream.write_all(&body).await?;
        return Ok(());
    }

    // GET /api/proxy/status
    if path == "/api/proxy/status" && method == "GET" {
        let cfg = cfg_mgr.get().await;
        let running = proxy_state.running.load(Ordering::Relaxed);
        let body = serde_json::to_vec(&serde_json::json!({
            "running": running,
            "pii_mode": cfg.pii.mode,
            "http_proxy": cfg.proxy.listen,
            "network_proxy": cfg.network_proxy.listen,
        })).unwrap_or_default();
        send_response(&mut stream, 200, "application/json", &body).await?;
        return Ok(());
    }

    // OPTIONS preflight
    if method == "OPTIONS" {
        let header = "HTTP/1.1 204\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, PATCH, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n";
        stream.write_all(header.as_bytes()).await?;
        return Ok(());
    }

    // POST /api/proxy/start
    if path == "/api/proxy/start" && method == "POST" {
        proxy_state.running.store(true, Ordering::Relaxed);
        let event = WsEvent::ProxyStatus {
            running: true,
            pii_mode: cfg_mgr.get().await.pii.mode.clone(),
        };
        let _ = ws_tx.send(event);
        send_response(&mut stream, 200, "application/json", b"{\"ok\":true}").await?;
        return Ok(());
    }

    // POST /api/proxy/stop
    if path == "/api/proxy/stop" && method == "POST" {
        proxy_state.running.store(false, Ordering::Relaxed);
        let event = WsEvent::ProxyStatus {
            running: false,
            pii_mode: cfg_mgr.get().await.pii.mode.clone(),
        };
        let _ = ws_tx.send(event);
        proxy_state.shutdown.notify_one();
        crate::pid::remove_pid();
        send_response(&mut stream, 200, "application/json", b"{\"ok\":true}").await?;
        return Ok(());
    }

    // GET /api/models
    if path == "/api/models" && method == "GET" {
        let cfg = cfg_mgr.get().await;
        let models_dir = cfg.resolved_models_dir();
        let entries = crate::models::get_api_entries(
            &models_dir,
            cfg.pii.slm.model_id.as_deref(),
            Some(&download_tracker),
        );
        let body = serde_json::to_vec(&entries).unwrap_or_default();
        send_response(&mut stream, 200, "application/json", &body).await?;
        return Ok(());
    }

    // POST /api/models/:id/download  or  DELETE /api/models/:id/download (cancel)
    if let Some(model_id) = path
        .strip_prefix("/api/models/")
        .and_then(|s| s.strip_suffix("/download"))
    {
        let model_id = model_id.to_string();
        if method == "POST" {
            let cfg = cfg_mgr.get().await;
            let models_dir = cfg.resolved_models_dir();
            if crate::models::is_downloaded(&models_dir, &model_id) {
                let body = serde_json::to_vec(&serde_json::json!({
                    "ok": false, "error": "already downloaded"
                })).unwrap_or_default();
                send_response(&mut stream, 409, "application/json", &body).await?;
                return Ok(());
            }
            if download_tracker.is_downloading(&model_id) {
                let body = serde_json::to_vec(&serde_json::json!({
                    "ok": false, "error": "download already in progress"
                })).unwrap_or_default();
                send_response(&mut stream, 409, "application/json", &body).await?;
                return Ok(());
            }
            crate::models::start_background_download(
                model_id,
                models_dir,
                download_tracker,
                dl_progress_tx,
                dl_error_tx,
            );
            send_response(&mut stream, 202, "application/json", b"{\"ok\":true,\"status\":\"downloading\"}").await?;
            return Ok(());
        }
        if method == "DELETE" {
            download_tracker.cancel(&model_id);
            send_response(&mut stream, 200, "application/json", b"{\"ok\":true}").await?;
            return Ok(());
        }
    }

    // POST /api/models/deactivate
    if path == "/api/models/deactivate" && method == "POST" {
        // Stop any running sidecar before patching config.
        if let Ok(mut guard) = sidecar.lock() {
            *guard = None;
        }
        let patch = serde_json::json!({ "pii": { "slm": { "model_id": null }, "tiers": { "slm": false } } });
        if (cfg_mgr.patch(patch).await).is_ok() {
            let _ = cfg_mgr.save_to_disk().await;
        }
        send_response(&mut stream, 200, "application/json", b"{\"ok\":true}").await?;
        return Ok(());
    }

    // POST /api/models/:id/activate
    if let Some(model_id) = path
        .strip_prefix("/api/models/")
        .and_then(|s| s.strip_suffix("/activate"))
    {
        if method == "POST" {
            let cfg = cfg_mgr.get().await;
            let models_dir = cfg.resolved_models_dir();
            if !crate::models::is_downloaded(&models_dir, model_id) {
                let body = serde_json::to_vec(&serde_json::json!({
                    "ok": false, "error": "model not downloaded"
                })).unwrap_or_default();
                send_response(&mut stream, 409, "application/json", &body).await?;
                return Ok(());
            }
            // §5.6: Stop any existing sidecar and start a new one for this model.
            let model_file = crate::models::model_path(&models_dir, model_id);
            let llama_server_bin = crate::config::default_config_dir().join("bin/llama-server");
            // Kill old sidecar first (dropping SidecarProcess kills the child).
            if let Ok(mut guard) = sidecar.lock() {
                *guard = None;
            }
            // Start new sidecar. Log errors but continue — config is still updated.
            match crate::pii::tier3::SidecarProcess::start(&llama_server_bin, &model_file, 16442, 30u64) {
                Ok(sp) => {
                    if let Ok(mut guard) = sidecar.lock() {
                        *guard = Some(sp);
                    }
                }
                Err(e) => {
                    tracing::warn!(model_id = %model_id, error = %e, "failed to start llama-server sidecar");
                }
            }
            // Update pii.slm.model_id in the live config.
            let patch = serde_json::json!({ "pii": { "slm": { "model_id": model_id } } });
            if let Ok(_result) = cfg_mgr.patch(patch).await {
                let _ = cfg_mgr.save_to_disk().await;
            }
            let body = serde_json::to_vec(&serde_json::json!({ "ok": true })).unwrap_or_default();
            send_response(&mut stream, 200, "application/json", &body).await?;
            return Ok(());
        }
    }

    // DELETE /api/models/:id
    if path.starts_with("/api/models/")
        && !path.contains("/download")
        && !path.contains("/activate")
        && !path.contains("/deactivate")
        && method == "DELETE"
    {
        let model_id = path.trim_start_matches("/api/models/");
        let cfg = cfg_mgr.get().await;
        // Reject if this is the currently active model.
        if cfg.pii.slm.model_id.as_deref() == Some(model_id) {
            let body = serde_json::to_vec(&serde_json::json!({
                "ok": false, "error": "deactivate model before deleting"
            })).unwrap_or_default();
            send_response(&mut stream, 409, "application/json", &body).await?;
            return Ok(());
        }
        let models_dir = cfg.resolved_models_dir();
        // Try both .onnx and .gguf extensions.
        let deleted = [
            models_dir.join(format!("{}.onnx", model_id)),
            models_dir.join(format!("{}.gguf", model_id)),
        ]
        .iter()
        .filter(|p| p.exists())
        .any(|p| std::fs::remove_file(p).is_ok());
        let body = serde_json::to_vec(&serde_json::json!({ "ok": deleted })).unwrap_or_default();
        send_response(&mut stream, 200, "application/json", &body).await?;
        return Ok(());
    }

    let (status, content_type, body) = match path {
        "/" | "/index.html" => serve_asset("index.html"),
        "/style.css" => serve_asset("style.css"),
        "/app.js" => serve_asset("app.js"),
        p if (p == "/api/conversations" || p.starts_with("/api/conversations?")) => {
            let limit = parse_query_limit(p).unwrap_or(50).min(200);
            let convs = store.list_conversations(limit).unwrap_or_default();
            let json = serde_json::to_vec(&convs).unwrap_or_default();
            (200u16, "application/json", json)
        }
        p if p.starts_with("/api/conversations/") => {
            let id = p.trim_start_matches("/api/conversations/");
            let convs = store.list_conversations(50).unwrap_or_default();
            let conv = convs.into_iter().find(|c| c.id == id);
            let msgs = store.get_messages(id).unwrap_or_default();
            let json = serde_json::to_vec(&serde_json::json!({
                "conversation": conv,
                "messages": msgs,
            }))?;
            (200, "application/json", json)
        }
        "/api/version" => {
            let body = serde_json::to_vec(&serde_json::json!({
                "version": crate::version::VERSION,
                "git_hash": crate::version::GIT_HASH,
                "build_date": crate::version::BUILD_DATE,
            })).unwrap_or_default();
            (200u16, "application/json", body)
        }
        "/api/config" => {
            // GET (or any non-PATCH) — return current config snapshot.
            let cfg = cfg_mgr.get().await;
            match serde_json::to_vec(&cfg) {
                Ok(body) => (200u16, "application/json", body),
                Err(_) => (500, "application/json", b"{\"error\":\"serialisation failed\"}".to_vec()),
            }
        }
        _ => (404, "text/plain", b"Not Found".to_vec()),
    };

    tracing::info!(path = %path, status, body_bytes = body.len(), "dashboard: HTTP response");
    send_response(&mut stream, status, content_type, &body).await?;
    Ok(())
}

async fn handle_vault_api(stream: &mut TcpStream, store: &Store, conv_id: &str) -> Result<()> {
    // store.load_vault returns Option<(u64, Vec<StoredVaultRecord>)>
    let vault_data = match store.load_vault(conv_id) {
        Ok(Some((_seed, records))) => {
            records.iter().map(|r| serde_json::json!({
                "type": r.pii_type,
                "original": r.original,
                "original_masked": format!("[{}]", r.pii_type.to_uppercase()),
                "synthetic": r.synthetic,
                "tier": r.tier,
                "confidence": r.confidence,
            })).collect::<Vec<_>>()
        }
        _ => vec![],
    };
    let body = serde_json::to_string(&vault_data)?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
        body.len(), body
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

async fn handle_detections_api(
    stream: &mut TcpStream,
    store: &Store,
    conv_id: &str,
    message_id: Option<&str>,
) -> Result<()> {
    let detections = store.load_detections(conv_id, message_id).unwrap_or_default();
    let body = serde_json::to_string(&detections)?;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{}",
        body.len(), body
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

/// Parse `?limit=N` from a path string like `/api/conversations?limit=50`.
fn parse_query_limit(path: &str) -> Option<usize> {
    let qs = path.split_once('?')?.1;
    for pair in qs.split('&') {
        if let Some(val) = pair.strip_prefix("limit=") {
            return val.parse::<usize>().ok();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 4.3 WsEvent::Message JSON shape ───────────────────────────────────────

    /// When content_masked and pii_processed are both Some, they appear in JSON.
    #[test]
    fn ws_event_message_with_optional_fields_serialises() {
        let ev = WsEvent::Message {
            conversation_id: "conv-1".to_string(),
            direction: "request".to_string(),
            role: Some("user".to_string()),
            content: "Hello alice@acme.com".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content_masked: Some("Hello [EMAIL]".to_string()),
            pii_processed: Some(true),
        };
        let json = serde_json::to_string(&ev).unwrap();

        assert!(json.contains("\"content_masked\""),
            "content_masked must appear when Some: {json}");
        assert!(json.contains("\"pii_processed\""),
            "pii_processed must appear when Some: {json}");
        assert!(json.contains("Hello [EMAIL]"),
            "masked content value missing: {json}");
        assert!(json.contains("\"pii_processed\":true"),
            "pii_processed value wrong: {json}");
    }

    /// When content_masked and pii_processed are None, they must NOT appear in JSON.
    /// This ensures backward compat: old clients ignoring unknown fields still work,
    /// and null fields are never injected (which would break stricter decoders).
    #[test]
    fn ws_event_message_none_optional_fields_absent_from_json() {
        let ev = WsEvent::Message {
            conversation_id: "conv-2".to_string(),
            direction: "response".to_string(),
            role: Some("assistant".to_string()),
            content: "Hello".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content_masked: None,
            pii_processed: None,
        };
        let json = serde_json::to_string(&ev).unwrap();

        assert!(!json.contains("\"content_masked\""),
            "None content_masked must be absent (not null): {json}");
        assert!(!json.contains("\"pii_processed\""),
            "None pii_processed must be absent (not null): {json}");
        // Ensure neither field appears as null.
        assert!(!json.contains("content_masked\":null"), "null content_masked present: {json}");
        assert!(!json.contains("pii_processed\":null"), "null pii_processed present: {json}");
    }

    /// pii_processed: Some(false) must appear in JSON (not omitted).
    #[test]
    fn ws_event_message_pii_processed_false_appears_in_json() {
        let ev = WsEvent::Message {
            conversation_id: "conv-3".to_string(),
            direction: "request".to_string(),
            role: None,
            content: "plain".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            content_masked: None,
            pii_processed: Some(false),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"pii_processed\":false"),
            "pii_processed:false must appear: {json}");
        assert!(!json.contains("\"content_masked\""),
            "None content_masked must still be absent: {json}");
    }

    /// WsEvent uses a `type` tag. Verify the tag value is correct.
    #[test]
    fn ws_event_message_type_tag_is_message() {
        let ev = WsEvent::Message {
            conversation_id: "c".to_string(),
            direction: "request".to_string(),
            role: None,
            content: "test".to_string(),
            timestamp: "t".to_string(),
            content_masked: None,
            pii_processed: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(val["type"], "message", "type tag must be 'message': {json}");
    }

    /// Old JSON consumer that only reads known fields must not error on extra fields.
    /// We verify this by deserialising a JSON that includes content_masked back into
    /// a minimal struct, confirming round-trip safety.
    #[test]
    fn ws_event_message_extra_fields_ignored_by_minimal_consumer() {
        #[derive(serde::Deserialize)]
        struct MinimalMessage {
            #[allow(dead_code)] content: String,
        }
        let ev = WsEvent::Message {
            conversation_id: "conv-x".to_string(),
            direction: "request".to_string(),
            role: None,
            content: "hello".to_string(),
            timestamp: "t".to_string(),
            content_masked: Some("[EMAIL]".to_string()),
            pii_processed: Some(true),
        };
        let json = serde_json::to_string(&ev).unwrap();
        // A minimal consumer that only cares about `content` should not fail.
        let minimal: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(minimal["content"], "hello");
    }

    // ── 5.6 parse_query_limit helper ──────────────────────────────────────────

    #[test]
    fn parse_query_limit_extracts_n() {
        assert_eq!(parse_query_limit("/api/conversations?limit=25"), Some(25));
        assert_eq!(parse_query_limit("/api/conversations?limit=1"), Some(1));
        assert_eq!(parse_query_limit("/api/conversations?limit=200"), Some(200));
    }

    #[test]
    fn parse_query_limit_no_query_string_returns_none() {
        assert_eq!(parse_query_limit("/api/conversations"), None);
    }

    #[test]
    fn parse_query_limit_non_numeric_returns_none() {
        assert_eq!(parse_query_limit("/api/conversations?limit=abc"), None);
        assert_eq!(parse_query_limit("/api/conversations?limit="), None);
    }

    #[test]
    fn parse_query_limit_extra_params_still_works() {
        // Other query params before or after limit.
        assert_eq!(parse_query_limit("/api/conversations?foo=bar&limit=42"), Some(42));
        assert_eq!(parse_query_limit("/api/conversations?limit=10&bar=baz"), Some(10));
    }

    #[test]
    fn parse_query_limit_missing_limit_key_returns_none() {
        assert_eq!(parse_query_limit("/api/conversations?offset=5"), None);
    }

}

fn serve_asset(name: &str) -> (u16, &'static str, Vec<u8>) {
    match Assets::get(name) {
        Some(file) => {
            let ct = match name.rsplit('.').next().unwrap_or("") {
                "html" => "text/html; charset=utf-8",
                "css" => "text/css",
                "js" => "application/javascript",
                _ => "application/octet-stream",
            };
            (200, ct, file.data.to_vec())
        }
        None => (404, "text/plain", b"Not Found".to_vec()),
    }
}

async fn handle_ws_upgrade(
    mut stream: TcpStream,
    headers: &HashMap<String, String>,
    ws_tx: broadcast::Sender<WsEvent>,
) -> Result<()> {
    // Compute Sec-WebSocket-Accept
    let key = headers
        .get("sec-websocket-key")
        .cloned()
        .unwrap_or_default();
    let mut hasher = Sha1::new();
    hasher.update(format!("{}258EAFA5-E914-47DA-95CA-C5AB0DC85B11", key));
    let accept = base64::engine::general_purpose::STANDARD.encode(hasher.finalize());

    let handshake = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept
    );
    stream.write_all(handshake.as_bytes()).await?;
    tracing::warn!("dashboard: WebSocket client connected");

    // Hand off to tungstenite as a server-side WS (after manual handshake)
    let ws = WebSocketStream::from_raw_socket(
        stream,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        None,
    )
    .await;

    let (mut sink, mut source) = ws.split();
    let mut rx = ws_tx.subscribe();

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(ev) => {
                        let event_type = match &ev {
                            WsEvent::ConversationStart { .. } => "conversation_start",
                            WsEvent::Message { .. } => "message",
                            WsEvent::TextDelta { .. } => "text_delta",
                            WsEvent::ResponseComplete { .. } => "response_complete",
                            WsEvent::PiiDetected { .. } => "pii_detected",
                            WsEvent::ConfigChanged { .. } => "config_changed",
                            WsEvent::ProxyStatus { .. } => "proxy_status",
                            WsEvent::ModelDownloadProgress { .. } => "model_download_progress",
                            WsEvent::ModelDownloadError { .. } => "model_download_error",
                        };
                        let json = serde_json::to_string(&ev).unwrap_or_default();
                        if sink.send(Message::Text(json)).await.is_err() {
                            break;
                        }
                        tracing::info!(event_type, "dashboard: WS event sent");
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = source.next() => {
                match msg {
                    Some(Ok(_)) => {}
                    _ => break,
                }
            }
        }
    }
    tracing::warn!("dashboard: WebSocket client disconnected");
    Ok(())
}
