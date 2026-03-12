use anyhow::{Context, Result};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyUsagePurpose, SanType};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_rustls::TlsAcceptor;

use super::CaBundle;

// ─── CertCache ───────────────────────────────────────────────────────────────

/// Thread-safe cache of per-domain TLS acceptors and certified keys.
#[derive(Clone)]
pub struct CertCache {
    /// Cache for CONNECT-mode: full ServerConfig per domain.
    acceptor_cache: Arc<Mutex<HashMap<String, Arc<ServerConfig>>>>,
    /// Cache for network-mode: CertifiedKey per domain.
    key_cache: Arc<Mutex<HashMap<String, Arc<CertifiedKey>>>>,
    ca_bundle: Arc<CaBundle>,
}

impl CertCache {
    pub fn new(ca_bundle: CaBundle) -> Self {
        Self {
            acceptor_cache: Arc::new(Mutex::new(HashMap::new())),
            key_cache: Arc::new(Mutex::new(HashMap::new())),
            ca_bundle: Arc::new(ca_bundle),
        }
    }

    /// Get or create a `TlsAcceptor` for the given domain (CONNECT mode).
    pub fn get_or_create(&self, domain: &str) -> Result<TlsAcceptor> {
        let mut cache = self.acceptor_cache.lock().unwrap();
        tracing::debug!(domain = %domain, cache_size = cache.len(), "cert_gen: cache size");
        if let Some(cfg) = cache.get(domain) {
            tracing::info!(domain = %domain, "cert_gen: cache hit");
            return Ok(TlsAcceptor::from(cfg.clone()));
        }
        tracing::info!(domain = %domain, "cert_gen: cache miss, generating cert");
        let ck = self.get_or_create_key_inner(domain)?;
        let mut server_cfg = ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(SingleKeyResolver(ck)));
        server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let arc_cfg = Arc::new(server_cfg);
        cache.insert(domain.to_string(), arc_cfg.clone());
        tracing::info!(domain = %domain, "cert_gen: cert generated");
        Ok(TlsAcceptor::from(arc_cfg))
    }

    /// Get or create a `CertifiedKey` for the given domain (network mode).
    pub fn get_or_create_key(&self, domain: &str) -> Result<Arc<CertifiedKey>> {
        self.get_or_create_key_inner(domain)
    }

    fn get_or_create_key_inner(&self, domain: &str) -> Result<Arc<CertifiedKey>> {
        let mut cache = self.key_cache.lock().unwrap();
        tracing::debug!(domain = %domain, cache_size = cache.len(), "cert_gen: key cache size");
        if let Some(ck) = cache.get(domain) {
            tracing::info!(domain = %domain, "cert_gen: key cache hit");
            return Ok(ck.clone());
        }
        tracing::info!(domain = %domain, "cert_gen: key cache miss, generating cert");
        let ck = Arc::new(build_certified_key(domain, &self.ca_bundle)?);
        cache.insert(domain.to_string(), ck.clone());
        tracing::info!(domain = %domain, "cert_gen: cert generated for domain");
        Ok(ck)
    }
}

// ─── SingleKeyResolver ───────────────────────────────────────────────────────

/// Simple resolver that returns the same CertifiedKey regardless of SNI.
/// Used in CONNECT-mode where we build one ServerConfig per domain.
struct SingleKeyResolver(Arc<CertifiedKey>);

impl std::fmt::Debug for SingleKeyResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SingleKeyResolver")
    }
}

impl ResolvesServerCert for SingleKeyResolver {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

// ─── SniCertResolver ─────────────────────────────────────────────────────────

/// Implements `ResolvesServerCert` for network-mode TLS: looks up the SNI
/// from the ClientHello and returns the matching certified key.
pub struct SniCertResolver {
    pub cert_cache: CertCache,
}

impl std::fmt::Debug for SniCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SniCertResolver")
    }
}

impl ResolvesServerCert for SniCertResolver {
    fn resolve(&self, ch: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let domain = ch.server_name()?;
        self.cert_cache.get_or_create_key(domain).ok()
    }
}

// ─── Certificate generation ───────────────────────────────────────────────────

/// Build a leaf `CertifiedKey` for `domain`, signed by the given CA bundle.
fn build_certified_key(domain: &str, ca: &CaBundle) -> Result<CertifiedKey> {
    tracing::debug!(domain = %domain, "cert_gen: building CertifiedKey");
    // Parse CA key
    let ca_key_pair = rcgen::KeyPair::from_pem(&ca.key_pem)
        .context("Failed to parse CA key")?;

    // Reconstruct CA Certificate for signing (same DN as generate_ca in ca/mod.rs).
    let mut ca_params = CertificateParams::new(vec![]).context("Failed to create CA params")?;
    ca_params.distinguished_name.push(DnType::OrganizationName, "Claudovka Privacy Proxy");
    ca_params.distinguished_name.push(DnType::CommonName, "Claudovka Root CA");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let ca_cert = ca_params
        .self_signed(&ca_key_pair)
        .context("Failed to reconstruct CA certificate for signing")?;

    // Generate leaf cert for the domain
    let mut leaf_params = CertificateParams::new(vec![domain.to_string()])
        .context("Failed to create leaf cert params")?;
    leaf_params.distinguished_name.push(DnType::CommonName, domain);
    leaf_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    leaf_params.subject_alt_names = vec![SanType::DnsName(
        domain.to_string().try_into().context("Invalid domain name")?,
    )];

    let leaf_key = rcgen::KeyPair::generate()?;
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key_pair)
        .context("Failed to sign leaf certificate")?;

    let leaf_der = leaf_cert.der().to_vec();
    tracing::debug!(domain = %domain, der_bytes = leaf_der.len(), "cert_gen: cert DER size");
    let cert_chain: Vec<CertificateDer<'static>> = vec![
        CertificateDer::from(leaf_der),
        CertificateDer::from(ca.cert_der.clone()),
    ];

    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .context("Failed to create signing key from leaf key")?;

    Ok(CertifiedKey::new(cert_chain, signing_key))
}
