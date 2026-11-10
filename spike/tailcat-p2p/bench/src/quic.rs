//! QUIC endpoints for the direct path: airhand's relay transport settings
//! plus the 1232-byte datagram cap (spec I11), a per-session self-signed
//! certificate and fingerprint pinning.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, EndpointConfig, ServerConfig, TransportConfig};
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};

pub const DIRECT_ALPN: &[u8] = b"airhand-direct-spike/0";
pub const MAX_DGRAM: u16 = 1232;

pub fn direct_transport_config() -> TransportConfig {
    let mut t = airhand_link::net::transport_config();
    t.initial_mtu(MAX_DGRAM);
    t.min_mtu(1200);
    t.mtu_discovery_config(None);
    t
}

pub fn direct_endpoint_config() -> EndpointConfig {
    let mut e = EndpointConfig::default();
    e.max_udp_payload_size(MAX_DGRAM).unwrap();
    e
}

pub struct HostCert {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub fp: [u8; 32],
}

pub fn host_cert() -> Result<HostCert> {
    let ck = rcgen::generate_simple_self_signed(vec!["airhand-direct".into()])?;
    let der = ck.cert.der().to_vec();
    let fp: [u8; 32] = Sha256::digest(&der).into();
    Ok(HostCert {
        chain: vec![CertificateDer::from(der)],
        key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()).into(),
        fp,
    })
}

pub fn server_config(c: &HostCert) -> Result<ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(c.chain.clone(), c.key.clone_key())?;
    tls.alpn_protocols = vec![DIRECT_ALPN.to_vec()];
    tls.max_early_data_size = 0;
    let mut cfg = ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
    cfg.transport_config(Arc::new(direct_transport_config()));
    Ok(cfg)
}

pub fn client_config(fp: [u8; 32]) -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { provider, fp }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![DIRECT_ALPN.to_vec()];
    let mut cfg = ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    cfg.transport_config(Arc::new(direct_transport_config()));
    Ok(cfg)
}

#[derive(Debug)]
struct Pinned {
    provider: Arc<CryptoProvider>,
    fp: [u8; 32],
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
        if got == self.fp {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("direct certificate pin mismatch".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex32(s: &str) -> Result<[u8; 32]> {
    let v: Vec<u8> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| anyhow!("bad fingerprint"))
}


/// Placeholder addresses for the pipe socket: no real port is bound.
pub fn pipe_local() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}
pub fn pipe_peer() -> SocketAddr {
    "127.0.0.2:2".parse().unwrap()
}
