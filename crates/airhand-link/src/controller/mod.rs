//! 控制端服务：维护与服务器的连接（自动重连），管理多个并发会话。
//!
//! 与界面无关：会话的画面和事件交给 [`SessionUi`]。控制台可以和本服务在同一台电脑上（桌面版），
//! 也可以是经网络连过来的浏览器（网页版），两者的区别见 [`Frontend`]。
//!
//! # 被控端掉线后的重连
//!
//! 会话断了由控制台决定是否重连（再调用一次 [`ControllerService::connect`]）。连上时得到的重连凭证
//!（[`SessionInfo::resume`]，见 `airhand_proto::signal` 模块说明）由控制台保管，重连时带上：
//! 这样的重连不会被服务器当成猜码而限速。被控端不在线时 `connect` 返回 [`Problem::HostOffline`]，
//! 被控端重新上线时服务器通知，本服务调用 [`HostOnline`]，控制台这时再连一次即可，等待期间不需要反复尝试。
//! 与服务器的连接断过之后服务器不再记得谁在等谁，控制台在重新连上服务器时再连一次（还不在线的话会重新登记）。

mod queue;
mod session;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::transport::{HostReceiver, open_session};
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::session::{
    ControllerHello, ControllerMsg, InputEvent, MAX_CLIPBOARD_TEXT, QualitySettings,
};
use airhand_proto::signal::{ClientMsg, ConnectError, ResumeToken, ServerMsg, SessionGrant};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Result, anyhow};
use quinn::Connection;
use serde::Serialize;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tracing::{info, warn};

use crate::net::{self, LinkConfig, Reconnect, Report};
use crate::{Problem, ServerState};
pub use session::{HEADER_LEN, NoticeView, SessionEvent, SessionUi, UiClosed};

/// 桌面版检查本机剪贴板变化的间隔
pub const CLIPBOARD_POLL: Duration = Duration::from_millis(300);

/// 本机剪贴板的双向同步（桌面版；实现见客户端的 `clipboard.rs`）。
pub trait ClipboardSync: Send + 'static {
    /// 每隔 [`CLIPBOARD_POLL`] 调用一次：本机剪贴板有新文本（且不是对端刚写入的）时返回它
    fn poll(&mut self) -> Pin<Box<dyn Future<Output = Option<String>> + Send + '_>>;
    /// 写入被控端复制的文本
    fn apply(&self, text: String);
}

/// 控制台在哪里。
pub enum Frontend {
    /// 桌面控制台：由 WebView 收到画面后确认，并同步这台电脑的剪贴板
    Desktop {
        clipboard: Box<dyn ClipboardSync>,
        /// 是否同步剪贴板（之后可用 [`ControllerService::set_clipboard_sync`] 切换）
        sync: bool,
    },
    /// 浏览器，经网络连到本服务（网页控制台）：
    ///
    /// * 画面由浏览器收到后确认（[`ControllerService::ack`]），被控端的在途帧控制因此把浏览器这一段网络也算进去，
    ///   这段网络慢的时候画面不会在中间越积越多；
    /// * 剪贴板是浏览器的：被控端复制的文本作为 [`SessionEvent::Clipboard`] 交给浏览器，
    ///   浏览器复制的文本用 [`ControllerService::send_clipboard`] 发给被控端。
    Browser,
}

/// 服务器通知：控制码为 `code` 的被控端重新上线了，在等它的会话（[`Problem::HostOffline`]）现在重连
pub type HostOnline = Arc<dyn Fn(u32) + Send + Sync>;

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Result<SessionGrant, ConnectError>>>>>;

#[derive(Clone)]
struct Live {
    conn: Connection,
    tx: mpsc::Sender<ClientMsg>,
    pending: Pending,
}

struct SessionHandle {
    tx: queue::Sender,
    task: AbortHandle,
    /// 桌面版：是否同步剪贴板
    clipboard: Option<Arc<AtomicBool>>,
}

struct Inner {
    cfg: Arc<LinkConfig>,
    /// 本次运行的随机标识，每个会话开始时告诉被控端（见 [`ControllerHello::device`]）：
    /// 断线重连时被控端认得出是同一台，不用再请它的用户同意
    device: [u8; 16],
    rt: Handle,
    live: Mutex<Option<Live>>,
    sessions: Mutex<HashMap<u32, SessionHandle>>,
    next_req: AtomicU32,
    next_sid: AtomicU32,
    host_online: HostOnline,
}

pub struct ControllerService {
    inner: Arc<Inner>,
    task: JoinHandle<()>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub sid: u32,
    pub code: String,
    pub host_name: String,
    pub host_os: String,
    /// 重连凭证：这个会话断了之后重连时交给 [`ControllerService::connect`]
    pub resume: ResumeToken,
}

impl ControllerService {
    /// 在 `rt` 上启动服务：连接服务器并在掉线后自动重连，连接状态的变化通过 `report` 通知，
    /// 在等的被控端上线了通过 `host_online` 通知。服务被 drop 时断开所有会话和连接。
    pub fn start(
        cfg: Arc<LinkConfig>,
        rt: Handle,
        report: Report,
        host_online: HostOnline,
    ) -> Self {
        let inner = Arc::new(Inner {
            cfg,
            device: rand::random(),
            rt: rt.clone(),
            live: Mutex::default(),
            sessions: Mutex::default(),
            next_req: AtomicU32::new(0),
            next_sid: AtomicU32::new(0),
            host_online,
        });
        let task = rt.spawn(run(inner.clone(), report));
        Self { inner, task }
    }

    /// `resume`：重连时带上之前连上这个控制码时得到的凭证（[`SessionInfo::resume`]），第一次连接时为 `None`
    pub async fn connect(
        &self,
        code: u32,
        resume: Option<ResumeToken>,
        settings: QualitySettings,
        ui: Arc<dyn SessionUi>,
        frontend: Frontend,
    ) -> Result<SessionInfo, Problem> {
        let live = self
            .inner
            .live
            .lock()
            .unwrap()
            .clone()
            .ok_or(Problem::NotConnected)?;
        let req_id = self.inner.next_req.fetch_add(1, Ordering::Relaxed);
        let (otx, orx) = oneshot::channel();
        {
            let mut pending = live.pending.lock().unwrap();
            if pending.len() >= 64 {
                return Err(Problem::failed("too many pending connections"));
            }
            pending.insert(req_id, otx);
        }
        // Also remove cancelled requests, not just requests that time out.
        struct RequestGuard(Pending, u32);
        impl Drop for RequestGuard {
            fn drop(&mut self) {
                self.0.lock().unwrap().remove(&self.1);
            }
        }
        let _request = RequestGuard(live.pending.clone(), req_id);
        live.tx
            .try_send(ClientMsg::Connect {
                req_id,
                code,
                resume,
            })
            .map_err(|_| server_gone())?;
        let grant = match tokio::time::timeout(Duration::from_secs(10), orx).await {
            Ok(Ok(Ok(g))) => g,
            Ok(Ok(Err(e))) => return Err(e.into()),
            Ok(Err(_)) => return Err(server_gone()),
            Err(_) => {
                live.pending.lock().unwrap().remove(&req_id);
                return Err(Problem::ServerTimeout);
            }
        };

        let streams = open_session(&live.conn, grant.session_id, Role::Controller)
            .await
            .map_err(Problem::failed)?;
        let (mut send, recv) = streams.control;
        let recv = HostReceiver::new(recv, streams.video, streams.audio);
        let hello = ControllerMsg::Hello(ControllerHello {
            device: self.inner.device,
        });
        write_msg(&mut send, &hello)
            .await
            .map_err(Problem::failed)?;
        write_msg(&mut send, &ControllerMsg::Settings(settings.sanitized()))
            .await
            .map_err(Problem::failed)?;

        let sid = self.inner.next_sid.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = queue::channel();
        let (mode, clipboard) = match frontend {
            Frontend::Desktop { clipboard, sync } => {
                let enabled = Arc::new(AtomicBool::new(sync));
                let mode = session::Mode::Desktop {
                    clip: Arc::new(tokio::sync::Mutex::new(clipboard)),
                    enabled: enabled.clone(),
                };
                (mode, Some(enabled))
            }
            Frontend::Browser => (session::Mode::Browser, None),
        };
        let io = session::SessionIo {
            send,
            recv,
            ui,
            mode,
        };
        let inner = self.inner.clone();
        let tx2 = tx.clone();
        // Hold the registry lock until insertion; a fast EOF must not remove a
        // not-yet-inserted entry and leave a dead handle behind.
        let mut sessions = self.inner.sessions.lock().unwrap();
        let handle = self.inner.rt.spawn(async move {
            session::run(sid, io, tx2, rx).await;
            inner.sessions.lock().unwrap().remove(&sid);
        });
        sessions.insert(
            sid,
            SessionHandle {
                tx,
                task: handle.abort_handle(),
                clipboard,
            },
        );
        info!(sid, code, host = %grant.host_name, "session established");
        Ok(SessionInfo {
            sid,
            code: airhand_proto::format_code(code),
            host_name: grant.host_name,
            host_os: grant.host_os,
            resume: grant.resume,
        })
    }

    fn send(&self, sid: u32, msg: ControllerMsg) -> Result<(), Problem> {
        let sessions = self.inner.sessions.lock().unwrap();
        let s = sessions.get(&sid).ok_or(Problem::NoSession)?;
        s.tx.send(msg)
    }

    pub fn input(&self, sid: u32, events: Vec<InputEvent>) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::Input(events))
    }

    pub fn settings(&self, sid: u32, s: QualitySettings) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::Settings(s.sanitized()))
    }

    pub fn keyframe(&self, sid: u32) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::RequestKeyframe)
    }

    pub fn select_display(&self, sid: u32, index: u8) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::SelectDisplay { index })
    }

    pub fn set_audio(&self, sid: u32, enabled: bool) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::Audio { enabled })
    }

    /// 前端收到了这一帧（桌面 WebView / 浏览器 / Android）。确认是累积的。
    pub fn ack(&self, sid: u32, seq: u32) -> Result<(), Problem> {
        self.send(sid, ControllerMsg::Ack { seq })
    }

    /// 把浏览器复制的文本发给被控端（[`Frontend::Browser`]）
    pub fn send_clipboard(&self, sid: u32, text: String) -> Result<(), Problem> {
        if text.is_empty() || text.len() > MAX_CLIPBOARD_TEXT {
            return Ok(());
        }
        self.send(sid, ControllerMsg::Clipboard { text })
    }

    /// 打开 / 关闭剪贴板同步（[`Frontend::Desktop`]；网页版由浏览器自己决定发不发）
    pub fn set_clipboard_sync(&self, sid: u32, enabled: bool) -> Result<(), Problem> {
        let sessions = self.inner.sessions.lock().unwrap();
        let s = sessions.get(&sid).ok_or(Problem::NoSession)?;
        if let Some(c) = &s.clipboard {
            c.store(enabled, Ordering::Relaxed);
        }
        Ok(())
    }

    pub fn disconnect(&self, sid: u32) {
        if let Some(s) = self.inner.sessions.lock().unwrap().remove(&sid) {
            let _ =
                s.tx.send(ControllerMsg::Input(vec![InputEvent::ReleaseAll]));
            // 给写任务一点时间把"释放按键"发出去
            let task = s.task;
            self.inner.rt.spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                task.abort();
            });
        }
    }
}

/// 等待连接结果时与服务器的连接断了
fn server_gone() -> Problem {
    Problem::ServerLost {
        detail: "the connection closed while waiting for the answer".into(),
    }
}

impl Drop for ControllerService {
    fn drop(&mut self) {
        for (_, s) in self.inner.sessions.lock().unwrap().drain() {
            s.task.abort();
        }
        self.task.abort();
    }
}

async fn run(inner: Arc<Inner>, report: Report) {
    let mut reconnect = Reconnect::new(report.clone());
    loop {
        reconnect.begin();
        // 控制端不会主动要求重连：控制流正常结束也算掉线
        let result = connect_once(&inner, &report)
            .await
            .and_then(|()| Err(anyhow!("the server closed the control stream")));
        let live = inner.live.lock().unwrap().take();
        if let Some(live) = live {
            // 让所有等待中的连接请求立即失败
            live.pending.lock().unwrap().clear();
        }
        reconnect.end("controller", result).await;
    }
}

async fn connect_once(inner: &Arc<Inner>, report: &Report) -> Result<()> {
    let sig = net::connect_signal(&inner.cfg, Role::Controller).await?;
    let (tx, mut rx) = mpsc::channel::<ClientMsg>(64);
    let pending: Pending = Arc::default();
    *inner.live.lock().unwrap() = Some(Live {
        conn: sig.conn.clone(),
        tx,
        pending: pending.clone(),
    });
    report(ServerState::Connected);
    info!("controller connected to the server");

    let mut send = sig.send;
    let mut recv = sig.recv;
    let inner = inner.clone();
    let writer = async move {
        while let Some(m) = rx.recv().await {
            write_msg(&mut send, &m).await?;
        }
        anyhow::Ok(())
    };
    let reader = async move {
        let mut buf = Vec::new();
        while let Some(m) = read_msg::<_, ServerMsg>(&mut recv, MAX_SIGNAL_MSG, &mut buf).await? {
            match m {
                ServerMsg::ConnectResult { req_id, result } => {
                    if let Some(tx) = pending.lock().unwrap().remove(&req_id) {
                        let _ = tx.send(result);
                    }
                }
                ServerMsg::HostOnline { code } => {
                    info!(code, "a host we were waiting for is back");
                    (inner.host_online)(code);
                }
                other => warn!("ignoring server message: {other:?}"),
            }
        }
        anyhow::Ok(())
    };
    tokio::select! {
        r = reader => r,
        r = writer => r,
        e = sig.conn.closed() => Err(anyhow!(e)),
    }
}
