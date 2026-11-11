//! 真实 quinn 前置核实（PV-2 / PV-3）用例的胶水层：两端 quinn 0.11 端点 + 可注入时延、单向丢包、带宽与队列的
//! 内存数据报通道（`docs/specs/tailcat-p2p.md` §2 O / H）。
//!
//! 尚未实现：这里只有让 `tests/direct_quinn.rs` 能编译的类型与签名，全部是空操作——握手不完成、不开探测流、
//! 不记录任何事件，用例因正向对照不成立而以 `ASSERT:` 失败；两种空操作极性（`noop-lenient` / `noop-strict`）
//! 下行为相同。PV PR 把这里换成真实端点（探测流、`direct_transport_config()`、通道模型），不改用例本身。
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

/// 建连、按 `losses` 注入丢包、运行 `run_for`（握手完成后起算），返回观测。空操作：什么都不做。
pub fn run(_cfg: &ChannelConfig, _losses: &[Loss], _run_for: Duration) -> Observed {
    Observed::default()
}
