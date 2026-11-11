//! 直连会话的确定性场景驱动（虚拟时钟、假 helper 工厂、假 tailcat 路径与假中转），供 `tests/direct.rs` 使用。
//!
//! 场景只描述“发生了什么”（控制端连上、用户允许、某一侧闸门打开、helper 崩溃……），[`Trace`] 记录两端
//! 对外可观察的行为（各通道上发出的消息、helper 的启动 / 结束 / 网络活动、直连 QUIC 包、闸门、码率……）。
//! Phase 1 把 [`Scenario::run`] 接到 `airhand_link` 的直连实现上；在此之前它一律返回 [`NotWired`]，
//! 所以每个用例都在第一条断言处失败（先红）。
// `Trace` 及其记录类型要到 Phase 1 接线后才会被构造，部分构造器也只有个别用例用到。
#![allow(dead_code)]

use std::fmt;
use std::time::Duration;

use airhand_proto::session::ByeReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Host,
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Relay,
    Direct,
}

/// 被控端 `[host] approve`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approve {
    Ask,
    Never,
}

/// 被控端画面：静止桌面不编码、不发包，也就没有 Ack（存活检测不能依赖画面）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Static,
    Moving,
}

/// 控制端连接时使用的控制码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeChoice {
    /// 被控端当前的控制码
    Current,
    /// 最近一次“停止共享”之前的控制码
    Previous,
}

/// 被控端配置（`[host] approve` 与 `[direct]`），默认值与规格「配置」一致
#[derive(Debug, Clone)]
pub struct HostConfig {
    pub approve: Approve,
    pub direct_enabled: bool,
    pub bitrate_cap_kbps: u32,
    pub rx_silence: Duration,
    pub write_timeout: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            approve: Approve::Ask,
            direct_enabled: false,
            bitrate_cap_kbps: 100_000,
            rx_silence: Duration::from_millis(1000),
            write_timeout: Duration::from_millis(500),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub direct_enabled: bool,
    /// `ControllerHello::device`
    pub device: [u8; 16],
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            direct_enabled: false,
            device: [1; 16],
        }
    }
}

/// 场景里发生的事。`controller` 是 [`Scenario::controller`] 加入的顺序（0 起）。
#[derive(Debug, Clone)]
pub enum Event {
    /// 控制端经中转连上被控端并发 `ControllerMsg::Hello`
    Connect { controller: u8, code: CodeChoice },
    /// 被控端用户在确认窗点“允许”，`direct` 为“允许直连”勾选
    Approve { controller: u8, direct: bool },
    /// 该侧的路径判定认为当前 `epoch` 已是 direct（被控端：`CurAddr` 非空；控制端：DiscoPing 走直连）
    GateOpen { side: Side, controller: u8 },
    /// 之后中转单向时延改为该值（拉长栅栏等待）
    RelayLatency(Duration),
    /// 之后控制端在直连上收不到 `Pong`
    StopPongs { controller: u8 },
    /// 该侧 helper 进程崩溃退出
    HelperCrash { side: Side, controller: u8 },
    /// 该侧 helper 往隧道的写入从此永久阻塞（假写路径，I25）
    HelperWriteStall { side: Side, controller: u8 },
    /// 直连路径失效（路径判定离开 direct）
    DirectPathFails { controller: u8 },
    /// 自适应码率请求的目标码率
    TargetBitrate { controller: u8, kbps: u32 },
    /// 被控端编码出一帧这么大的画面（例如关键帧）
    VideoFrame { controller: u8, bytes: usize },
    /// 不按协议行事的控制端：不等 `Hello` 授予直连就发 `DirectRequest`
    ForceDirectRequest { controller: u8 },
    /// 经中转向控制端投递一条 `DirectGateOpen`，`epoch` 为当前升级的 `epoch` 加上 `epoch_delta`
    InjectDirectGateOpen { controller: u8, epoch_delta: i64 },
    /// 控制端用已经用过的直连令牌再连一次
    ReplayDirectToken { controller: u8 },
    /// 被控端用户“断开此控制端”（保留控制码）
    Kick { controller: u8 },
    /// 被控端用户“停止共享”（换码）
    StopSharing,
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub host: HostConfig,
    pub controllers: Vec<ControllerConfig>,
    pub screen: Screen,
    pub events: Vec<(Duration, Event)>,
    pub run_for: Duration,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            host: HostConfig::default(),
            controllers: Vec::new(),
            screen: Screen::Static,
            events: Vec::new(),
            run_for: Duration::from_secs(30),
        }
    }
}

impl Scenario {
    pub fn new(host: HostConfig) -> Self {
        Self {
            host,
            ..Self::default()
        }
    }

    pub fn controller(mut self, c: ControllerConfig) -> Self {
        self.controllers.push(c);
        self
    }

    pub fn screen(mut self, screen: Screen) -> Self {
        self.screen = screen;
        self
    }

    pub fn at(mut self, ms: u64, e: Event) -> Self {
        self.events.push((Duration::from_millis(ms), e));
        self
    }

    pub fn run_for(mut self, secs: u64) -> Self {
        self.run_for = Duration::from_secs(secs);
        self
    }

    pub fn run(&self) -> Result<Trace, NotWired> {
        Err(NotWired)
    }
}

#[derive(Debug)]
pub struct NotWired;

impl fmt::Display for NotWired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("airhand_link 还没有直连实现（tailcat-p2p Phase 1），场景无法运行")
    }
}

/// 控制流消息（只记与直连 / 确认相关的部分）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Hello {
        granted_direct: bool,
    },
    AwaitingApproval,
    DirectRequest {
        epoch: u32,
    },
    DirectOffer {
        epoch: u32,
    },
    DirectDecline {
        epoch: u32,
    },
    DirectGateOpen {
        epoch: u32,
    },
    DirectSwitch {
        epoch: u32,
    },
    DirectCommit {
        epoch: u32,
    },
    DirectFallback {
        epoch: u32,
    },
    Ping,
    Pong,
    /// `None` 是控制端的无字段 `ControllerMsg::Bye`
    Bye(Option<ByeReason>),
}

impl Msg {
    pub fn is_direct_control(&self) -> bool {
        matches!(
            self,
            Self::DirectRequest { .. }
                | Self::DirectOffer { .. }
                | Self::DirectDecline { .. }
                | Self::DirectGateOpen { .. }
                | Self::DirectSwitch { .. }
                | Self::DirectCommit { .. }
                | Self::DirectFallback { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub struct Sent {
    pub at: Duration,
    /// 对端读到它的时间；`None` 表示没有送达（被丢弃、会话已结束……）
    pub delivered: Option<Duration>,
    pub from: Side,
    pub controller: u8,
    pub path: Path,
    pub msg: Msg,
}

#[derive(Debug, Clone)]
pub struct HelperEvent {
    pub at: Duration,
    pub side: Side,
    pub controller: u8,
}

#[derive(Debug, Clone)]
pub struct NetworkActivity {
    pub at: Duration,
    pub side: Side,
    pub controller: u8,
    /// 例如 "derp-map"、"derp-connect"、"disco-ping"
    pub what: &'static str,
}

/// 直连 QUIC 数据报（helper 隧道上看到的）
#[derive(Debug, Clone)]
pub struct Datagram {
    pub at: Duration,
    pub from: Side,
    pub controller: u8,
    pub epoch: u32,
    pub size: usize,
    /// 因对端闸门关闭而被丢弃
    pub dropped_by_gate: bool,
    /// 超过 1232 字节，被假 helper 记录后丢弃
    pub dropped_oversize: bool,
}

#[derive(Debug, Clone)]
pub struct GateChange {
    pub at: Duration,
    pub side: Side,
    pub controller: u8,
    pub epoch: u32,
    pub open: bool,
}

#[derive(Debug, Clone)]
pub struct SenderPath {
    pub at: Duration,
    pub side: Side,
    pub controller: u8,
    pub path: Path,
}

#[derive(Debug, Clone)]
pub struct BitrateTarget {
    pub at: Duration,
    pub controller: u8,
    pub path: Path,
    /// 实际交给编码器的目标码率
    pub kbps: u32,
}

#[derive(Debug, Clone)]
pub struct ConnectAttempt {
    pub at: Duration,
    pub controller: u8,
    pub code: CodeChoice,
    pub accepted: bool,
}

#[derive(Debug, Clone)]
pub struct Upgrade {
    pub controller: u8,
    pub epoch: u32,
    /// 两端都在直连上收发控制流的时刻
    pub completed: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct Trace {
    pub sent: Vec<Sent>,
    pub helper_spawns: Vec<HelperEvent>,
    pub helper_exits: Vec<HelperEvent>,
    pub network: Vec<NetworkActivity>,
    pub approval_prompts: Vec<(Duration, u8)>,
    pub datagrams: Vec<Datagram>,
    /// 直连 QUIC 因 Initial 丢失而走 PTO 的重传次数
    pub pto_retransmits: usize,
    pub gates: Vec<GateChange>,
    pub sender_paths: Vec<SenderPath>,
    /// 直连专用 `direct_transport_config()` 的 `keep_alive_interval`
    pub direct_keep_alive: Option<Duration>,
    pub bitrate: Vec<BitrateTarget>,
    pub connects: Vec<ConnectAttempt>,
    pub sessions_ended: Vec<(Duration, u8)>,
    pub code_changes: Vec<Duration>,
    /// 每次直连令牌校验的结果（按时间顺序）
    pub token_checks: Vec<bool>,
    pub upgrades: Vec<Upgrade>,
    /// 因 I2 / I25 回退后计入“30 秒、最多 3 次”的重试次数
    pub upgrade_retries: Vec<(u8, u32)>,
    /// 本次运行产生的地址 / PSK / 令牌 / 密钥的明文
    pub secrets: Vec<String>,
    /// 两端写出的日志与 `{:?}` 输出
    pub logs: Vec<String>,
    /// 两端显示给用户的校验串（SAS）
    pub sas_shown: Vec<(Side, u8, String)>,
}

impl Trace {
    pub fn msgs(&self, from: Side, controller: u8) -> impl Iterator<Item = &Sent> {
        self.sent
            .iter()
            .filter(move |s| s.from == from && s.controller == controller)
    }

    pub fn first(&self, f: impl Fn(&Sent) -> bool) -> Option<&Sent> {
        self.sent.iter().find(|s| f(s))
    }

    pub fn offer_epoch(&self, controller: u8) -> Option<u32> {
        self.msgs(Side::Host, controller).find_map(|s| match s.msg {
            Msg::DirectOffer { epoch } => Some(epoch),
            _ => None,
        })
    }

    pub fn spawns(&self, side: Side, controller: u8) -> Vec<Duration> {
        self.helper_spawns
            .iter()
            .filter(|h| h.side == side && h.controller == controller)
            .map(|h| h.at)
            .collect()
    }

    pub fn exits(&self, side: Side, controller: u8) -> Vec<Duration> {
        self.helper_exits
            .iter()
            .filter(|h| h.side == side && h.controller == controller)
            .map(|h| h.at)
            .collect()
    }

    pub fn session_ended(&self, controller: u8) -> bool {
        self.sessions_ended.iter().any(|(_, c)| *c == controller)
    }

    pub fn last_sender_path(&self, side: Side, controller: u8) -> Option<Path> {
        self.sender_paths
            .iter()
            .rev()
            .find(|p| p.side == side && p.controller == controller)
            .map(|p| p.path)
    }
}
