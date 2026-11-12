//! 直连会话的确定性场景驱动（虚拟时钟、假 helper 工厂、假 tailcat 路径与假中转），供 `tests/direct.rs` 使用。
//!
//! 场景只描述“发生了什么”（控制端连上、用户允许、某一侧闸门打开、helper 崩溃……），[`Trace`] 记录两端
//! 对外可观察的行为（各通道上发出的消息、helper 的启动 / 结束 / 网络活动、直连 QUIC 包、闸门、码率……）。
//!
//! [`Scenario::run`] 交给 [`Driver`]：Phase 1 把真实驱动接到 `airhand_link` 的直连实现上；在此之前（以及
//! Lenient / Strict 两个空操作作业里）用 [`Noop`] 驱动，它不实现任何直连行为，用例因断言不满足而红（先红），
//! 不靠“未实现”报错。
// 记录类型的部分字段 / 构造器只有个别用例用到。
#![allow(dead_code)]

use std::time::Duration;

use crate::support::{NOOP, Polarity};

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
    /// `[direct] ack_stall_ms`（O）
    pub ack_stall: Duration,
    /// `[direct] handshake_watchdog_ms`（H）
    pub handshake_watchdog: Duration,
    /// `[direct] upgrade_timeout_secs`
    pub upgrade_timeout: Duration,
    /// 被控端执行的重试上限（与控制端同一常量，OQ-27）
    pub retry_cap: u32,
    /// 重试间隔（与控制端同一常量）
    pub retry_interval: Duration,
    /// `[host] grant_memory_minutes`
    pub grant_memory_minutes: u32,
    /// `[host] max_pending_approvals`
    pub max_pending_approvals: u32,
    /// `[host] approval_timeout_secs`
    pub approval_timeout: Duration,
    /// 授予的会话是否带输入权限
    pub grant_input: bool,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            approve: Approve::Ask,
            direct_enabled: false,
            bitrate_cap_kbps: 100_000,
            rx_silence: Duration::from_millis(1000),
            write_timeout: Duration::from_millis(500),
            ack_stall: Duration::from_millis(1000),
            handshake_watchdog: Duration::from_millis(1500),
            upgrade_timeout: Duration::from_secs(10),
            retry_cap: 3,
            retry_interval: Duration::from_secs(30),
            grant_memory_minutes: 0,
            max_pending_approvals: 4,
            approval_timeout: Duration::from_secs(60),
            grant_input: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub direct_enabled: bool,
    /// `ControllerHello::device`
    pub device: [u8; 16],
    /// `ControllerHello` 里自报的设备名
    pub name: String,
    /// 注入给守规则控制端的升级结局（#144 / #148 夹具）
    pub plan: UpgradePlan,
}

/// 控制端每个 `epoch` 的升级结局（注入）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpgradePlan {
    /// 按场景里的事件自然推进
    #[default]
    Natural,
    /// 不运行控制端的升级状态机：`DirectRequest` 只由 [`Event::ScriptedDirectRequest`] 发出
    Scripted,
    /// 入口 ①：初次升级成功提交，随后注入路径故障；之后每次再升级都失败
    CommitThenFailRetries,
    /// 入口 ②：每个 `epoch` 都在闸门打开后被握手看门狗放弃
    AbandonAfterGateOpen,
    /// 控制端本地 5 ms 就判本次升级失败（被控端收到请求之前就放弃了上一个 `epoch`）
    LocalFailFast,
}

/// `DirectDecline` 的原因（枚举，与 `ByeReason` 的风格一致）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclineReason {
    /// 未取得直连同意（既有检查）
    NotGranted,
    /// 不认识的 `DirectOffer.kind`（既有）
    UnknownKind,
    /// 本会话的再升级次数已达上限（OQ-27）
    RetryLimit,
    /// 距上一个被受理请求的到达时刻不足重试间隔（OQ-27）
    RetryInterval,
}

/// 闸门关闭 / epoch 放弃的条件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCause {
    CurAddr,
    RxSilence,
    Ack,
    Handshake,
    Disco,
    Command,
    /// 提交等待超时
    CommitTimeout,
    /// 控制端闸门已开、等不到 `DirectGateOpen` 的升级超时
    GateOpenTimeout,
    /// 控制端闸门从未打开的升级超时
    UpgradeTimeout,
    /// 只有 DERP（路径始终不是 direct）
    DerpOnly,
}

/// 控制端出示的直连令牌
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenUse {
    Replay,
    WrongSession,
    Expired,
    WrongEpoch,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            direct_enabled: false,
            device: [1; 16],
            name: "controller".into(),
            plan: UpgradePlan::Natural,
        }
    }
}

/// 场景里发生的事。`controller` 是 [`Scenario::controller`] 加入的顺序（0 起）。
#[derive(Debug, Clone)]
pub enum Event {
    /// 控制端经中转连上被控端并发 `ControllerMsg::Hello`
    Connect {
        controller: u8,
        code: CodeChoice,
    },
    /// 被控端用户在确认窗点“允许”，`direct` 为“允许直连”勾选
    Approve {
        controller: u8,
        direct: bool,
    },
    /// 该侧的路径判定认为当前 `epoch` 已是 direct（被控端：`CurAddr` 非空；控制端：DiscoPing 走直连）
    GateOpen {
        side: Side,
        controller: u8,
    },
    /// 之后中转单向时延改为该值（拉长栅栏等待）
    RelayLatency(Duration),
    /// 之后控制端在直连上收不到 `Pong`
    StopPongs {
        controller: u8,
    },
    /// 该侧 helper 进程崩溃退出
    HelperCrash {
        side: Side,
        controller: u8,
    },
    /// 该侧 helper 往隧道的写入从此永久阻塞（假写路径，I25）
    HelperWriteStall {
        side: Side,
        controller: u8,
    },
    /// 直连路径失效（路径判定离开 direct）
    DirectPathFails {
        controller: u8,
    },
    /// 自适应码率请求的目标码率
    TargetBitrate {
        controller: u8,
        kbps: u32,
    },
    /// 被控端编码出一帧这么大的画面（例如关键帧）
    VideoFrame {
        controller: u8,
        bytes: usize,
    },
    /// 不按协议行事的控制端：不等 `Hello` 授予直连就发 `DirectRequest`
    ForceDirectRequest {
        controller: u8,
    },
    /// 经中转向控制端投递一条 `DirectGateOpen`，`epoch` 为当前升级的 `epoch` 加上 `epoch_delta`
    InjectDirectGateOpen {
        controller: u8,
        epoch_delta: i64,
    },
    /// 控制端用已经用过的直连令牌再连一次
    ReplayDirectToken {
        controller: u8,
    },
    /// 被控端用户“断开此控制端”（保留控制码）
    Kick {
        controller: u8,
    },
    /// 被控端用户“停止共享”（换码）
    StopSharing,
    /// 脚本化的控制端经中转发一个新 `epoch` 的 `DirectRequest`（不经控制端状态机，#146–#148）
    ScriptedDirectRequest {
        controller: u8,
    },
    /// 被控端用户收回 / 再授予该会话的直连权限
    RevokeDirect {
        controller: u8,
    },
    GrantDirect {
        controller: u8,
    },
    /// 收回输入 / 剪贴板权限
    RevokeInput {
        controller: u8,
    },
    RevokeClipboard {
        controller: u8,
    },
    /// 被控端进程重启（所有会话结束；之后的连接是新会话）
    HostRestart,
    /// 被控端用户“断开全部”
    KickAll,
    /// 被控端换码（不经“停止共享”界面，例如定时换码）
    NewCode,
    /// 控制端主动结束会话（`ControllerMsg::Bye`）
    ControllerBye {
        controller: u8,
    },
    /// 中转连接断开
    RelayDisconnect {
        controller: u8,
    },
    /// 从该侧发出的直连包从此全部丢失（单向断路）
    OneWayLoss {
        controller: u8,
        from: Side,
    },
    /// 双向完全丢包突发
    BurstLoss {
        controller: u8,
        length: Duration,
    },
    /// 该侧收到的 ACK 帧计数照常增长，但探测流不再被新确认（假信号源）
    AckFramesWithoutProgress {
        side: Side,
        controller: u8,
    },
    /// 直连 QUIC 握手从此卡住（不完成）
    HandshakeStall {
        controller: u8,
    },
    /// 该侧闸门因给定条件关闭（注入）
    GateClose {
        side: Side,
        controller: u8,
        cause: GateCause,
    },
    /// 被控端的 `DirectCommit` 丢失
    CommitLost {
        controller: u8,
    },
    /// 控制端发一串输入事件（序号连续）
    Input {
        controller: u8,
        count: u32,
    },
    /// 控制端按下一个键不放
    KeyDown {
        controller: u8,
    },
    /// 回退之后经直连迟到的输入
    LateDirectInput {
        controller: u8,
    },
    /// 直连流被对端 reset
    DirectStreamReset {
        controller: u8,
    },
    /// 控制端发 `DirectOffer.kind` 为该值的请求（被控端不认识的种类）
    OfferKind {
        controller: u8,
        kind: u8,
    },
    /// 直连 QUIC 证书指纹与 `quic_fp` 不符
    CertPinMismatch {
        controller: u8,
    },
    /// 控制端出示直连令牌
    PresentToken {
        controller: u8,
        token: TokenUse,
    },
    /// 停止共享之后迟到的直连连接
    LateDirectConnection {
        controller: u8,
    },
    /// 被控端超时放弃当前 `epoch` 之后，迟到的 `DirectRequest`
    LateDirectRequest {
        controller: u8,
    },
    /// 该侧 helper 往 stderr 写了这段文字
    HelperStderr {
        side: Side,
        controller: u8,
        text: String,
    },
    /// 该侧 helper 从此不再上报写进度心跳
    HelperProgressSilence {
        side: Side,
        controller: u8,
    },
    /// 两个方向各复制一条剪贴板
    Clipboard {
        controller: u8,
    },
    /// 被控端运营者改 `[host] approve`
    SetApprove(Approve),
    /// 中转单向时延的抖动幅度（均匀分布，固定种子）
    RelayJitter(Duration),
    /// 控制端从此不再在直连上发 `Ping`
    StopPings {
        controller: u8,
    },
    /// 控制端发 `DirectRequest`，其中的 `client_key` 为该值
    DirectRequestWithKey {
        controller: u8,
        key: String,
    },
    /// 控制端在 broker 上等被控端上线（持换码前取得的重连凭证时 `stale = true`）
    ResumeWait {
        controller: u8,
        stale: bool,
    },
    /// 同一条连接上连续登记 `count` 个等待项（各等一个不同的控制码）
    FillWaits {
        controller: u8,
        count: u32,
    },
    /// 被控端以当前控制码在 broker 上（重新）上线
    HostOnline,
    /// 控制端连接被控端，但用户一直不回应确认窗
    ConnectUnanswered {
        controller: u8,
    },
}

#[derive(Debug, Clone)]
pub struct Scenario {
    pub host: HostConfig,
    pub controllers: Vec<ControllerConfig>,
    pub screen: Screen,
    pub events: Vec<(Duration, Event)>,
    pub run_for: Duration,
    /// 直连路径的 RTT
    pub direct_rtt: Duration,
    /// 控制端→被控端方向的单程时延 d1（`None` = RTT 的一半）；被控端→控制端为 RTT − d1
    pub direct_up: Option<Duration>,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            host: HostConfig::default(),
            controllers: Vec::new(),
            screen: Screen::Static,
            events: Vec::new(),
            run_for: Duration::from_secs(30),
            direct_rtt: Duration::from_millis(20),
            direct_up: None,
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

    pub fn direct_rtt(mut self, ms: u64) -> Self {
        self.direct_rtt = Duration::from_millis(ms);
        self
    }

    pub fn direct_split(mut self, up_ms: u64) -> Self {
        self.direct_up = Some(Duration::from_millis(up_ms));
        self
    }

    pub fn at_dur(mut self, at: Duration, e: Event) -> Self {
        self.events.push((at, e));
        self
    }

    pub fn run(&self) -> Trace {
        driver().run(self)
    }
}

/// 场景驱动：按 [`Scenario`] 推进虚拟时钟并记录 [`Trace`]
pub trait Driver {
    fn run(&self, s: &Scenario) -> Trace;
}

/// 本次构建使用的驱动：打开空操作开关时是对应极性的 [`Noop`]；常规作业在 Phase 1 接线之前是严格极性的
/// [`Noop`]（什么也不发生），接线之后换成真实驱动。
fn driver() -> Box<dyn Driver> {
    Box::new(Noop(NOOP.unwrap_or(Polarity::Strict)))
}

/// 空操作驱动（规格 #98）：不发任何消息、不启动 helper、不升级。
/// * 宽松：场景里注入的 `GateOpen` 照记为闸门打开，之后永不关闭；脚本化的 `DirectRequest` 一律受理
///   （回 `DirectOffer`、从不拒绝）；
/// * 严格：闸门恒关，什么都不记录（计数全为 0）。
pub struct Noop(pub Polarity);

impl Driver for Noop {
    fn run(&self, s: &Scenario) -> Trace {
        let mut t = Trace::default();
        if self.0 == Polarity::Lenient {
            let mut epoch = 0;
            for (at, e) in &s.events {
                match e {
                    Event::GateOpen { side, controller } => t.gates.push(GateChange {
                        at: *at,
                        side: *side,
                        controller: *controller,
                        epoch: 0,
                        open: true,
                        cause: None,
                    }),
                    Event::ScriptedDirectRequest { controller } => {
                        epoch += 1;
                        t.sent.push(Sent {
                            at: *at,
                            delivered: Some(*at),
                            from: Side::Host,
                            controller: *controller,
                            path: Path::Relay,
                            msg: Msg::DirectOffer { epoch },
                        });
                    }
                    _ => {}
                }
            }
        }
        t
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
        reason: DeclineReason,
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
    /// 会话数已满 / 确认提示显示期间的拒绝（`Problem::HostBusy`；`ByeReason` 里暂无此变体）
    Busy,
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
    /// 关闭时由哪个条件触发（打开时为 `None`）
    pub cause: Option<GateCause>,
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
    /// 控制端收到的画面包（时刻、路径）
    pub frames: Vec<(Duration, u8, Path)>,
    /// 控制端收到的声音包
    pub audio: Vec<(Duration, u8, Path)>,
    /// 直连 QUIC keep-alive 帧出站时刻
    pub keep_alive_frames: Vec<(Duration, Side, u8)>,
    /// 栅栏等待（开始、结束）
    pub fence_waits: Vec<(u8, Duration, Duration)>,
    /// 被控端收到并注入的输入事件（序号、路径）
    pub inputs: Vec<(Duration, u8, u32, Path)>,
    /// 被丢弃的迟到直连输入个数
    pub late_inputs_dropped: usize,
    /// 被控端按键状态：按下 / `release_all()`
    pub keys_down: Vec<(Duration, u8)>,
    pub release_all: Vec<(Duration, u8)>,
    /// 中转媒体流字节计数采样（时刻、累计字节）
    pub relay_media_bytes: Vec<(Duration, u8, u64)>,
    /// 被控端新建 tailcat `Server` 的时刻
    pub server_news: Vec<(Duration, u8)>,
    /// 向用户显示的错误提示
    pub error_prompts: Vec<(Duration, Side, u8)>,
    /// 被放弃的 `epoch`（时刻、控制端、epoch、原因）
    pub abandoned: Vec<(Duration, u8, u32, GateCause)>,
    /// 进入重试期的会话与每次计数后的累计值
    pub retry_counts: Vec<(Duration, u8, u32)>,
    /// helper 的写进度心跳（时刻、侧、控制端）
    pub progress_heartbeats: Vec<(Duration, Side, u8)>,
    /// 控制端 / 被控端 O 的进展事件（探测流被新确认）
    pub ack_progress: Vec<(Duration, Side, u8)>,
    /// 首条探测流发出时刻
    pub first_probe: Vec<(Duration, Side, u8)>,
    /// 握手完成命令送达 helper 的时刻
    pub handshake_complete: Vec<(Duration, Side, u8)>,
    /// H 被撤销的时刻
    pub watchdog_disarmed: Vec<(Duration, Side, u8)>,
    /// 确认窗显示的文字
    pub prompt_texts: Vec<(u8, String)>,
    /// 被控端看到的设备标识（原始字节）
    pub device_ids: Vec<(u8, Vec<u8>)>,
    /// 界面上显示过的文字
    pub ui_texts: Vec<String>,
    /// 剪贴板（方向、控制端）：被控端→控制端为 `Side::Host`
    pub clipboard: Vec<(Duration, Side, u8)>,
    /// 生效的 `[host] approve`（时刻、值）
    pub approve_applied: Vec<(Duration, Approve)>,
    /// 控制端发出的 `DirectRequest`（含脚本化控制端）
    pub requests: Vec<(Duration, u8, u32)>,
    /// broker 向等待者发出的上线通知
    pub online_notices: Vec<(Duration, u8)>,
    /// broker 上每条连接的等待项个数（采样）
    pub waiters: Vec<(Duration, u8, usize)>,
    /// 计入“同源拒绝 / 无应答”的静默拒绝次数
    pub silent_declines: usize,
    /// 被控端 `Server.Close()` 的时刻
    pub server_closes: Vec<(Duration, u8)>,
    /// 直连上收到的数据报（时刻、接收侧、控制端），用于入站计数
    pub inbound: Vec<(Duration, Side, u8)>,
    /// I25 结束并重启 helper 的时刻
    pub i25_restarts: Vec<(Duration, Side, u8)>,
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

    pub fn declines(&self, controller: u8) -> Vec<(Duration, DeclineReason)> {
        self.msgs(Side::Host, controller)
            .filter_map(|s| match s.msg {
                Msg::DirectDecline { reason, .. } => Some((s.at, reason)),
                _ => None,
            })
            .collect()
    }

    pub fn offers(&self, controller: u8) -> Vec<(Duration, u32)> {
        self.msgs(Side::Host, controller)
            .filter_map(|s| match s.msg {
                Msg::DirectOffer { epoch } => Some((s.at, epoch)),
                _ => None,
            })
            .collect()
    }

    /// 任一时刻同时存活的该侧 helper 进程数的最大值
    pub fn max_live_helpers(&self, side: Side, controller: u8) -> usize {
        let mut ev: Vec<(Duration, i32)> = self
            .spawns(side, controller)
            .into_iter()
            .map(|a| (a, 1))
            .chain(self.exits(side, controller).into_iter().map(|a| (a, -1)))
            .collect();
        ev.sort();
        let (mut cur, mut max) = (0i32, 0i32);
        for (_, d) in ev {
            cur += d;
            max = max.max(cur);
        }
        max as usize
    }

    pub fn derp_contacts(&self, side: Side, controller: u8) -> Vec<Duration> {
        self.network
            .iter()
            .filter(|n| n.side == side && n.controller == controller && n.what == "derp-connect")
            .map(|n| n.at)
            .collect()
    }

    pub fn last_sender_path(&self, side: Side, controller: u8) -> Option<Path> {
        self.sender_paths
            .iter()
            .rev()
            .find(|p| p.side == side && p.controller == controller)
            .map(|p| p.path)
    }
}
