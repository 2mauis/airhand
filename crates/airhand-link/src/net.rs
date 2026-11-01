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
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::warn;

use crate::problem::Problem;

/// 连接中转服务器所需的配置：桌面客户端编译进程序，网页控制台从环境变量读取。
#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// 中转服务器地址 `host:port`（UDP）
    pub server: String,
    /// 服务器证书的 SHA-256 指纹。`None` 表示**不校验证书**，仅建议在开发环境使用
    pub pin: Option<[u8; 32]>,
    /// 告诉服务器的本机名称（被控端的托盘里显示"谁在控制"），空字符串表示不知道
    pub name: String,
    /// 本机操作系统，仅用于展示
    pub os: String,
}

impl LinkConfig {
    /// `fingerprint`：服务器日志里打印的 64 位十六进制指纹（允许用 `:` 或空白分隔），空字符串表示不校验。
    pub fn new(
        server: impl Into<String>,
        fingerprint: &str,
        name: impl Into<String>,
        os: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            server: server.into(),
            pin: parse_fingerprint(fingerprint)?,
            name: name.into(),
            os: os.into(),
        })
    }

    /// 是否校验服务器证书
    pub fn pinned(&self) -> bool {
        self.pin.is_some()
    }
}

/// 解析证书指纹：64 位十六进制，允许用 `:` 或空白分隔；空字符串表示不校验（`None`）。
pub fn parse_fingerprint(s: &str) -> Result<Option<[u8; 32]>> {
    if let Some(c) = s
        .chars()
        .find(|c| !c.is_ascii_hexdigit() && *c != ':' && !c.is_ascii_whitespace())
    {
        bail!("the server certificate fingerprint contains {c:?}: expected 64 hex digits");
    }
    let hex: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
    if hex.is_empty() {
        return Ok(None);
    }
    if hex.len() != 64 {
        bail!(
            "the server certificate fingerprint must be 64 hex digits, got {}",
            hex.len()
        );
    }
    let mut out = [0u8; 32];
    for (b, pair) in out.iter_mut().zip(hex.chunks(2)) {
        *b = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(Some(out))
}

/// 与服务器的连接状态（托盘 / 控制台按各自的界面语言显示）
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ServerState {
    Connecting,
    Connected,
    /// 断开的原因（类型，由托盘 / 控制台按各自的界面语言显示）
    Disconnected {
        reason: Problem,
    },
}

/// 连接状态变化的回调
pub type Report = Arc<dyn Fn(ServerState) + Send + Sync>;

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
fn client_config(pin: Option<[u8; 32]>, mismatch: Arc<AtomicBool>) -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
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

/// 解析服务器地址，返回 (地址, SNI)。
async fn resolve(addr: &str) -> Result<(SocketAddr, String)> {
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
pub async fn connect_signal(cfg: &LinkConfig, role: Role) -> std::result::Result<Signal, Problem> {
    let mismatch = Arc::new(AtomicBool::new(false));
    handshake(cfg, role, mismatch.clone()).await.map_err(|e| {
        if mismatch.load(Ordering::Relaxed) {
            Problem::CertMismatch
        } else {
            Problem::from_error(e, |detail| Problem::ServerUnreachable { detail })
        }
    })
}

async fn handshake(cfg: &LinkConfig, role: Role, mismatch: Arc<AtomicBool>) -> Result<Signal> {
    let (addr, sni) = resolve(&cfg.server).await?;
    let bind: SocketAddr = if addr.is_ipv6() {
        "[::]:0".parse()?
    } else {
        "0.0.0.0:0".parse()?
    };
    let mut endpoint = Endpoint::client(bind).context("can't create a UDP socket")?;
    endpoint.set_default_client_config(client_config(cfg.pin, mismatch)?);

    let conn = tokio::time::timeout(Duration::from_secs(8), endpoint.connect(addr, &sni)?)
        .await
        .map_err(|_| anyhow!("timed out connecting to {addr}"))?
        .with_context(|| format!("can't connect to {addr}"))?;
    let (mut send, mut recv) = conn.open_bi().await?;
    write_msg(
        &mut send,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
            role,
            name: cfg.name.clone(),
            os: cfg.os.clone(),
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
/// let mut reconnect = Reconnect::new(report.clone());
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
    report: Report,
    delay: Duration,
    started: Instant,
}

impl Reconnect {
    const FIRST: Duration = Duration::from_millis(500);
    const MAX: Duration = Duration::from_secs(10);
    const STABLE: Duration = Duration::from_secs(20);
    const FATAL: Duration = Duration::from_secs(60);

    pub fn new(report: Report) -> Self {
        Self {
            report,
            delay: Self::FIRST,
            started: Instant::now(),
        }
    }

    /// 开始一次连接
    pub fn begin(&mut self) {
        (self.report)(ServerState::Connecting);
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
        (self.report)(ServerState::Disconnected { reason });
        tokio::time::sleep(wait).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint() {
        let hex = "3a".repeat(32);
        assert_eq!(parse_fingerprint(&hex).unwrap(), Some([0x3a; 32]));
        // 冒号、空白分隔，大小写都可以
        let colons = ["3A"; 32].join(":");
        assert_eq!(parse_fingerprint(&colons).unwrap(), Some([0x3a; 32]));
        assert_eq!(
            parse_fingerprint(&format!(" {} \n", ["3a"; 32].join(" "))).unwrap(),
            Some([0x3a; 32])
        );
        assert_eq!(parse_fingerprint("").unwrap(), None);
        assert_eq!(parse_fingerprint("  ").unwrap(), None);
        assert!(parse_fingerprint(&"3a".repeat(31)).is_err());
        // 整行日志粘贴进来：报错而不是从里面凑出 64 个"十六进制"字符
        assert!(parse_fingerprint(&format!("fingerprint: {hex}")).is_err());
    }
}
