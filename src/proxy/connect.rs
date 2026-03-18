use crate::ca::cert_gen::CertCache;
use crate::config::Config;
use crate::dashboard::WsEvent;
use crate::storage::Store;
use anyhow::{Context, Result};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;

pub async fn handle(
    stream: TcpStream,
    config: Arc<Config>,
    cert_cache: CertCache,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    pii: crate::pii::PiiCtx,
    client_tls_cfg: Arc<ClientConfig>,
) -> Result<()> {
    tracing::debug!("connect: reading CONNECT request line");
    let mut buf_reader = BufReader::new(stream);

    // Read the CONNECT request line — 30 s timeout guards against clients that
    // open a TCP socket but never send a CONNECT line (e.g. stale pre-sleep sockets).
    let mut connect_line = String::new();
    match timeout(Duration::from_secs(30), buf_reader.read_line(&mut connect_line)).await {
        Ok(result) => { result?; }
        Err(_) => {
            tracing::warn!("connect: request-line read timeout, dropping connection");
            return Ok(());
        }
    }
    let connect_line = connect_line.trim();
    tracing::debug!(line = %connect_line, "connect: CONNECT request line received");

    // Drain remaining headers — same 30 s timeout per line.
    loop {
        let mut line = String::new();
        match timeout(Duration::from_secs(30), buf_reader.read_line(&mut line)).await {
            Ok(result) => { result?; }
            Err(_) => {
                tracing::warn!("connect: header drain read timeout, dropping connection");
                return Ok(());
            }
        }
        tracing::debug!(line = %line.trim(), "connect: drained header line");
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
    }

    // Parse "CONNECT host:port HTTP/1.1"
    let (host, port) = parse_connect(connect_line)
        .with_context(|| format!("Failed to parse CONNECT: {:?}", connect_line))?;
    tracing::info!(host = %host, port = port, "connect: CONNECT parsed");

    // Get the underlying TcpStream back
    let inner_stream = buf_reader.into_inner();

    if config.is_intercepted(&host) {
        tracing::info!(host = %host, intercept = true, "connect: intercept decision");
        mitm(inner_stream, &host, port, cert_cache, store, ws_tx, pii.clone(), client_tls_cfg).await
    } else {
        tracing::info!(host = %host, intercept = false, "connect: intercept decision");
        passthrough(inner_stream, &host, port).await
    }
}

async fn passthrough(mut stream: TcpStream, host: &str, port: u16) -> Result<()> {
    let addr = format!("{}:{}", host, port);
    tracing::debug!(addr = %addr, "connect: passthrough: connecting to upstream");
    // 3.1 — 10 s timeout on upstream TCP connect.
    let mut upstream = timeout(Duration::from_secs(10), TcpStream::connect(&addr))
        .await
        .with_context(|| format!("TCP connect timeout to {}", addr))?
        .with_context(|| format!("Failed to connect to {}", addr))?;
    tracing::debug!(addr = %addr, "connect: passthrough: upstream connected");

    tracing::debug!("connect: sending 200 Connection established to client (passthrough)");
    stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await?;

    // 3.2 — 300 s idle timeout on bidirectional copy; idle close is normal, so return Ok.
    match timeout(
        Duration::from_secs(300),
        tokio::io::copy_bidirectional(&mut stream, &mut upstream),
    )
    .await
    {
        Ok(result) => { result?; }
        Err(_) => {
            tracing::warn!(host = %host, port = port, "connect: passthrough idle timeout");
        }
    }
    tracing::warn!(host = %host, port = port, "connect: passthrough established and closed");
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn mitm(
    mut client_stream: TcpStream,
    host: &str,
    port: u16,
    cert_cache: CertCache,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    pii: crate::pii::PiiCtx,
    client_tls_cfg: Arc<ClientConfig>,
) -> Result<()> {
    tracing::warn!(host = %host, port = port, "connect: MITM session starting");

    // Resolve the real upstream IP, bypassing /etc/hosts (which may point to us
    // when the network proxy is enabled), then connect using the IP directly.
    tracing::debug!(host = %host, "connect: resolving upstream DNS (bypass /etc/hosts)");
    let ip = super::network::resolve_bypass_hosts(host).await
        .with_context(|| format!("DNS resolution failed for {}", host))?;
    let addr = std::net::SocketAddr::new(ip, port);
    tracing::debug!(addr = %addr, "connect: connecting to upstream TCP");
    // 3.3 — 10 s timeout on upstream TCP connect.
    let upstream_tcp = timeout(Duration::from_secs(10), TcpStream::connect(addr))
        .await
        .with_context(|| format!("TCP connect timeout to {}", addr))?
        .with_context(|| format!("Failed to connect upstream: {}", addr))?;
    tracing::info!(addr = %addr, "connect: upstream TCP connected");

    // Reuse the shared TlsConnector — just clones the Arc<ClientConfig>.
    let connector = TlsConnector::from(client_tls_cfg);
    let server_name = ServerName::try_from(host.to_string())
        .context("Invalid server name")?;
    tracing::debug!(host = %host, "connect: starting upstream TLS handshake");
    // 3.4 — 10 s timeout on upstream TLS handshake.
    let upstream_tls = timeout(Duration::from_secs(10), connector.connect(server_name, upstream_tcp))
        .await
        .with_context(|| format!("TLS handshake timeout for {}", host))?
        .context("Upstream TLS handshake failed")?;
    tracing::info!(host = %host, "connect: upstream TLS handshake done");

    // Send 200 to client
    tracing::debug!("connect: sending 200 Connection established to client (MITM)");
    client_stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await?;

    // Build client-side TLS acceptor (presents leaf cert signed by our CA)
    tracing::debug!(host = %host, "connect: starting client TLS handshake");
    let acceptor = cert_cache.get_or_create(host)
        .with_context(|| format!("Failed to get cert for {}", host))?;
    let client_tls = acceptor.accept(client_stream).await
        .context("Client TLS handshake failed")?;
    tracing::info!(host = %host, "connect: client TLS handshake done");

    tracing::warn!(host = %host, port = port, "connect: MITM session started");

    let (client_reader, client_writer) = tokio::io::split(client_tls);
    let (upstream_reader, upstream_writer) = tokio::io::split(upstream_tls);

    let result = crate::proxy::intercept::run(
        client_reader, client_writer,
        upstream_reader, upstream_writer,
        host.to_string(), store, ws_tx, pii,
    ).await;

    tracing::warn!(host = %host, port = port, "connect: MITM session ended");
    result
}

fn parse_connect(line: &str) -> Option<(String, u16)> {
    // "CONNECT host:port HTTP/1.1"
    let mut parts = line.splitn(3, ' ');
    let method = parts.next()?;
    if method != "CONNECT" {
        return None;
    }
    let hostport = parts.next()?;
    let (host, port_str) = hostport.rsplit_once(':')?;
    let port: u16 = port_str.parse().ok()?;
    Some((host.to_string(), port))
}
