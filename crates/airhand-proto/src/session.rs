//! 会话流（被控端 ⇄ 控制端，经服务器透传）消息。

use serde::{Deserialize, Serialize};

/// 被控端 → 控制端
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum HostMsg {
    Hello(HostHello),
    Video(VideoFrame),
    /// 回应 [`ControllerMsg::Ping`]，用于测量端到端往返时延。
    Pong {
        t: u64,
    },
    /// 显示器列表或当前显示器变化。
    Displays {
        displays: Vec<DisplayInfo>,
        current: u8,
    },
    /// 给控制端的提示（例如采集失败）。
    Notice(Notice),
    /// 编码器（重新）创建后通知控制端当前实际使用的编码参数。
    Encoder(EncoderInfo),
    /// 远端光标形状/可见性变化。控制端用本地光标显示该形状（移动零延迟）。
    Cursor(CursorState),
    /// 被控端剪贴板文本变化
    Clipboard {
        text: String,
    },
    /// 被控端主动结束会话（例如用户在托盘点了"断开所有控制端"），控制端不应自动重连。
    Bye(ByeReason),
    /// 被控端正在播放的声音（控制端用 [`ControllerMsg::Audio`] 开启后才会发送）。
    Audio(AudioFrame),
}

/// 被控端给控制端的提示。
///
/// 发送的是类型而不是文字：控制端按**自己的**界面语言显示，不会出现对方电脑的语言。
/// 会自动恢复的问题成对出现（`XxxFailed` / `XxxRecovered`）：恢复时控制端撤下之前的提示，`XxxRecovered` 本身不显示。
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// 被控端没有可以采集的显示器
    NoDisplays,
    /// 被控端是 Wayland 桌面：只能采集和操作 X11（XWayland）程序
    Wayland,
    /// 屏幕采集失败（被控端会自动重试）。`detail` 是给排查问题用的技术信息：英文，
    /// 不含操作系统按本地语言给出的错误说明（Windows 只带 HRESULT 错误码）。
    /// 缺少系统权限不算采集失败，用 [`Notice::MissingPermissions`] 说明。
    CaptureFailed { detail: String },
    /// 屏幕采集恢复正常（之前发过 [`Notice::CaptureFailed`]）。
    CaptureRecovered,
    /// 无法采集声音（被控端会自动重试），只发给开着声音的控制端。`detail` 同 [`Notice::CaptureFailed`]。
    /// 缺少系统权限时不发（已经用 [`Notice::MissingPermissions`] 说明）。
    AudioFailed { detail: String },
    /// 声音采集恢复正常（之前发过 [`Notice::AudioFailed`]）。
    AudioRecovered,
    /// 被控端缺少系统权限（目前只有 macOS），只能由那台电脑的用户在"系统设置 → 隐私与安全性"里授予：
    /// `screen` 是"屏幕与系统录音"（缺了就没有画面和声音），`input` 是"辅助功能"（缺了就不能操作键盘鼠标）。
    ///
    /// 会话开始时缺少权限就发一次，之后权限有变化再发；两项都为 false 表示已经全部授予，
    /// 控制端据此撤下之前的提示。
    MissingPermissions { screen: bool, input: bool },
}

/// 被控端结束会话的原因
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByeReason {
    /// 被控端的用户断开了所有控制端
    Kicked,
    /// 被控端的用户更换了控制码（所有控制端随之断开）
    CodeChanged,
}

/// 声音格式固定为 Opus 48 kHz 立体声，每包 20 ms。
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: u8 = 2;
pub const AUDIO_FRAME_MS: u32 = 20;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AudioFrame {
    /// 包序号。没有声音时被控端不发包，序号也不递增。
    pub seq: u32,
    /// 静音之后（或刚开始收听时）的第一包。控制端据此区分"声音停了"和"网络抖动"：
    /// 前者不需要加大缓冲。
    pub start: bool,
    /// 这一包解码后第一个采样的采集时间（被控端时钟，微秒），与 [`VideoFrame::capture_us`]
    /// 是同一个时钟，控制端据此做音画同步。已扣除 Opus 编码器的前瞻延迟。
    pub capture_us: u64,
    /// 一个 Opus 包
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CursorState {
    pub visible: bool,
    /// `None` 表示形状未变化
    pub shape: Option<CursorShape>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CursorShape {
    pub width: u16,
    pub height: u16,
    pub hot_x: u16,
    pub hot_y: u16,
    /// RGBA8，非预乘
    #[serde(with = "serde_bytes")]
    pub rgba: Vec<u8>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EncoderInfo {
    pub codec: Codec,
    /// 例如 "NVIDIA H.264 Encoder MFT" / "OpenH264"
    pub name: String,
    pub hardware: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct HostHello {
    pub hostname: String,
    pub os: String,
    pub version: String,
    pub displays: Vec<DisplayInfo>,
    pub current_display: u8,
    /// 被控端可用的编码格式（硬件或软件）
    pub codecs: Vec<Codec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub index: u8,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// 视频编码格式。控制端统一使用 WebCodecs（GPU 硬件解码）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Codec {
    /// H.264 Annex-B：兼容性最好，支持软件编码兜底
    #[default]
    H264,
    /// H.265/HEVC Annex-B：同画质约节省 30~40% 带宽（需要硬件编码器）
    H265,
    /// AV1 OBU：同画质约节省 40~50% 带宽（需要较新的 GPU）
    AV1,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VideoFrame {
    /// 帧序号，控制端需对每一帧回 [`ControllerMsg::Ack`]。
    pub seq: u32,
    pub key: bool,
    pub codec: Codec,
    pub width: u16,
    pub height: u16,
    /// 画面呈现的时间（被控端时钟，微秒），与 [`AudioFrame::capture_us`] 是同一个时钟，用于音画同步。
    pub capture_us: u64,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

/// 控制端 → 被控端
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ControllerMsg {
    /// 批量输入事件（鼠标移动会被合并）
    Input(Vec<InputEvent>),
    /// 确认收到某一帧，用于被控端的在途帧控制（防止延迟堆积）。
    Ack {
        seq: u32,
    },
    Settings(QualitySettings),
    RequestKeyframe,
    SelectDisplay {
        index: u8,
    },
    Ping {
        t: u64,
    },
    /// 控制端剪贴板文本变化
    Clipboard {
        text: String,
    },
    /// 开启/关闭被控端声音（默认关闭）
    Audio {
        enabled: bool,
    },
}

/// 剪贴板同步的文本上限
pub const MAX_CLIPBOARD_TEXT: usize = 1 << 20;

/// 画质设置（由控制端 UI 选择，被控端会做范围校验）。
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct QualitySettings {
    /// 最大帧率 1..=120
    pub fps: u8,
    /// 目标码率 kbps
    pub bitrate_kbps: u32,
    /// 输出画面短边上限（如 720/1080），0 表示不限制
    pub max_short: u16,
    /// 根据网络状况自动下调/恢复码率
    pub adaptive: bool,
    /// 期望的编码格式（被控端不支持时自动回退到 H.264）
    pub codec: Codec,
}

impl Default for QualitySettings {
    fn default() -> Self {
        Self {
            fps: 30,
            bitrate_kbps: 4_000,
            max_short: 1080,
            adaptive: true,
            codec: Codec::H264,
        }
    }
}

impl QualitySettings {
    pub fn sanitized(mut self) -> Self {
        self.fps = self.fps.clamp(1, 120);
        self.bitrate_kbps = self.bitrate_kbps.clamp(100, 100_000);
        self
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// 归一化坐标（0..=1），相对当前显示器画面
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
    },
    /// 滚轮，120 = 一格
    Wheel {
        dx: i16,
        dy: i16,
    },
    /// 物理按键，取值为 W3C `KeyboardEvent.code`（如 `KeyA`、`ControlLeft`）
    Key {
        code: String,
        down: bool,
    },
    /// 直接输入一段 Unicode 文本
    Text {
        text: String,
    },
    /// 释放所有按下的键（控制端失焦时发送，防止按键粘连）
    ReleaseAll,
}
