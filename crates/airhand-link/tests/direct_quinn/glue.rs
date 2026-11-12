//! 真实 quinn 前置核实（PV-2 / PV-3）用例的胶水层：两端 quinn 0.11 端点 + 可注入时延、单向丢包、带宽与队列的
//! 内存数据报通道（`docs/specs/tailcat-p2p.md` §2 O / H）。
//!
//! Uses the standalone signal source and independent direct transport configuration.
//! Both noop polarities still bypass all real work and return no observations.
#![allow(dead_code)]

use std::time::Duration;

/// 通道的一端
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Host,
    Controller,
}

impl Side {
    pub fn peer(self) -> Self {
        match self {
            Self::Host => Self::Controller,
            Self::Controller => Self::Host,
        }
    }
}

/// 数据报方向：`from` 端发往对端
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dir {
    pub from: Side,
}

/// 握手里要丢的那一个包
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakePacket {
    /// 被控端（服务端）的首个飞行
    HostFirstFlight,
    /// 控制端的 Finished
    ControllerFinished,
}

/// 探测流的发送优先级
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbePriority {
    /// `SendStream::set_priority` 设为最高（规格要求）
    Highest,
    /// 默认优先级（对照实现）
    Default,
}

/// 内存通道与端点的设置
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    pub rtt: Duration,
    /// 控制端→被控端的单程时延 d1（`None` = RTT 的一半）
    pub up: Option<Duration>,
    /// 每个数据报的时延抖动上限（均匀分布 ±jitter）
    pub jitter: Duration,
    /// 带宽限制（bit/s，`None` = 不限）
    pub bandwidth_bps: Option<u64>,
    /// 尾部丢弃 FIFO 的排队时延上限（`None` = 不排队）
    pub queue_limit: Option<Duration>,
    pub ack_stall: Duration,
    pub handshake_watchdog: Duration,
    pub probe_priority: ProbePriority,
    /// 一条持续写满的媒体流把拥塞窗口占满
    pub media_saturation: bool,
    /// 握手期间要丢的单个包
    pub handshake_drop: Option<HandshakePacket>,
    pub seed: u64,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            rtt: Duration::from_millis(100),
            up: None,
            jitter: Duration::ZERO,
            bandwidth_bps: None,
            queue_limit: None,
            ack_stall: Duration::from_millis(1000),
            handshake_watchdog: Duration::from_millis(1500),
            probe_priority: ProbePriority::Highest,
            media_saturation: false,
            handshake_drop: None,
            seed: 1,
        }
    }
}

/// 注入的丢包：`[from, until)` 内沿 `dir` 的数据报全部丢弃（`until = None` 表示一直丢）
#[derive(Debug, Clone, Copy)]
pub struct Loss {
    pub dir: Option<Dir>,
    pub from: Duration,
    pub until: Option<Duration>,
}

impl Loss {
    pub fn one_way(from_side: Side, from: Duration) -> Self {
        Self {
            dir: Some(Dir { from: from_side }),
            from,
            until: None,
        }
    }

    /// 双向完全丢包突发
    pub fn burst(at: Duration, length: Duration) -> Self {
        Self {
            dir: None,
            from: at,
            until: Some(at + length),
        }
    }
}

/// 一次运行的观测（时刻相对握手完成）
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// 握手完成的时刻（相对连接发起）；`None` = 没有完成
    pub handshake_completed: Option<Duration>,
    /// 握手是否被 H 放弃
    pub handshake_abandoned: bool,
    /// O 的进展事件（探测流 `stopped()` 返回 `Ok(None)`）
    pub progress: Vec<(Duration, Side)>,
    /// 探测流发出的时刻
    pub probes_sent: Vec<(Duration, Side)>,
    /// 收到的 ACK 帧（`ConnectionStats` 的帧计数，变异实现的错误信号）
    pub ack_frames_received: Vec<(Duration, Side)>,
    /// 发出的 ack-eliciting 包
    pub ack_eliciting_sent: Vec<(Duration, Side)>,
    /// 直连数据报到达对端的时刻（按接收侧）
    pub inbound: Vec<(Duration, Side)>,
    /// 对端 accept 到并读完丢弃的探测流个数
    pub probes_accepted: Vec<(Side, usize)>,
    /// 额度用尽而本周期不发的探测流个数
    pub probes_skipped: Vec<(Side, usize)>,
    /// 任一时刻在途（未被确认）的探测流个数最大值
    pub max_in_flight_probes: Vec<(Side, usize)>,
    /// 对端给的 `max_concurrent_uni_streams`
    pub peer_uni_stream_limit: Vec<(Side, u64)>,
    /// 中转 `transport_config()` 的 `max_concurrent_uni_streams`
    pub relay_uni_stream_limit: Option<u64>,
    /// 探测流实际使用的发送优先级
    pub probe_priority: Option<i32>,
    /// 测得的含排队 RTT 最大值
    pub max_queued_rtt: Option<Duration>,
    /// 媒体流吞吐（字节）
    pub media_bytes: u64,
    /// quinn 是否在突发后的包里持续携带旧 ACK 区间（观测，`None` = 无法判定）
    pub carried_old_ack_ranges: Option<bool>,
}

impl Observed {
    pub fn progress_of(&self, side: Side) -> Vec<Duration> {
        self.progress
            .iter()
            .filter(|p| p.1 == side)
            .map(|p| p.0)
            .collect()
    }

    pub fn probes_of(&self, side: Side) -> Vec<Duration> {
        self.probes_sent
            .iter()
            .filter(|p| p.1 == side)
            .map(|p| p.0)
            .collect()
    }

    pub fn inbound_of(&self, side: Side) -> Vec<Duration> {
        self.inbound
            .iter()
            .filter(|p| p.1 == side)
            .map(|p| p.0)
            .collect()
    }

    pub fn ack_frames_of(&self, side: Side) -> Vec<Duration> {
        self.ack_frames_received
            .iter()
            .filter(|p| p.1 == side)
            .map(|p| p.0)
            .collect()
    }

    pub fn ack_eliciting_of(&self, side: Side) -> Vec<Duration> {
        self.ack_eliciting_sent
            .iter()
            .filter(|p| p.1 == side)
            .map(|p| p.0)
            .collect()
    }

    fn per_side<T: Copy + Default>(v: &[(Side, T)], side: Side) -> T {
        v.iter()
            .find(|x| x.0 == side)
            .map(|x| x.1)
            .unwrap_or_default()
    }

    pub fn accepted_of(&self, side: Side) -> usize {
        Self::per_side(&self.probes_accepted, side)
    }

    pub fn skipped_of(&self, side: Side) -> usize {
        Self::per_side(&self.probes_skipped, side)
    }

    pub fn max_in_flight_of(&self, side: Side) -> usize {
        Self::per_side(&self.max_in_flight_probes, side)
    }

    pub fn peer_limit_of(&self, side: Side) -> u64 {
        Self::per_side(&self.peer_uni_stream_limit, side)
    }
}

#[path = "channel.rs"]
mod channel;

use airhand_link::direct::{direct_transport_config, probe::ProbeSource};
use std::sync::Arc;
use tokio::time::{Instant, timeout};

/// Real endpoints and datagrams; only time and network conditions are simulated.
pub fn run(cfg: &ChannelConfig, losses: &[Loss], run_for: Duration) -> Observed {
    if cfg!(any(feature = "noop-lenient", feature = "noop-strict"))
        || std::env::var("AIRHAND_TEST_NOOP").is_ok_and(|v| v == "lenient" || v == "strict")
    {
        return Observed::default();
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(run_async(cfg, losses, run_for))
}

async fn run_async(cfg: &ChannelConfig, losses: &[Loss], run_for: Duration) -> Observed {
    let state = channel::State::new(cfg, losses);
    let [hs, cs] = channel::pair(state.clone());
    let cert =
        rustls::pki_types::CertificateDer::from(include_bytes!("fixtures/cert.der").to_vec());
    let key =
        rustls::pki_types::PrivatePkcs8KeyDer::from(include_bytes!("fixtures/key.der").to_vec());
    let mut server = quinn::ServerConfig::with_single_cert(vec![cert.clone()], key.into()).unwrap();
    // PV-2 intentionally holds loss for 10 s to exhaust 16 probes. Keep the
    // measurement connection alive beyond that interval; no product gate is run.
    let mut transport = direct_transport_config();
    transport.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    let peer_uni_limit = format!("{transport:?}")
        .split("max_concurrent_uni_streams: ")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.parse::<u64>().ok())
        .expect("transport Debug exposes configured credit");
    let transport = Arc::new(transport);
    server.transport_config(transport.clone());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let mut client = quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap();
    client.transport_config(transport);
    let mut endpoint_cfg = quinn::EndpointConfig::default();
    endpoint_cfg.max_udp_payload_size(1232).unwrap();
    let host = quinn::Endpoint::new_with_abstract_socket(
        endpoint_cfg.clone(),
        Some(server),
        hs,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let mut ctrl = quinn::Endpoint::new_with_abstract_socket(
        endpoint_cfg,
        None,
        cs,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    ctrl.set_default_client_config(client);
    let connect = ctrl
        .connect(host.local_addr().unwrap(), "localhost")
        .unwrap();
    let accept = async { host.accept().await.unwrap().await };
    let start = Instant::now();
    let (h, c) = tokio::join!(
        timeout(Duration::from_secs(5), accept),
        timeout(Duration::from_secs(5), connect)
    );
    let (h, c) = match (h, c) {
        (Ok(Ok(h)), Ok(Ok(c))) => (h, c),
        other => panic!("ASSERT: real Quinn fixture handshake failed: {other:?}"),
    };
    let origin = Instant::now();
    {
        let mut s = state.lock().unwrap();
        s.origin = Some(origin);
        s.observed.handshake_completed = Some(origin - start);
        let since_inbound = origin - s.first_host_inbound.unwrap();
        s.observed.handshake_abandoned =
            since_inbound + Duration::from_millis(50) >= cfg.handshake_watchdog;
        let abandoned = s.observed.handshake_abandoned;
        s.trace.push(format!("host handshake complete at {:?}; since first inbound {since_inbound:?}; H + IPC exceeded={}", origin-start, abandoned));
        if std::env::var_os("P2P_TRACE").is_some() {
            eprintln!(
                "TRACE RTT {:?} drop {:?}\n{}",
                cfg.rtt,
                cfg.handshake_drop,
                s.trace.join("\n")
            );
        }
    }
    let priority = if cfg.probe_priority == ProbePriority::Highest {
        i32::MAX
    } else {
        0
    };
    let mut hp = if priority == i32::MAX {
        ProbeSource::start(h.clone())
    } else {
        ProbeSource::with_priority(h.clone(), priority)
    };
    let mut cp = if priority == i32::MAX {
        ProbeSource::start(c.clone())
    } else {
        ProbeSource::with_priority(c.clone(), priority)
    };
    let mut media = tokio::task::JoinSet::new();
    if cfg.media_saturation {
        let send = h.clone();
        media.spawn(async move {
            let mut stream = send.open_bi().await.unwrap().0;
            let data = vec![0u8; 64 * 1024];
            while stream.write_all(&data).await.is_ok() {}
        });
        let recv = c.clone();
        let stats = state.clone();
        media.spawn(async move {
            let mut stream = recv.accept_bi().await.unwrap().1;
            let mut data = vec![0u8; 64 * 1024];
            while let Ok(Some(n)) = stream.read(&mut data).await {
                stats.lock().unwrap().observed.media_bytes += n as u64;
            }
        });
    }
    let end = origin + run_for;
    let mut tick = tokio::time::interval(Duration::from_millis(5));
    let mut acks = [0; 2];
    let mut eliciting = [0; 2];
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(end) => break,
            Some(at) = hp.sent.recv() => state.lock().unwrap().observed.probes_sent.push((at.duration_since(origin.into_std()), Side::Host)),
            Some(at) = cp.sent.recv() => state.lock().unwrap().observed.probes_sent.push((at.duration_since(origin.into_std()), Side::Controller)),
            Some(at) = hp.progress.recv() => state.lock().unwrap().observed.progress.push((at.duration_since(origin.into_std()), Side::Host)),
            Some(at) = cp.progress.recv() => state.lock().unwrap().observed.progress.push((at.duration_since(origin.into_std()), Side::Controller)),
            _ = tick.tick() => {
                let now = Instant::now() - origin;
                let mut s = state.lock().unwrap();
                for (i, (conn, side)) in [(&h, Side::Host), (&c, Side::Controller)].into_iter().enumerate() {
                    let stats = conn.stats();
                    s.observed.max_queued_rtt = Some(s.observed.max_queued_rtt.unwrap_or_default().max(stats.path.rtt));
                    if stats.frame_rx.acks > acks[i] { s.observed.ack_frames_received.push((now, side)); acks[i] = stats.frame_rx.acks; }
                    let n = stats.frame_tx.stream + stats.frame_tx.ping + stats.frame_tx.crypto;
                    if n > eliciting[i] { s.observed.ack_eliciting_sent.push((now, side)); eliciting[i] = n; }
                }
            }
        }
    }
    if std::env::var_os("P2P_TRACE").is_some() {
        eprintln!(
            "END {:?} media {}: host close {:?}; controller close {:?}; host probes {:?}; controller probes {:?}",
            run_for,
            cfg.media_saturation,
            h.close_reason(),
            c.close_reason(),
            hp.stats.lock().unwrap(),
            cp.stats.lock().unwrap()
        );
    }
    let mut s = state.lock().unwrap();
    for (source, side) in [(&hp, Side::Host), (&cp, Side::Controller)] {
        let stats = source.stats.lock().unwrap();

        s.observed.probes_accepted.push((side, stats.accepted));
        s.observed.probes_skipped.push((side, stats.skipped));
        s.observed
            .max_in_flight_probes
            .push((side, stats.max_in_flight));
        s.observed
            .peer_uni_stream_limit
            .push((side, peer_uni_limit));
    }
    // TransportConfig's Debug view exposes the configured peer credit without
    // adding product getters solely for the fixture.
    s.observed.relay_uni_stream_limit = Some(
        if format!("{:?}", airhand_link::net::transport_config())
            .contains("max_concurrent_uni_streams: 0")
        {
            0
        } else {
            u64::MAX
        },
    );
    s.observed.probe_priority = Some(priority);
    s.observed.carried_old_ack_ranges = None; // frame counters cannot establish which ranges an ACK carried
    h.close(0u32.into(), b"PV done");
    c.close(0u32.into(), b"PV done");
    s.observed.clone()
}
