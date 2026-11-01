//! 网页的 HTTPS 证书。
//!
//! 浏览器只在安全的页面（HTTPS 或 localhost）上提供 WebCodecs 视频解码，所以从其它电脑打开控制台必须用 HTTPS。
//! 默认用自签名证书（浏览器第一次打开时会提示不受信任，确认继续即可）；也可以换成自己的证书。

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};
use tokio_rustls::TlsAcceptor;
use tracing::info;

pub struct Tls {
    pub acceptor: TlsAcceptor,
    /// 叶子证书的 SHA-256 指纹（浏览器的证书详情里能看到，可以核对）
    pub fingerprint: String,
}

/// 读取证书；文件不存在时生成一张自签名证书写进去（之后重启都用它，浏览器就不会每次都提示）。
pub fn load_or_generate(cert_path: &Path, key_path: &Path, names: &[String]) -> Result<Tls> {
    if !cert_path.exists() || !key_path.exists() {
        info!(
            "no HTTPS certificate found, generating a self-signed one: {}",
            cert_path.display()
        );
        let ck = rcgen::generate_simple_self_signed(names.to_vec())
            .context("can't generate the certificate")?;
        for dir in [cert_path.parent(), key_path.parent()]
            .into_iter()
            .flatten()
        {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(cert_path, ck.cert.pem())
            .with_context(|| format!("can't write the certificate: {}", cert_path.display()))?;
        std::fs::write(key_path, ck.signing_key.serialize_pem())
            .with_context(|| format!("can't write the private key: {}", key_path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert_path)
        .with_context(|| format!("can't read the certificate: {}", cert_path.display()))?
        .collect::<Result<_, _>>()
        .context("can't parse the certificate")?;
    anyhow::ensure!(
        !chain.is_empty(),
        "the certificate file is empty: {}",
        cert_path.display()
    );
    let key = PrivateKeyDer::from_pem_file(key_path)
        .with_context(|| format!("can't read the private key: {}", key_path.display()))?;
    build(chain, key)
}

/// 临时生成一张自签名证书（只在内存里，重启后会换一张）。
pub fn generate(names: &[String]) -> Result<Tls> {
    let ck = rcgen::generate_simple_self_signed(names.to_vec())
        .context("can't generate the certificate")?;
    let key = PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der());
    build(vec![ck.cert.der().clone()], key.into())
}

fn build(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>) -> Result<Tls> {
    let fingerprint = Sha256::digest(chain[0].as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .context("the certificate and private key don't match")?;
    // WebSocket 走 HTTP/1.1 的 Upgrade
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Tls {
        acceptor: TlsAcceptor::from(Arc::new(config)),
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_signed() {
        let a = generate(&["localhost".into(), "192.168.1.10".into()]).unwrap();
        assert_eq!(a.fingerprint.len(), 64);

        let dir = std::env::temp_dir().join(format!("airhand-web-tls-{}", std::process::id()));
        let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
        let first = load_or_generate(&cert, &key, &["localhost".into()]).unwrap();
        // 第二次读取同一张证书
        let again = load_or_generate(&cert, &key, &["localhost".into()]).unwrap();
        assert_eq!(first.fingerprint, again.fingerprint);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
