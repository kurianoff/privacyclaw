pub mod cert_gen;

use anyhow::{Context, Result};
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyUsagePurpose};
use std::path::{Path, PathBuf};

pub struct CaBundle {
    #[allow(dead_code)]
    pub cert_pem: String,
    pub key_pem: String,
    pub cert_der: Vec<u8>,
}

/// Load existing CA or return None.
pub fn load_ca(ca_dir: &Path) -> Result<Option<CaBundle>> {
    let cert_path = ca_dir.join("ca.pem");
    let key_path = ca_dir.join("ca.key.pem");
    tracing::debug!(cert_path = %cert_path.display(), key_path = %key_path.display(), "ca: checking paths");

    if !cert_path.exists() || !key_path.exists() {
        tracing::info!(cert_path = %cert_path.display(), "ca: CA not found, returning None");
        return Ok(None);
    }

    let cert_pem = std::fs::read_to_string(&cert_path)
        .with_context(|| format!("Failed to read CA cert: {:?}", cert_path))?;
    let key_pem = std::fs::read_to_string(&key_path)
        .with_context(|| format!("Failed to read CA key: {:?}", key_path))?;

    let cert_der = pem_cert_to_der(&cert_pem)?;
    tracing::debug!(der_bytes = cert_der.len(), "ca: cert DER bytes");
    tracing::info!(cert_path = %cert_path.display(), "ca: CA loaded from disk");
    Ok(Some(CaBundle { cert_pem, key_pem, cert_der }))
}

/// Generate a new ECDSA P-256 CA and save to disk.
pub fn generate_ca(ca_dir: &Path) -> Result<CaBundle> {
    std::fs::create_dir_all(ca_dir)
        .with_context(|| format!("Failed to create CA dir: {:?}", ca_dir))?;

    let mut params = CertificateParams::new(vec![]).context("Failed to create cert params")?;
    params.distinguished_name.push(DnType::OrganizationName, "Claudovka Privacy Proxy");
    params.distinguished_name.push(DnType::CommonName, "Claudovka Root CA");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];

    tracing::debug!("ca: generating key pair");
    let key_pair = rcgen::KeyPair::generate()?;
    let cert = params.self_signed(&key_pair)?;

    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let cert_der = cert.der().to_vec();

    let cert_path = ca_dir.join("ca.pem");
    let key_path = ca_dir.join("ca.key.pem");

    tracing::debug!(cert_path = %cert_path.display(), key_path = %key_path.display(), "ca: writing cert and key");
    std::fs::write(&cert_path, &cert_pem)
        .with_context(|| format!("Failed to write CA cert: {:?}", cert_path))?;
    std::fs::write(&key_path, &key_pem)
        .with_context(|| format!("Failed to write CA key: {:?}", key_path))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?;
    }

    tracing::warn!(cert_path = %cert_path.display(), "ca: CA generated");
    Ok(CaBundle { cert_pem, key_pem, cert_der })
}

/// Remove CA files.
pub fn delete_ca(ca_dir: &Path) -> Result<()> {
    for name in ["ca.pem", "ca.key.pem"] {
        let p = ca_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p)
                .with_context(|| format!("Failed to remove {:?}", p))?;
        }
    }
    tracing::info!(ca_dir = %ca_dir.display(), "ca: CA deleted");
    Ok(())
}

pub fn ca_cert_path(ca_dir: &Path) -> PathBuf {
    ca_dir.join("ca.pem")
}

pub fn install_ca_trust(ca_dir: &Path) -> Result<()> {
    let cert_path = ca_cert_path(ca_dir);
    let cert_str = cert_path.to_str().unwrap_or("ca.pem");

    #[cfg(target_os = "macos")]
    {
        let status = std::process::Command::new("security")
            .args(["add-trusted-cert", "-d", "-r", "trustRoot",
                   "-k", "/Library/Keychains/System.keychain", cert_str])
            .status()?;
        if !status.success() {
            print_manual_instructions(&cert_path);
            anyhow::bail!("security add-trusted-cert failed — see instructions above");
        }
    }

    #[cfg(target_os = "linux")]
    {
        let dest = std::path::PathBuf::from("/usr/local/share/ca-certificates/claudovka-ca.crt");
        std::fs::copy(&cert_path, &dest)
            .context("Failed to copy CA cert — try running as root")?;
        let status = std::process::Command::new("update-ca-certificates").status()?;
        if !status.success() {
            anyhow::bail!("update-ca-certificates failed");
        }
    }

    #[cfg(target_os = "windows")]
    {
        let status = std::process::Command::new("certutil")
            .args(["-addstore", "-user", "Root", cert_str])
            .status()?;
        if !status.success() {
            anyhow::bail!("certutil -addstore failed");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        print_manual_instructions(&cert_path);
    }

    tracing::warn!("ca: CA installed into OS trust store");
    Ok(())
}

fn print_manual_instructions(cert_path: &Path) {
    println!("\nManual CA installation:");
    println!("  macOS:   security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain {:?}", cert_path);
    println!("  Linux:   sudo cp {:?} /usr/local/share/ca-certificates/claudovka-ca.crt && sudo update-ca-certificates", cert_path);
    println!("  Windows: certutil -addstore -user Root {:?}", cert_path);
}

fn pem_cert_to_der(pem: &str) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(pem.as_bytes());
    let item = rustls_pemfile::certs(&mut cursor)
        .next()
        .context("No certificate in PEM")?
        .context("Failed to parse PEM certificate")?;
    Ok(item.to_vec())
}
