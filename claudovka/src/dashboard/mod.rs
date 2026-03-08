use crate::storage::Store;
use anyhow::Result;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

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
        original_masked: String, // e.g. "[EMAIL]" — NOT the real value
        synthetic: String,
        tier: u8,
        confidence: f32,
    },
}

#[derive(Embed)]
#[folder = "src/dashboard/assets/"]
struct Assets;

pub async fn run(addr: &str, store: Store, ws_tx: broadcast::Sender<WsEvent>) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::warn!(addr = %addr, "dashboard bound");

    loop {
        let (stream, peer) = listener.accept().await?;
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = dispatch(stream, store, ws_tx).await {
                tracing::debug!("Dashboard connection error from {}: {}", peer, e);
            }
        });
    }
}

/// Read the HTTP request line + headers, then route to HTTP handler or WS upgrade.
async fn dispatch(stream: TcpStream, store: Store, ws_tx: broadcast::Sender<WsEvent>) -> Result<()> {
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

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    let is_ws_upgrade = headers
        .get("upgrade")
        .map(|v| v.to_lowercase().contains("websocket"))
        .unwrap_or(false);

    tracing::debug!(path = %path, is_ws_upgrade, "dashboard: routing");

    // Reclaim the raw TcpStream (the BufReader may have buffered data after headers)
    let raw = reader.into_inner();

    if is_ws_upgrade && path == "/ws" {
        handle_ws_upgrade(raw, &headers, ws_tx).await
    } else {
        handle_http(raw, &path, store).await
    }
}

async fn handle_http(mut stream: TcpStream, path: &str, store: Store) -> Result<()> {
    tracing::info!(path = %path, "dashboard: HTTP request");

    // Vault endpoint — must be checked before the generic /api/conversations/:id branch
    if path.starts_with("/api/conversations/") && path.ends_with("/vault") {
        let conv_id = path
            .strip_prefix("/api/conversations/").unwrap_or("")
            .strip_suffix("/vault").unwrap_or("");
        handle_vault_api(&mut stream, &store, conv_id).await?;
        return Ok(());
    }

    let (status, content_type, body) = match path {
        "/" | "/index.html" => serve_asset("index.html"),
        "/style.css" => serve_asset("style.css"),
        "/app.js" => serve_asset("app.js"),
        "/api/conversations" => {
            let convs = store.list_conversations().unwrap_or_default();
            let json = serde_json::to_vec(&convs).unwrap_or_default();
            (200u16, "application/json", json)
        }
        p if p.starts_with("/api/conversations/") => {
            let id = p.trim_start_matches("/api/conversations/");
            let convs = store.list_conversations().unwrap_or_default();
            let conv = convs.into_iter().find(|c| c.id == id);
            let msgs = store.get_messages(id).unwrap_or_default();
            let json = serde_json::to_vec(&serde_json::json!({
                "conversation": conv,
                "messages": msgs,
            }))?;
            (200, "application/json", json)
        }
        _ => (404, "text/plain", b"Not Found".to_vec()),
    };

    tracing::info!(path = %path, status, body_bytes = body.len(), "dashboard: HTTP response");
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(&body).await?;
    Ok(())
}

async fn handle_vault_api(stream: &mut TcpStream, store: &Store, conv_id: &str) -> Result<()> {
    // store.load_vault returns Option<(u64, Vec<StoredVaultRecord>)>
    let vault_data = match store.load_vault(conv_id) {
        Ok(Some((_seed, records))) => {
            records.iter().map(|r| serde_json::json!({
                "type": r.pii_type,
                "original_masked": format!("[{}]", r.pii_type.to_uppercase()),
                "synthetic": r.synthetic,
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
