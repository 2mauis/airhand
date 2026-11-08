//! 被控端（手机被别人控制）：保持与服务器的连接（自动重连、取回原来的控制码），为每个控制端请求启动会话。
//!
//! 和桌面版（`client/src-tauri/src/host/`）的分工不同：屏幕采集、编码、声音、注入输入都在 Kotlin 那边
//!（MediaProjection、MediaCodec、AccessibilityService），这里只管网络：
//!
//! * Kotlin → Rust：编码好的画面 / 声音（[`Host::video`] / [`Host::audio`]），状态变化（[`Op`]，JSON）；
//! * Rust → Kotlin：事件队列（JSON，见 [`Host::poll_event`]）：与服务器的连接状态、分配到的控制码、
//!   正在控制的设备、需不需要画面 / 声音、控制端发来的输入和剪贴板；
//! * 画面的节奏由 [`video::VideoHub`] 控制（Kotlin 的渲染线程每一轮调用 `tick` 和 `wait_window`）。
//!
//! 控制码像 Windows 安装版一样固定（`Register::keep`）：Kotlin 把服务器分配的控制码和密钥保存在
//! 应用自己的数据目录里，下次启动时带上（[`HostConfig::reclaim`]）取回同一个控制码。

pub mod audio;
mod session;
pub mod video;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_link::net::{self, Reconnect, Report};
use airhand_link::{LinkConfig, ServerState};
use airhand_proto::framing::{self, read_msg, write_msg};
use airhand_proto::session::{
    ByeReason, Codec, DisplayInfo, EncoderInfo, HostMsg, Notice, SystemAction,
};
use airhand_proto::signal::{ClientMsg, Reclaim, ServerMsg};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{info, warn};

use crate::ctl::LinkSettings;
use crate::queue::Queue;
use crate::rt::rt;
use crate::{hex, unhex};

/// Kotlin 传来的启动配置
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    #[serde(flatten)]
    pub link: LinkSettings,
    /// 应用版本，告诉控制端
    pub version: String,
    /// 上次分配到的控制码和密钥（32 位十六进制），用来取回同一个控制码
    pub reclaim: Option<SavedCode>,
    pub displays: Vec<DisplayInfo>,
    /// 手机能编码的格式
    pub codecs: Vec<Codec>,
    /// 能不能操作（无障碍服务开着没有）
    pub input: bool,
    /// 能接收多点触摸（Android 8+ 的连续手势）
    #[serde(default)]
    pub touch: bool,
    /// 能执行的系统操作（返回、主页……，随系统版本不同）
    #[serde(default)]
    pub actions: Vec<SystemAction>,
    /// 有人连接时先请本机用户同意
    #[serde(default)]
    pub approval: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SavedCode {
    pub code: u32,
    pub secret: String,
}

/// Kotlin 发来的状态变化 / 命令
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Op {
    /// 断开所有控制端
    KickAll,
    /// 断开所有控制端并更换控制码
    NewCode,
    /// 屏幕尺寸变了（旋转）
    Displays { displays: Vec<DisplayInfo> },
    /// 编码器（重新）创建了
    Encoder { info: EncoderInfo },
    /// 无障碍服务开关变了
    Permissions { input: bool },
    /// 屏幕采集失败（`None` 表示恢复了）
    Capture { failure: Option<String> },
    /// 声音采集失败（`None` 表示恢复了）
    Audio { failure: Option<String> },
    /// 手机的剪贴板有新文本（Android 10 起应用在后台读不到剪贴板，只有前台时才会有）
    Clipboard { text: String },
    /// 本机用户回答了连接请求（`id` 是 controllers 事件里的 id）
    Approve { id: String, allow: bool },
    /// 打开 / 关闭"有人连接时先请我同意"
    Approval { required: bool },
}

/// 托盘那边的"命令"
#[derive(Debug, Clone, Copy)]
enum HostCmd {
    KickAll,
    NewCode,
}

/// 会话开始时告诉控制端的信息，以及之后的变化
pub(crate) struct Info {
    pub hostname: String,
    pub os: String,
    pub version: String,
    pub displays: Vec<DisplayInfo>,
    pub codecs: Vec<Codec>,
    pub encoder: Option<EncoderInfo>,
    pub input: bool,
    pub touch: bool,
    pub actions: Vec<SystemAction>,
    pub capture_failure: Option<String>,
}

/// 会话之间共享的东西
pub(crate) struct Shared {
    pub info: Mutex<Info>,
    pub events: Arc<Queue<String>>,
    pub video: video::VideoHub,
    pub audio: audio::AudioHub,
    /// 发给所有会话的控制消息（显示器变化、编码器信息、提示……），已编码
    pub broadcast: broadcast::Sender<Bytes>,
    controllers: Mutex<Vec<ControllerView>>,
    /// 有人连接时先请本机用户同意
    approval: AtomicBool,
    /// 正在问本机用户的会话：回答从这里交给会话
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    /// 这次共享里本机用户同意过的控制端（`ControllerHello::device`）：断线重连时不再问
    trusted: Mutex<HashSet<[u8; 16]>>,
}

fn as_string<S: serde::Serializer>(v: &u64, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.collect_str(v)
}

/// 告诉 Kotlin 的控制端列表里的一项
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ControllerView {
    /// 会话 id。是随机的 u64，转成字符串（Kotlin 的 JSON 解析会把超过 Long 的数字变成浮点数）
    #[serde(serialize_with = "as_string")]
    id: u64,
    name: String,
    ip: String,
    /// 在等本机用户同意
    pub pending: bool,
    /// 已经开始（看得到画面）
    pub active: bool,
}

impl Shared {
    pub fn event(&self, v: serde_json::Value) {
        self.events.push(v.to_string());
    }

    fn broadcast(&self, msg: &HostMsg) {
        if let Ok(b) = framing::encode(msg) {
            let _ = self.broadcast.send(Bytes::from(b));
        }
    }

    fn report_controllers(&self) {
        let list = self.controllers.lock().unwrap().clone();
        self.event(json!({ "type": "controllers", "list": list }));
    }

    pub fn update_controller(&self, id: u64, f: impl FnOnce(&mut ControllerView)) {
        let mut list = self.controllers.lock().unwrap();
        if let Some(c) = list.iter_mut().find(|c| c.id == id) {
            f(c);
            drop(list);
            self.report_controllers();
        }
    }

    pub fn approval_required(&self) -> bool {
        self.approval.load(Ordering::Relaxed)
    }

    pub fn add_pending(&self, id: u64, tx: oneshot::Sender<bool>) {
        self.pending.lock().unwrap().insert(id, tx);
    }

    /// 会话不再等回答了（已经回答、超时或者断开）
    pub fn take_pending(&self, id: u64) -> Option<oneshot::Sender<bool>> {
        self.pending.lock().unwrap().remove(&id)
    }

    pub fn is_trusted(&self, device: &[u8; 16]) -> bool {
        self.trusted.lock().unwrap().contains(device)
    }

    pub fn trust(&self, device: [u8; 16]) {
        self.trusted.lock().unwrap().insert(device);
    }
}

pub struct Host {
    shared: Arc<Shared>,
    cmd: mpsc::UnboundedSender<HostCmd>,
    task: JoinHandle<()>,
}

impl Host {
    pub fn start(cfg: HostConfig) -> Result<Arc<Host>> {
        let link = Arc::new(cfg.link.link()?);
        let events = Arc::new(Queue::new(1024));
        let (broadcast, _) = broadcast::channel(32);
        let shared = Arc::new(Shared {
            info: Mutex::new(Info {
                hostname: cfg.link.name.clone(),
                os: cfg.link.os.clone(),
                version: cfg.version,
                displays: cfg.displays,
                codecs: cfg.codecs,
                encoder: None,
                input: cfg.input,
                touch: cfg.touch,
                actions: cfg.actions,
                capture_failure: None,
            }),
            video: video::VideoHub::new(events.clone()),
            audio: audio::AudioHub::new(events.clone()),
            events,
            broadcast,
            controllers: Mutex::default(),
            approval: AtomicBool::new(cfg.approval),
            pending: Mutex::default(),
            trusted: Mutex::default(),
        });
        let reclaim = cfg.reclaim.and_then(|s| {
            Some(Reclaim {
                code: s.code,
                secret: unhex::<16>(&s.secret)?,
            })
        });
        let (cmd, rx) = mpsc::unbounded_channel();
        let task = rt().spawn(run(link, shared.clone(), reclaim, rx));
        Ok(Arc::new(Host { shared, cmd, task }))
    }

    pub fn poll_event(&self, timeout: Duration) -> Option<String> {
        self.shared.events.pop(timeout)
    }

    pub fn video_hub(&self) -> &video::VideoHub {
        &self.shared.video
    }

    pub fn audio_hub(&self) -> &audio::AudioHub {
        &self.shared.audio
    }

    pub fn call(&self, json: &str) -> Result<()> {
        let op: Op = serde_json::from_str(json)?;
        let sh = &self.shared;
        match op {
            Op::KickAll => {
                let _ = self.cmd.send(HostCmd::KickAll);
            }
            Op::NewCode => {
                let _ = self.cmd.send(HostCmd::NewCode);
            }
            Op::Displays { displays } => {
                let mut info = sh.info.lock().unwrap();
                if info.displays != displays {
                    info.displays = displays.clone();
                    drop(info);
                    sh.broadcast(&HostMsg::Displays {
                        displays,
                        current: 0,
                    });
                }
            }
            Op::Encoder { info: e } => {
                sh.video.reset_config();
                sh.info.lock().unwrap().encoder = Some(e.clone());
                sh.broadcast(&HostMsg::Encoder(e));
            }
            Op::Permissions { input } => {
                let mut info = sh.info.lock().unwrap();
                if info.input != input {
                    info.input = input;
                    drop(info);
                    sh.broadcast(&HostMsg::Notice(Notice::MissingPermissions {
                        screen: false,
                        input: !input,
                    }));
                }
            }
            Op::Capture { failure } => {
                let mut info = sh.info.lock().unwrap();
                if info.capture_failure != failure {
                    info.capture_failure = failure.clone();
                    drop(info);
                    sh.broadcast(&HostMsg::Notice(match failure {
                        Some(detail) => Notice::CaptureFailed { detail },
                        None => Notice::CaptureRecovered,
                    }));
                }
            }
            Op::Audio { failure } => sh.audio.set_failure(failure),
            Op::Clipboard { text } => {
                if !text.is_empty() && text.len() <= airhand_proto::session::MAX_CLIPBOARD_TEXT {
                    sh.broadcast(&HostMsg::Clipboard { text });
                }
            }
            Op::Approve { id, allow } => {
                if let Some(tx) = id.parse().ok().and_then(|id| sh.take_pending(id)) {
                    let _ = tx.send(allow);
                }
            }
            Op::Approval { required } => {
                info!(required, "approval setting changed");
                sh.approval.store(required, Ordering::Relaxed);
            }
        }
        Ok(())
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // 中止任务会依次释放会话、QUIC 连接（自动向服务器发送关闭帧）
        self.task.abort();
        self.shared.events.close();
    }
}

fn reporter(shared: &Arc<Shared>) -> Report {
    let sh = shared.clone();
    Arc::new(move |s: ServerState| sh.event(json!({ "type": "server", "server": s })))
}

async fn run(
    link: Arc<LinkConfig>,
    shared: Arc<Shared>,
    mut reclaim: Option<Reclaim>,
    mut cmds: mpsc::UnboundedReceiver<HostCmd>,
) {
    let mut reconnect = Reconnect::new(reporter(&shared));
    loop {
        reconnect.begin();
        let result = connect_once(&link, &shared, &mut reclaim, &mut cmds).await;
        shared.controllers.lock().unwrap().clear();
        shared.report_controllers();
        reconnect.end("host", result).await;
    }
}

/// 返回 `Ok(())` 表示需要立即重连（更换控制码），`Err` 表示掉线。
async fn connect_once(
    link: &LinkConfig,
    shared: &Arc<Shared>,
    reclaim: &mut Option<Reclaim>,
    cmds: &mut mpsc::UnboundedReceiver<HostCmd>,
) -> Result<()> {
    let mut sig = net::connect_signal(link, Role::Host).await?;
    write_msg(
        &mut sig.send,
        &ClientMsg::Register {
            reclaim: *reclaim,
            keep: true,
        },
    )
    .await?;

    // 独立的读任务，保证 select 中的接收是取消安全的
    let (tx, mut rx) = mpsc::channel::<ServerMsg>(16);
    let mut recv = sig.recv;
    let reader = tokio::spawn(async move {
        let mut buf = Vec::new();
        while let Ok(Some(m)) = read_msg::<_, ServerMsg>(&mut recv, MAX_SIGNAL_MSG, &mut buf).await
        {
            if tx.send(m).await.is_err() {
                break;
            }
        }
    });
    let _reader = AbortOnDrop(reader);

    let first = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .map_err(|_| anyhow!("registration timed out"))?
        .ok_or_else(|| anyhow!("the server closed the connection before registering"))?;
    let ServerMsg::Registered { code, secret } = first else {
        bail!("registration failed: {first:?}");
    };
    if reclaim.is_some_and(|r| r.code != code) {
        info!("the previous control code isn't available anymore, got a new one");
    }
    *reclaim = Some(Reclaim { code, secret });
    info!(code, "host connected to the server");
    shared.event(json!({ "type": "registered", "code": code, "secret": hex(&secret) }));
    shared.event(json!({ "type": "server", "server": ServerState::Connected }));

    let mut sessions: JoinSet<u64> = JoinSet::new();
    // 每次发送都会让所有会话向控制端发送 Bye（带上这个原因）并结束
    let (kick_tx, _) = watch::channel(ByeReason::Kicked);
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(ServerMsg::Incoming { session_id, peer }) => {
                    info!(session_id, name = %peer.name, ip = %peer.ip, "controller wants to connect");
                    shared.controllers.lock().unwrap().push(ControllerView {
                        id: session_id,
                        name: peer.name.clone(),
                        ip: peer.ip.to_string(),
                        pending: false,
                        active: false,
                    });
                    shared.report_controllers();
                    let (conn, kick, sh) = (sig.conn.clone(), kick_tx.subscribe(), shared.clone());
                    sessions.spawn(async move {
                        let _ = session::run(conn, session_id, kick, sh).await;
                        session_id
                    });
                }
                Some(other) => warn!("ignoring server message: {other:?}"),
                None => match sig.conn.close_reason() {
                    Some(reason) => bail!(reason),
                    None => bail!("the server closed the control stream"),
                },
            },
            Some(done) = sessions.join_next() => {
                if let Ok(id) = done {
                    shared.controllers.lock().unwrap().retain(|c| c.id != id);
                    shared.report_controllers();
                }
            }
            Some(cmd) = cmds.recv() => {
                info!(?cmd, "command from the app");
                // 断开了的设备要重新请本机用户同意
                shared.trusted.lock().unwrap().clear();
                kick_tx.send_replace(match cmd {
                    HostCmd::KickAll => ByeReason::Kicked,
                    HostCmd::NewCode => ByeReason::CodeChanged,
                });
                if let HostCmd::NewCode = cmd {
                    // 给会话一点时间把 Bye 发出去，然后放弃旧控制码重新注册
                    let _ = tokio::time::timeout(Duration::from_millis(800), async {
                        while sessions.join_next().await.is_some() {}
                    })
                    .await;
                    let release = async {
                        write_msg(&mut sig.send, &ClientMsg::Release).await?;
                        // 服务器处理完 Release 才会读到控制流结束、断开连接
                        let _ = sig.send.finish();
                        sig.conn.closed().await;
                        anyhow::Ok(())
                    };
                    let _ = tokio::time::timeout(Duration::from_secs(2), release).await;
                    *reclaim = None;
                    shared.event(json!({ "type": "released" }));
                    sig.conn.close(quinn::VarInt::from_u32(0), b"new code");
                    return Ok(());
                }
            }
            reason = sig.conn.closed() => bail!(reason),
        }
    }
}

struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_ids_are_strings() {
        let v = ControllerView {
            id: u64::MAX,
            name: "PC".into(),
            ip: "1.2.3.4".into(),
            pending: true,
            active: false,
        };
        assert_eq!(
            serde_json::to_value(&v).unwrap(),
            json!({ "id": "18446744073709551615", "name": "PC", "ip": "1.2.3.4", "pending": true, "active": false })
        );
    }

    #[test]
    fn config_and_ops_parse() {
        let cfg: HostConfig = serde_json::from_str(
            r#"{"server":"127.0.0.1:4433","fingerprint":"","name":"Pixel 8","os":"Android 15",
                "version":"0.1.0","reclaim":{"code":123456,"secret":"00112233445566778899aabbccddeeff"},
                "displays":[{"index":0,"name":"Screen","x":0,"y":0,"width":1080,"height":2400,"primary":true}],
                "codecs":["H264","H265"],"input":false,"touch":true,"actions":["Back","Home","VolumeUp"],"approval":true}"#,
        )
        .unwrap();
        assert_eq!(cfg.link.name, "Pixel 8");
        assert_eq!(cfg.codecs, [Codec::H264, Codec::H265]);
        assert_eq!(cfg.reclaim.unwrap().code, 123456);
        assert!(cfg.touch && cfg.approval);
        assert_eq!(
            cfg.actions,
            [
                SystemAction::Back,
                SystemAction::Home,
                SystemAction::VolumeUp
            ]
        );

        for op in [
            r#"{"op":"kickAll"}"#,
            r#"{"op":"newCode"}"#,
            r#"{"op":"permissions","input":true}"#,
            r#"{"op":"capture","failure":null}"#,
            r#"{"op":"audio","failure":"no RECORD_AUDIO permission"}"#,
            r#"{"op":"clipboard","text":"hi"}"#,
            r#"{"op":"encoder","info":{"codec":"H264","name":"c2.qti.avc.encoder","hardware":true,"width":1080,"height":2400}}"#,
            r#"{"op":"displays","displays":[]}"#,
            r#"{"op":"approve","id":"18303851325851793452","allow":true}"#,
            r#"{"op":"approval","required":false}"#,
        ] {
            serde_json::from_str::<Op>(op).unwrap_or_else(|e| panic!("{op}: {e}"));
        }
    }
}
