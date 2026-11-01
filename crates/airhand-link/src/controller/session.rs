//! 控制端的单个会话：接收画面转交给控制台（WebCodecs 硬件解码），转发输入与设置。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use airhand_proto::MAX_SESSION_MSG;
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::session::{
    AudioFrame, Codec, ControllerMsg, CursorState, DisplayInfo, HostMsg, Notice, VideoFrame,
};
use anyhow::Result;
use quinn::{RecvStream, SendStream};
use serde::Serialize;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::{CLIPBOARD_POLL, ClipboardSync};
use crate::Problem;

static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_us() -> u64 {
    EPOCH.elapsed().as_micros() as u64
}

/// 会话的画面和事件交给谁：桌面版是 Tauri 的 Channel，网页版是浏览器的 WebSocket。
///
/// 返回 `Err` 表示控制台已经不在了（窗口关了、浏览器断开了），会话随之结束。
pub trait SessionUi: Send + Sync + 'static {
    /// 画面 / 光标 / 声音数据包，格式见 [`HEADER_LEN`]
    fn packet(&self, packet: Vec<u8>) -> Result<(), UiClosed>;
    /// 会话事件（控制台那边是 JSON）
    fn event(&self, event: SessionEvent) -> Result<(), UiClosed>;
}

/// 控制台已经关闭
#[derive(Debug, Clone, Copy)]
pub struct UiClosed;

/// 发往控制台的会话事件（JSON）。
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionEvent {
    Hello {
        hostname: String,
        os: String,
        version: String,
        displays: Vec<DisplayInfo>,
        current: u8,
        codecs: Vec<Codec>,
    },
    Encoder {
        codec: Codec,
        name: String,
        hardware: bool,
        width: u32,
        height: u32,
    },
    Displays {
        displays: Vec<DisplayInfo>,
        current: u8,
    },
    Stats {
        fps: f32,
        kbps: f32,
        rtt_ms: Option<f32>,
        width: u16,
        height: u16,
        frames: u64,
    },
    /// 被控端发来的提示，由控制台按自己的界面语言显示
    Notice { notice: NoticeView },
    /// 被控端复制了文本（只发给浏览器，见 [`super::Frontend::Browser`]；桌面版直接写入本机剪贴板）
    Clipboard { text: String },
    /// `retry=false` 表示对方主动断开，不应自动重连
    Closed { reason: Problem, retry: bool },
}

/// [`Notice`] 发给控制台时的 JSON 形状：`{ "kind": "captureFailed", "detail": "..." }`
///（协议里的 `Notice` 用 postcard 编码，不能直接加 serde 的 `tag`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NoticeView {
    NoDisplays,
    Wayland,
    CaptureFailed {
        detail: String,
    },
    /// 不显示：撤下之前的 `captureFailed`
    CaptureRecovered,
    AudioFailed {
        detail: String,
    },
    /// 不显示：撤下之前的 `audioFailed`
    AudioRecovered,
    /// 两项都为 false 表示权限已经全部授予，控制台撤下之前的权限提示
    MissingPermissions {
        screen: bool,
        input: bool,
    },
}

impl From<Notice> for NoticeView {
    fn from(n: Notice) -> Self {
        match n {
            Notice::NoDisplays => NoticeView::NoDisplays,
            Notice::Wayland => NoticeView::Wayland,
            Notice::CaptureFailed { detail } => NoticeView::CaptureFailed { detail },
            Notice::CaptureRecovered => NoticeView::CaptureRecovered,
            Notice::AudioFailed { detail } => NoticeView::AudioFailed { detail },
            Notice::AudioRecovered => NoticeView::AudioRecovered,
            Notice::MissingPermissions { screen, input } => {
                NoticeView::MissingPermissions { screen, input }
            }
        }
    }
}

/// 二进制通道数据包，24 字节头（小端）。前 16 字节按类型区分，`[16..24]` 是采集时间 `capture_us u64`
///（被控端时钟；视频与声音是同一个时钟，控制端据此做音画同步；光标为 0）：
///
/// * 视频 `kind=1`：`kind u8 | key u8 | codec u16 | seq u32 | width u16 | height u16 | reserved u32 | capture_us | 码流…`
/// * 光标 `kind=2`：`kind u8 | visible u8 | has_shape u16 | reserved u32 | width u16 | height u16 | hot_x u16 | hot_y u16 | 0 | RGBA…`
/// * 声音 `kind=3`：`kind u8 | start u8 | reserved u16 | seq u32 | reserved[8] | capture_us | Opus 包`
pub const HEADER_LEN: usize = 24;

fn audio_packet(a: &AudioFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + a.data.len());
    buf.push(3);
    buf.push(a.start as u8);
    buf.extend_from_slice(&[0; 2]);
    buf.extend_from_slice(&a.seq.to_le_bytes());
    buf.extend_from_slice(&[0; 8]);
    buf.extend_from_slice(&a.capture_us.to_le_bytes());
    buf.extend_from_slice(&a.data);
    buf
}

fn cursor_packet(c: &CursorState) -> Vec<u8> {
    let shape_len = c.shape.as_ref().map_or(0, |s| s.rgba.len());
    let mut buf = Vec::with_capacity(HEADER_LEN + shape_len);
    buf.push(2);
    buf.push(c.visible as u8);
    buf.extend_from_slice(&(c.shape.is_some() as u16).to_le_bytes());
    buf.extend_from_slice(&[0; 4]);
    match &c.shape {
        Some(s) => {
            for v in [s.width, s.height, s.hot_x, s.hot_y] {
                buf.extend_from_slice(&v.to_le_bytes());
            }
        }
        None => buf.extend_from_slice(&[0; 8]),
    }
    buf.extend_from_slice(&[0; 8]);
    if let Some(s) = &c.shape {
        buf.extend_from_slice(&s.rgba);
    }
    buf
}

fn video_packet(v: &VideoFrame) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + v.data.len());
    buf.push(1);
    buf.push(v.key as u8);
    buf.extend_from_slice(
        &(match v.codec {
            Codec::H264 => 0u16,
            Codec::H265 => 1,
            Codec::AV1 => 2,
        })
        .to_le_bytes(),
    );
    buf.extend_from_slice(&v.seq.to_le_bytes());
    buf.extend_from_slice(&v.width.to_le_bytes());
    buf.extend_from_slice(&v.height.to_le_bytes());
    buf.extend_from_slice(&[0; 4]);
    buf.extend_from_slice(&v.capture_us.to_le_bytes());
    buf.extend_from_slice(&v.data);
    buf
}

struct Stats {
    frames: u32,
    bytes: usize,
    total: u64,
    w: u16,
    h: u16,
    rtt: Option<f32>,
    last_msg: Instant,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            frames: 0,
            bytes: 0,
            total: 0,
            w: 0,
            h: 0,
            rtt: None,
            last_msg: Instant::now(),
        }
    }
}

pub(super) struct SessionIo {
    pub send: SendStream,
    pub recv: RecvStream,
    pub ui: Arc<dyn SessionUi>,
    pub mode: Mode,
}

/// 见 [`super::Frontend`]
pub(super) enum Mode {
    Desktop {
        /// 远端→本机 与 本机→远端 共用同一个同步状态，用来抑制回声
        clip: Arc<tokio::sync::Mutex<Box<dyn ClipboardSync>>>,
        /// 是否同步剪贴板（界面可随时切换）
        enabled: Arc<AtomicBool>,
    },
    Browser,
}

pub(super) async fn run(
    sid: u32,
    io: SessionIo,
    tx: mpsc::UnboundedSender<ControllerMsg>,
    mut rx: mpsc::UnboundedReceiver<ControllerMsg>,
) {
    let SessionIo {
        mut send,
        mut recv,
        ui,
        mode,
    } = io;
    let local_clip = match mode {
        Mode::Desktop { clip, enabled } => Some((clip, enabled)),
        Mode::Browser => None,
    };
    // 桌面版收到画面立即确认；浏览器自己确认（见 Frontend::Browser）
    let ack_here = local_clip.is_some();

    let writer = async move {
        while let Some(msg) = rx.recv().await {
            write_msg(&mut send, &msg).await?;
        }
        let _ = send.finish();
        anyhow::Ok(())
    };

    // 统计数据由读循环写入、定时器读取（读循环本身绝不能被 select 取消，否则会破坏帧边界）
    let stats = std::sync::Arc::new(std::sync::Mutex::new(Stats::default()));

    let out = ui.clone();
    let st = stats.clone();
    let ack_tx = tx.clone();
    let clip_in = local_clip.clone();
    let reader = async move {
        let mut buf = Vec::with_capacity(256 * 1024);
        loop {
            let Some(msg) = read_msg::<_, HostMsg>(&mut recv, MAX_SESSION_MSG, &mut buf).await?
            else {
                return Err(Problem::HostEnded.into());
            };
            st.lock().unwrap().last_msg = Instant::now();
            match msg {
                HostMsg::Video(v) => {
                    // 收到即确认，被控端据此控制在途帧数量
                    if ack_here {
                        let _ = ack_tx.send(ControllerMsg::Ack { seq: v.seq });
                    }
                    {
                        let mut s = st.lock().unwrap();
                        s.frames += 1;
                        s.total += 1;
                        s.bytes += v.data.len();
                        s.w = v.width;
                        s.h = v.height;
                    }
                    out.packet(video_packet(&v)).map_err(|_| ui_closed())?;
                }
                HostMsg::Hello(h) => {
                    let _ = out.event(SessionEvent::Hello {
                        hostname: h.hostname,
                        os: h.os,
                        version: h.version,
                        displays: h.displays,
                        current: h.current_display,
                        codecs: h.codecs,
                    });
                }
                HostMsg::Displays { displays, current } => {
                    let _ = out.event(SessionEvent::Displays { displays, current });
                }
                HostMsg::Pong { t } => {
                    let sample = now_us().saturating_sub(t) as f32 / 1000.0;
                    let mut s = st.lock().unwrap();
                    s.rtt = Some(match s.rtt {
                        Some(r) => r * 0.7 + sample * 0.3,
                        None => sample,
                    });
                }
                HostMsg::Notice(n) => {
                    let _ = out.event(SessionEvent::Notice { notice: n.into() });
                }
                HostMsg::Clipboard { text } => match &clip_in {
                    Some((clip, enabled)) => {
                        if enabled.load(Ordering::Relaxed) {
                            clip.lock().await.apply(text);
                        }
                    }
                    None => {
                        let _ = out.event(SessionEvent::Clipboard { text });
                    }
                },
                HostMsg::Bye(reason) => return Err(Problem::from(reason).into()),
                HostMsg::Cursor(c) => {
                    out.packet(cursor_packet(&c)).map_err(|_| ui_closed())?;
                }
                HostMsg::Audio(a) => {
                    st.lock().unwrap().bytes += a.data.len();
                    out.packet(audio_packet(&a)).map_err(|_| ui_closed())?;
                }
                HostMsg::Encoder(e) => {
                    let _ = out.event(SessionEvent::Encoder {
                        codec: e.codec,
                        name: e.name,
                        hardware: e.hardware,
                        width: e.width,
                        height: e.height,
                    });
                }
            }
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    };

    let out = ui.clone();
    let ping_tx = tx.clone();
    let ticker = async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last = Instant::now();
        loop {
            tick.tick().await;
            let dt = last.elapsed().as_secs_f32().max(0.001);
            last = Instant::now();
            let event = {
                let mut s = stats.lock().unwrap();
                if s.last_msg.elapsed() > Duration::from_secs(20) {
                    return Err(Problem::HostUnresponsive.into());
                }
                let e = SessionEvent::Stats {
                    fps: s.frames as f32 / dt,
                    kbps: s.bytes as f32 * 8.0 / 1000.0 / dt,
                    rtt_ms: s.rtt,
                    width: s.w,
                    height: s.h,
                    frames: s.total,
                };
                s.frames = 0;
                s.bytes = 0;
                e
            };
            let _ = out.event(event);
            // 也是保活：被控端很久收不到控制端的消息就会结束会话
            let _ = ping_tx.send(ControllerMsg::Ping { t: now_us() });
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    };

    let clipper = async move {
        let Some((clip, enabled)) = local_clip else {
            // 网页版：浏览器自己读剪贴板
            return std::future::pending().await;
        };
        let mut tick = tokio::time::interval(CLIPBOARD_POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let mut c = clip.lock().await;
            // 关闭同步时也要消费变化，否则重新打开时会把旧内容发过去
            let Some(text) = c.poll().await else { continue };
            if enabled.load(Ordering::Relaxed)
                && tx.send(ControllerMsg::Clipboard { text }).is_err()
            {
                break;
            }
        }
        anyhow::Ok(())
    };

    let result: Result<()> = tokio::select! {
        r = reader => r,
        r = ticker => r,
        r = clipper => r,
        r = writer => r.and(Err(Problem::SessionEnded.into())),
    };
    let reason = match result {
        Ok(()) => Problem::SessionEnded,
        Err(e) => Problem::from_error(e, Problem::failed),
    };
    // 对方有意结束（断开所有控制端 / 更换控制码）时不自动重连
    let retry = !reason.is_final();
    info!(sid, retry, "session ended: {reason}");
    if ui.event(SessionEvent::Closed { reason, retry }).is_err() {
        warn!(sid, "couldn't tell the console that the session ended");
    }
}

/// 控制台已经关了（窗口关闭 / 浏览器断开），会话也就没必要继续了
fn ui_closed() -> anyhow::Error {
    anyhow::anyhow!("the console was closed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use airhand_proto::session::CursorShape;

    #[test]
    fn packet_layout() {
        let capture_us = 0x0102_0304_0506_0708u64;
        let v = VideoFrame {
            seq: 9,
            key: true,
            codec: Codec::AV1,
            width: 1920,
            height: 1080,
            capture_us,
            data: vec![7; 5],
        };
        let p = video_packet(&v);
        assert_eq!(&p[..4], &[1, 1, 2, 0]);
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 9);
        assert_eq!(
            u64::from_le_bytes(p[16..24].try_into().unwrap()),
            capture_us
        );
        assert_eq!(&p[HEADER_LEN..], &[7; 5]);

        let a = AudioFrame {
            seq: 3,
            start: true,
            capture_us,
            data: vec![1, 2, 3],
        };
        let p = audio_packet(&a);
        assert_eq!(&p[..2], &[3, 1]);
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 3);
        assert_eq!(
            u64::from_le_bytes(p[16..24].try_into().unwrap()),
            capture_us
        );
        assert_eq!(&p[HEADER_LEN..], &[1, 2, 3]);

        let shape = CursorShape {
            width: 2,
            height: 1,
            hot_x: 1,
            hot_y: 0,
            rgba: vec![9; 8],
        };
        let p = cursor_packet(&CursorState {
            visible: true,
            shape: Some(shape),
        });
        assert_eq!(p.len(), HEADER_LEN + 8);
        assert_eq!(u16::from_le_bytes(p[8..10].try_into().unwrap()), 2);
        assert_eq!(&p[HEADER_LEN..], &[9; 8]);
        assert_eq!(
            cursor_packet(&CursorState {
                visible: false,
                shape: None
            })
            .len(),
            HEADER_LEN
        );
    }

    /// 控制台（client/src/lib/api.ts 的 SessionEvent / Notice 类型）依赖这个 JSON 形状
    #[test]
    fn event_json_shape() {
        use serde_json::json;
        let n = SessionEvent::Notice {
            notice: Notice::CaptureFailed {
                detail: "0x887A0026".into(),
            }
            .into(),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({ "type": "notice", "notice": { "kind": "captureFailed", "detail": "0x887A0026" } })
        );
        let n = SessionEvent::Notice {
            notice: Notice::Wayland.into(),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({ "type": "notice", "notice": { "kind": "wayland" } })
        );
        let n = SessionEvent::Notice {
            notice: Notice::MissingPermissions {
                screen: false,
                input: true,
            }
            .into(),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({ "type": "notice", "notice": { "kind": "missingPermissions", "screen": false, "input": true } })
        );
        let n = SessionEvent::Notice {
            notice: Notice::AudioRecovered.into(),
        };
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({ "type": "notice", "notice": { "kind": "audioRecovered" } })
        );
        let c = SessionEvent::Closed {
            reason: Problem::from(airhand_proto::session::ByeReason::Kicked),
            retry: false,
        };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({ "type": "closed", "reason": { "kind": "kicked" }, "retry": false })
        );
        let c = SessionEvent::Clipboard { text: "hi".into() };
        assert_eq!(
            serde_json::to_value(&c).unwrap(),
            json!({ "type": "clipboard", "text": "hi" })
        );
    }
}
