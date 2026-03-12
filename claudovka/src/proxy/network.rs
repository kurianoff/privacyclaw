use crate::ca::cert_gen::{CertCache, SniCertResolver};
use crate::config::Config;
use crate::dashboard::WsEvent;
use crate::storage::Store;
use anyhow::{Context, Result};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::broadcast;
use tokio_rustls::{TlsAcceptor, TlsConnector};

/// Run the network-level transparent proxy.
///
/// Clients connect directly on TLS (no HTTP CONNECT). The proxy peeks at the
/// ClientHello to read the SNI before committing to a TLS handshake, then either:
///   - MITM (intercept): full TLS accept + proxy — for LLM API hosts
///   - Passthrough (raw TCP bridge): forward the raw TLS stream to the real upstream
///
/// Typical setup:
///   /etc/hosts: 127.0.0.1  api.anthropic.com
///   pf rule:    rdr port 443 -> 127.0.0.1 port 4443
pub async fn run(
    config: Arc<Config>,
    cert_cache: CertCache,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    pii: crate::pii::PiiCtx,
) -> Result<()> {
    let addr = &config.network_proxy.listen;

    // Single ServerConfig shared across all connections; cert resolution is per-SNI.
    // Advertise http/1.1 only — forces the client to downgrade from h2 so our
    // HTTP/1.1 intercept can parse and forward requests correctly.
    // (h2 transparent bridging causes CLOSE_WAIT deadlocks when Anthropic sends GOAWAY
    // but the h2 client keeps the connection open expecting stream reuse.)
    let resolver = Arc::new(SniCertResolver { cert_cache });
    let mut server_cfg = ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

    let listener = TcpListener::bind(addr).await
        .with_context(|| format!("Network proxy failed to bind on {}", addr))?;
    tracing::warn!(addr = %addr, "network proxy bound");

    // Build upstream TLS client config once — shared across all connections via Arc.
    // Use http/1.1 only upstream to match the forced client downgrade above.
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut upstream_client_cfg = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    upstream_client_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let client_cfg = Arc::new(upstream_client_cfg);

    loop {
        let (stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                // Transient errors (EMFILE, ENFILE, ECONNABORTED) must not kill
                // the accept loop — the proxy would stop handling all new connections.
                tracing::warn!("Network proxy: accept() error: {}", e);
                // Brief pause to avoid spinning on persistent resource exhaustion.
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        tracing::warn!(peer_addr = %peer_addr, "network: accepted connection");

        let config = config.clone();
        let acceptor = acceptor.clone();
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let pii = pii.clone();
        let client_cfg = client_cfg.clone();

        tokio::spawn(async move {
            tracing::debug!(peer_addr = %peer_addr, "network: connection task started");
            if let Err(e) = handle(stream, config, acceptor, store, ws_tx, pii, client_cfg).await {
                tracing::warn!(peer_addr = %peer_addr, err = %e, "network: connection handler error");
            }
            tracing::debug!(peer_addr = %peer_addr, "network: connection task finished");
        });
    }
}

async fn handle(
    stream: TcpStream,
    config: Arc<Config>,
    acceptor: TlsAcceptor,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
    pii: crate::pii::PiiCtx,
    client_cfg: Arc<ClientConfig>,
) -> Result<()> {
    // Peek at the ClientHello to get SNI BEFORE committing to a TLS handshake.
    // peek() leaves bytes in the socket buffer so subsequent reads see them again.
    let mut peek_buf = [0u8; 4096];
    let n = stream.peek(&mut peek_buf).await?;
    if n == 0 {
        return Ok(());
    }
    tracing::debug!(
        peek_hex = %crate::util::fmt_chunk_hex(&peek_buf[..n], 64),
        peek_bytes = n,
        "network: peek buf"
    );

    let host = match peek_sni(&peek_buf[..n]) {
        Some(h) => h,
        None => {
            tracing::debug!(peek_bytes = n, "network: no SNI in ClientHello, dropping");
            return Ok(());
        }
    };
    tracing::info!(host = %host, "network: SNI extracted");

    if !config.is_intercepted(&host) {
        tracing::info!(host = %host, intercept = false, "network: intercept decision");
        // Not an LLM host — bridge raw TCP to the real upstream without MITM.
        // ClientHello bytes are still in the socket buffer (peek didn't consume them).
        return passthrough_raw(stream, &host).await;
    }
    tracing::info!(host = %host, intercept = true, "network: intercept decision");

    tracing::warn!(host = %host, "network: intercepting host");

    // TLS accept — SniCertResolver generates a leaf cert for `host`.
    // Works because peek() left the ClientHello bytes unconsumed.
    // The server advertises http/1.1 only so the client downgrades from h2;
    // we log the negotiated protocol for diagnostics.
    tracing::debug!(host = %host, "network: starting client TLS handshake");
    let client_tls = acceptor.accept(stream).await
        .context("Network mode: client TLS handshake failed")?;
    let negotiated = client_tls.get_ref().1.alpn_protocol().map(|p| p.to_vec());
    tracing::info!(host = %host, alpn = ?negotiated.as_deref().and_then(|p| std::str::from_utf8(p).ok()), "network: client TLS handshake done");

    // Resolve the real upstream IP, bypassing /etc/hosts (which points to us).
    tracing::debug!(host = %host, "network: resolving DNS (bypass /etc/hosts)");
    let ip = resolve_bypass_hosts(&host).await
        .with_context(|| format!("DNS resolution failed for {}", host))?;
    tracing::info!(host = %host, ip = %ip, "network: DNS resolved");

    // Connect to the real upstream on port 443 using the resolved IP.
    tracing::debug!(host = %host, ip = %ip, port = 443, "network: connecting to upstream TCP");
    let upstream_tcp = TcpStream::connect(SocketAddr::new(ip, 443)).await
        .with_context(|| format!("Failed to connect to upstream {} ({})", host, ip))?;
    tracing::info!(host = %host, ip = %ip, "network: upstream TCP connected");

    // Reuse the shared TlsConnector — just clones the Arc<ClientConfig>.
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from(host.clone())
        .context("Invalid server name")?;
    tracing::debug!(host = %host, "network: starting upstream TLS handshake");
    let upstream_tls = connector.connect(server_name, upstream_tcp).await
        .with_context(|| format!("Upstream TLS handshake failed for {}", host))?;
    tracing::info!(host = %host, "network: upstream TLS handshake done");

    let (client_reader, client_writer) = tokio::io::split(client_tls);
    let (upstream_reader, upstream_writer) = tokio::io::split(upstream_tls);

    crate::proxy::intercept::run(
        client_reader, client_writer,
        upstream_reader, upstream_writer,
        host, store, ws_tx, pii,
    ).await
}

/// Forward a raw TLS stream to the real upstream without MITM.
///
/// Used for non-intercepted hosts that arrive via the pf redirect. The ClientHello
/// bytes are still in `stream`'s socket buffer (peek() didn't consume them), so the
/// upstream sees a normal TLS handshake with the client's real certificate expectations.
///
/// If DNS resolves to a loopback address (local service) or fails, the connection is
/// closed cleanly rather than causing a redirect loop.
async fn passthrough_raw(stream: TcpStream, host: &str) -> Result<()> {
    tracing::debug!(host = %host, "network: passthrough_raw: resolving DNS");
    let ip = match resolve_bypass_hosts(host).await {
        Ok(ip) if !ip.is_loopback() => ip,
        Ok(ip) => {
            tracing::debug!(host = %host, ip = %ip, "network: passthrough_raw: loopback drop");
            return Ok(());
        }
        Err(e) => {
            tracing::debug!(host = %host, err = %e, "network: passthrough_raw: DNS failed, dropping");
            return Ok(());
        }
    };

    tracing::warn!(host = %host, ip = %ip, "network: passthrough established");
    let mut upstream = TcpStream::connect(SocketAddr::new(ip, 443)).await
        .with_context(|| format!("Passthrough: failed to connect to {} ({})", host, ip))?;

    let mut stream = stream;
    tokio::io::copy_bidirectional(&mut stream, &mut upstream).await?;
    tracing::warn!(host = %host, ip = %ip, "network: passthrough closed");
    Ok(())
}

/// Extract the SNI hostname from raw TLS ClientHello bytes without consuming them.
///
/// Returns None if the buffer doesn't contain a valid ClientHello or has no SNI extension.
fn peek_sni(buf: &[u8]) -> Option<String> {
    // TLS record header: content_type(1) + version(2) + length(2)
    if buf.len() < 5 || buf[0] != 0x16 {
        return None; // Not a TLS handshake record
    }

    // Handshake message: type(1) + length(3)
    let hs = buf.get(5..)?;
    if hs.first()? != &0x01 {
        return None; // Not a ClientHello
    }
    let hs_len = (((*hs.get(1)?) as usize) << 16)
        | (((*hs.get(2)?) as usize) << 8)
        | ((*hs.get(3)?) as usize);
    let hello = hs.get(4..4 + hs_len)?;

    // Skip: legacy_version(2) + random(32) = 34 bytes
    let mut pos = 34usize;

    // Skip session_id: length(1) + data
    let sid_len = *hello.get(pos)? as usize;
    pos = pos.checked_add(1 + sid_len)?;

    // Skip cipher_suites: length(2) + data
    let cs_len = u16::from_be_bytes([*hello.get(pos)?, *hello.get(pos + 1)?]) as usize;
    pos = pos.checked_add(2 + cs_len)?;

    // Skip compression_methods: length(1) + data
    let cm_len = *hello.get(pos)? as usize;
    pos = pos.checked_add(1 + cm_len)?;

    // Extensions: total_length(2) + [type(2) + length(2) + data] ...
    let ext_total = u16::from_be_bytes([*hello.get(pos)?, *hello.get(pos + 1)?]) as usize;
    pos = pos.checked_add(2)?;
    let ext_end = pos.checked_add(ext_total)?;

    while pos + 4 <= ext_end {
        let ext_type = u16::from_be_bytes([*hello.get(pos)?, *hello.get(pos + 1)?]);
        let ext_len = u16::from_be_bytes([*hello.get(pos + 2)?, *hello.get(pos + 3)?]) as usize;
        pos += 4;

        if ext_type == 0x0000 {
            // SNI extension: list_len(2) + name_type(1) + name_len(2) + name
            let ext_data = hello.get(pos..pos + ext_len)?;
            if ext_data.len() < 5 || ext_data[2] != 0x00 {
                return None;
            }
            let name_len = u16::from_be_bytes([ext_data[3], ext_data[4]]) as usize;
            let name_bytes = ext_data.get(5..5 + name_len)?;
            return std::str::from_utf8(name_bytes).ok().map(|s| s.to_string());
        }

        pos = pos.checked_add(ext_len)?;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    // ─── SNI extraction tests (6.1–6.3) ──────────────────────────────────────

    /// Build a minimal TLS ClientHello containing an SNI extension for `hostname`.
    fn make_client_hello(hostname: &str) -> Vec<u8> {
        let name_bytes = hostname.as_bytes();
        let name_len = name_bytes.len();
        // SNI extension data: list_len(2) + name_type(1) + name_len(2) + name
        let sni_data_len = 2 + 1 + 2 + name_len;
        let ext_len = sni_data_len;
        let ext_block_len = 4 + ext_len; // type(2) + len(2) + data
        // ClientHello body length
        let hello_len = 2 + 32 + 1 + 2 + 2 + 1 + 1 + 2 + ext_block_len;
        // Handshake length = 1(type) + 3(len) + hello_body — but peek_sni reads hs.get(4..4+hs_len)
        // where hs = buf[5..], so hs_len is the hello_body length only.
        let hs_len = hello_len;
        let record_body_len = (1 + 3 + hs_len) as u16; // handshake type + 3-byte len + body

        let mut buf: Vec<u8> = Vec::new();
        // TLS record header
        buf.push(0x16); // content_type: handshake
        buf.extend_from_slice(&[0x03, 0x01]); // version TLS 1.0
        buf.extend_from_slice(&record_body_len.to_be_bytes());
        // Handshake header
        buf.push(0x01); // ClientHello
        // 3-byte big-endian length of the ClientHello body
        buf.push(0);
        buf.extend_from_slice(&(hello_len as u16).to_be_bytes());
        // legacy_version
        buf.extend_from_slice(&[0x03, 0x03]);
        // random (32 bytes)
        buf.extend_from_slice(&[0u8; 32]);
        // session_id_len = 0
        buf.push(0);
        // cipher_suites: length(2) + 1 suite(2)
        buf.extend_from_slice(&[0x00, 0x02, 0x00, 0x2F]);
        // compression_methods: length(1) + null(1)
        buf.extend_from_slice(&[0x01, 0x00]);
        // extensions total length
        buf.extend_from_slice(&(ext_block_len as u16).to_be_bytes());
        // SNI extension
        buf.extend_from_slice(&[0x00, 0x00]); // ext_type = 0 (SNI)
        buf.extend_from_slice(&(ext_len as u16).to_be_bytes());
        // SNI list: list_len(2) + name_type(1) + name_len(2) + name
        buf.extend_from_slice(&((1 + 2 + name_len) as u16).to_be_bytes());
        buf.push(0x00); // name_type: host_name
        buf.extend_from_slice(&(name_len as u16).to_be_bytes());
        buf.extend_from_slice(name_bytes);
        buf
    }

    #[test]
    fn test_peek_sni_extracts_hostname() {
        let buf = make_client_hello("api.anthropic.com");
        let result = peek_sni(&buf);
        assert_eq!(result, Some("api.anthropic.com".to_string()),
            "expected SNI = api.anthropic.com, got {result:?}");
    }

    #[test]
    fn test_peek_sni_returns_none_for_garbage() {
        // First byte != 0x16 → not a TLS handshake record.
        assert_eq!(peek_sni(&[0x00, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn test_peek_sni_returns_none_for_truncated_buffer() {
        // Only 3 bytes — too short even for the record header (needs 5).
        assert_eq!(peek_sni(&[0x16, 0x03, 0x01]), None);
    }

    // ─── Intercept decision tests (6.4–6.5) ──────────────────────────────────

    #[test]
    fn test_intercept_decision_known_hosts() {
        let cfg = Config::default();
        assert!(cfg.is_intercepted("api.anthropic.com"),
            "api.anthropic.com should be intercepted by default");
        assert!(cfg.is_intercepted("api.openai.com"),
            "api.openai.com should be intercepted by default");
    }

    #[test]
    fn test_intercept_decision_unknown_host() {
        let cfg = Config::default();
        assert!(!cfg.is_intercepted("example.com"),
            "example.com must NOT be intercepted");
        assert!(!cfg.is_intercepted("google.com"),
            "google.com must NOT be intercepted");
    }

    // ─── DNS packet format test (6.6) ────────────────────────────────────────

    #[test]
    fn test_dns_query_packet_format() {
        let pkt = build_dns_a_query("example.com");
        // Transaction ID
        assert_eq!(&pkt[0..2], &[0xAB, 0xCD], "wrong transaction ID");
        // QDCOUNT = 1
        assert_eq!(&pkt[4..6], &[0x00, 0x01], "QDCOUNT must be 1");
        // Label-encoded QNAME: [7]"example"[3]"com"[0]
        let qname: Vec<u8> = [7u8].iter()
            .chain(b"example")
            .chain(&[3u8])
            .chain(b"com")
            .chain(&[0u8])
            .copied()
            .collect();
        let found = pkt.windows(qname.len()).any(|w| w == qname.as_slice());
        assert!(found, "label-encoded QNAME not found in DNS packet");
        // Ends with QTYPE=A(1), QCLASS=IN(1)
        assert_eq!(pkt.last_chunk::<4>(), Some(&[0x00, 0x01, 0x00, 0x01]),
            "packet must end with QTYPE=A QCLASS=IN");
    }
}

// ─── DNS resolver that bypasses /etc/hosts ───────────────────────────────────

/// Query 8.8.8.8:53 directly for an A record, bypassing the system resolver
/// (and thus /etc/hosts). Falls back to 1.1.1.1:53 on timeout.
async fn resolve_bypass_hosts(hostname: &str) -> Result<IpAddr> {
    let query = build_dns_a_query(hostname);

    for dns in ["8.8.8.8:53", "1.1.1.1:53"] {
        tracing::debug!(hostname = %hostname, dns_server = %dns, "network: DNS query");
        let sock = UdpSocket::bind("0.0.0.0:0").await?;
        sock.send_to(&query, dns).await?;

        let mut buf = [0u8; 512];
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            sock.recv(&mut buf),
        ).await;

        match result {
            Ok(Ok(n)) => {
                if let Some(ip) = parse_first_a_record(&buf[..n]) {
                    tracing::debug!(hostname = %hostname, ip = %ip, dns_server = %dns, "network: DNS A record found");
                    return Ok(IpAddr::V4(ip));
                }
            }
            _ => continue,
        }
    }

    anyhow::bail!("No A record found for {} from 8.8.8.8 or 1.1.1.1", hostname)
}

/// Build a minimal DNS query packet for an A record.
fn build_dns_a_query(hostname: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&[0xAB, 0xCD]); // transaction ID (arbitrary)
    buf.extend_from_slice(&[0x01, 0x00]); // flags: RD=1 (recursion desired)
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT: 1 question
    buf.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // ANCOUNT/NSCOUNT/ARCOUNT: 0
    // QNAME: label-encoded hostname
    for label in hostname.split('.') {
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);                           // root label
    buf.extend_from_slice(&[0x00, 0x01]); // QTYPE: A
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS: IN
    buf
}

/// Parse the first A record from a DNS response.
/// Returns None if no A record is found or the response is malformed.
fn parse_first_a_record(response: &[u8]) -> Option<Ipv4Addr> {
    if response.len() < 12 {
        return None;
    }
    let ancount = u16::from_be_bytes([response[6], response[7]]) as usize;
    if ancount == 0 {
        return None;
    }

    // Skip question section starting at byte 12.
    let mut pos = 12;
    // Skip QNAME labels
    loop {
        if pos >= response.len() { return None; }
        let len = response[pos] as usize;
        if len == 0 { pos += 1; break; }
        if len & 0xC0 == 0xC0 { pos += 2; break; } // DNS pointer (unlikely in question)
        pos += 1 + len;
    }
    pos += 4; // skip QTYPE(2) + QCLASS(2)

    // Parse answer records.
    for _ in 0..ancount {
        if pos >= response.len() { return None; }
        // Skip NAME: either a pointer (2 bytes) or a sequence of labels
        if response[pos] & 0xC0 == 0xC0 {
            pos += 2;
        } else {
            loop {
                if pos >= response.len() { return None; }
                let len = response[pos] as usize;
                pos += 1;
                if len == 0 { break; }
                pos += len;
            }
        }
        if pos + 10 > response.len() { return None; }
        let rtype = u16::from_be_bytes([response[pos], response[pos + 1]]);
        pos += 8; // skip TYPE(2) CLASS(2) TTL(4)
        let rdlen = u16::from_be_bytes([response[pos], response[pos + 1]]) as usize;
        pos += 2;
        if rtype == 1 && rdlen == 4 && pos + 4 <= response.len() {
            // A record: 4-byte IPv4 address
            return Some(Ipv4Addr::new(
                response[pos],
                response[pos + 1],
                response[pos + 2],
                response[pos + 3],
            ));
        }
        pos += rdlen;
    }
    None
}
