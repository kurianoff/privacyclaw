pub mod connect;
pub mod intercept;
pub mod network;
pub mod passthrough;

use crate::ca::cert_gen::CertCache;
use crate::config::Config;
use crate::dashboard::WsEvent;
use crate::storage::Store;
use anyhow::Result;
use rustls::{ClientConfig, RootCertStore};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;

pub async fn run(
    config: Arc<Config>,
    cert_cache: CertCache,
    store: Store,
    ws_tx: broadcast::Sender<WsEvent>,
) -> Result<()> {
    let addr = &config.proxy.listen;
    let listener = TcpListener::bind(addr).await?;
    tracing::warn!(addr = %addr, "CONNECT proxy bound");

    // Build upstream TLS client config once — shared across all connections via Arc.
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_tls_cfg = Arc::new(
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::info!(peer_addr = %peer_addr, "CONNECT: accepted connection");

        let config = config.clone();
        let cert_cache = cert_cache.clone();
        let store = store.clone();
        let ws_tx = ws_tx.clone();
        let client_tls_cfg = client_tls_cfg.clone();

        tokio::spawn(async move {
            tracing::debug!(peer_addr = %peer_addr, "CONNECT: connection task started");
            if let Err(e) = connect::handle(stream, config, cert_cache, store, ws_tx, client_tls_cfg).await {
                tracing::warn!(peer_addr = %peer_addr, err = %e, "CONNECT: connection error");
            }
            tracing::debug!(peer_addr = %peer_addr, "CONNECT: connection task finished");
        });
    }
}
