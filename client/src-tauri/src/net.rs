//! QUIC 连接与信令握手（被控端与控制端共用）。

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{ClientMsg, ServerMsg};
use airhand_proto::{ALPN, MAX_SIGNAL_MSG, PROTOCOL_VERSION, Role};
use anyhow::{Context, Result, anyhow, bail};
use quinn::crypto::rustls::QuicClientConfig;
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream, TransportConfig, VarInt};
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::config;
use crate::problem::Problem;
use crate::status::{HubExt, ServerState, StatusHub};

pub struct Signal {
    pub conn: Connection,
    pub send: SendStream,
    pub recv: RecvStream,
    /// 保持 Endpoint 存活
    _endpoint: Endpoint,
}

pub fn transport_config() -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_concurrent_bidi_streams(VarInt::from_u32(64));
    t.max_concurrent_uni_streams(VarInt::from_u32(0));
    // 短保活间隔：既能防止 NAT 映射过期，也能让掉线在 10 秒内被发现并重连
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
    t.keep_alive_interval(Some(Duration::from_secs(2)));
    t.stream_receive_window(VarInt::from_u32(16 * 1024 * 1024));
    t.receive_window(VarInt::from_u32(64 * 1024 * 1024));
    t.send_window(64 * 1024 * 1024);
    t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    t
}

/// `mismatch`：证书指纹不符时置位（TLS 错误经过 QUIC 之后只剩一段文字，用它来可靠地识别）。
fn client_config(mismatch: Arc<AtomicBool>) -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pin = parse_fingerprint(config::SERVER_CERT_SHA256)?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedVerifier {
            provider,
            pin,
            mismatch,
        }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![ALPN.to_vec()];
    let mut cfg = ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
    cfg.transport_config(Arc::new(transport_config()));
    Ok(cfg)
}

fn parse_fingerprint(s: &str) -> Result<Option<[u8; 32]>> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.is_empty() {
        return Ok(None);
    }
    if hex.len() != 64 {
        bail!("SERVER_CERT_SHA256 must be 64 hex digits");
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)?;
    }
    Ok(Some(out))
}

/// 解析服务器地址，返回 (地址, SNI)。
async fn resolve() -> Result<(SocketAddr, String)> {
    let addr = config::SERVER_ADDR;
    let host = addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(addr)
        .trim_matches(['[', ']']);
    let sni = if host.parse::<std::net::IpAddr>().is_ok() {
        "airhand".to_string()
    } else {
        host.to_string()
    };
    let mut addrs = tokio::net::lookup_host(addr)
        .await
        .with_context(|| format!("can't resolve {addr}"))?;
    let first = addrs
        .next()
        .ok_or_else(|| anyhow!("{addr} has no usable IP address"))?;
    Ok((first, sni))
}

/// 连接服务器并完成 Hello 握手。
///
/// 失败时返回的 [`Problem`]：证书指纹不符 → `CertMismatch`，版本不一致 → `Incompatible`，
/// 其它（解析地址、网络、超时……）→ `ServerUnreachable`。
pub async fn connect_signal(role: Role) -> std::result::Result<Signal, Problem> {
    let mismatch = Arc::new(AtomicBool::new(false));
    handshake(role, mismatch.clone()).await.map_err(|e| {
        if mismatch.load(Ordering::Relaxed) {
            Problem::CertMismatch
        } else {
            Problem::from_error(e, |detail| Problem::ServerUnreachable { detail })
        }
    })
}

async fn handshake(role: Role, mismatch: Arc<AtomicBool>) -> Result<Signal> {
    let (addr, sni) = resolve().await?;
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = Endpoint::client(bind).context("can't create a UDP socket")?;
    endpoint.set_default_client_config(client_config(mismatch)?);

    let conn = tokio::time::timeout(Duration::from_secs(8), endpoint.connect(addr, &sni)?)
        .await
        .map_err(|_| anyhow!("timed out connecting to {addr}"))?
        .with_context(|| format!("can't connect to {addr}"))?;
    let (mut send, mut recv) = conn.open_bi().await?;
    let (name, os) = crate::sysinfo::host_identity();
    write_msg(
        &mut send,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            role,
            name,
            os,
        },
    )
    .await?;
    let mut buf = Vec::new();
    let resp: Option<ServerMsg> = tokio::time::timeout(
        Duration::from_secs(8),
        read_msg(&mut recv, MAX_SIGNAL_MSG, &mut buf),
    )
    .await
    .map_err(|_| anyhow!("server handshake timed out"))??;
    match resp {
        Some(ServerMsg::Welcome { .. }) => Ok(Signal {
            conn,
            send,
            recv,
            _endpoint: endpoint,
        }),
        Some(ServerMsg::Incompatible { server_version }) => Err(Problem::Incompatible {
            server: server_version,
            client: PROTOCOL_VERSION,
        }
        .into()),
        Some(other) => bail!("unexpected message from the server: {other:?}"),
        None => bail!("the server closed the connection during the handshake"),
    }
}

/// 证书固定校验器：只比较叶子证书的 SHA-256 指纹，但仍然校验 TLS 1.3 握手签名。
#[derive(Debug)]
struct PinnedVerifier {
    provider: Arc<CryptoProvider>,
    pin: Option<[u8; 32]>,
    mismatch: Arc<AtomicBool>,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match self.pin {
            None => Ok(ServerCertVerified::assertion()),
            Some(pin) => {
                let got: [u8; 32] = Sha256::digest(end_entity.as_ref()).into();
                if got == pin {
                    Ok(ServerCertVerified::assertion())
                } else {
                    self.mismatch.store(true, Ordering::Relaxed);
                    Err(rustls::Error::General(
                        "server certificate fingerprint mismatch".into(),
                    ))
                }
            }
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

/// 与服务器的断线重连（被控端、控制端共用）：
///
/// ```ignore
/// let mut reconnect = Reconnect::new(hub.clone());
/// loop {
///     reconnect.begin();
///     let result = connect_once(..).await; // 一直运行到断开
///     reconnect.end("host", result).await;
/// }
/// ```
///
/// 带抖动的指数退避（0.5 → 10 秒）；连上后稳定超过 20 秒再断开就从头开始；
/// 版本不一致这类重试也没用的问题每分钟才重试一次。
pub struct Reconnect {
    hub: StatusHub,
    delay: Duration,
    started: Instant,
}

impl Reconnect {
    const FIRST: Duration = Duration::from_millis(500);
    const MAX: Duration = Duration::from_secs(10);
    const STABLE: Duration = Duration::from_secs(20);
    const FATAL: Duration = Duration::from_secs(60);

    pub fn new(hub: StatusHub) -> Self {
        Self {
            hub,
            delay: Self::FIRST,
            started: Instant::now(),
        }
    }

    /// 开始一次连接
    pub fn begin(&mut self) {
        self.hub.set_server(ServerState::Connecting);
        self.started = Instant::now();
    }

    /// 一次连接结束。`Ok` 表示需要立即重连（例如更换控制码）；`Err` 表示掉线：记录原因，等待后返回。
    ///
    /// [`connect_signal`] 的错误本身就是 [`Problem`]；其它（连上之后的）错误算作"连接断开"。
    pub async fn end(&mut self, role: &str, result: Result<()>) {
        let Err(err) = result else {
            self.delay = Self::FIRST;
            return;
        };
        let reason = Problem::from_error(err, |detail| Problem::ServerLost { detail });
        warn!("{role} disconnected from the server: {reason}");
        let wait = if reason.is_fatal() {
            Self::FATAL
        } else {
            if self.started.elapsed() > Self::STABLE {
                self.delay = Self::FIRST;
            }
            let d = self.delay;
            self.delay = (d * 2).min(Self::MAX);
            d + Duration::from_millis(rand::random_range(0..=d.as_millis() as u64 / 4))
        };
        self.hub.set_server(ServerState::Disconnected { reason });
        tokio::time::sleep(wait).await;
    }
}
