//! 控制端服务：维护与服务器的连接（自动重连），管理多个并发会话。

pub mod session;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::session::{ControllerMsg, InputEvent, QualitySettings};
use airhand_proto::signal::{ClientMsg, ConnectError, ServerMsg, SessionGrant};
use airhand_proto::{MAX_SIGNAL_MSG, Role, SessionHeader};
use anyhow::{Result, anyhow};
use quinn::Connection;
use serde::Serialize;
use tauri::async_runtime::JoinHandle;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use crate::net::{self, Reconnect};
use crate::problem::Problem;
use crate::status::{HubExt, ServerState, StatusHub};
use session::{SessionEvent, SessionIo};

type Pending = Arc<Mutex<HashMap<u32, oneshot::Sender<Result<SessionGrant, ConnectError>>>>>;

#[derive(Clone)]
struct Live {
    conn: Connection,
    tx: mpsc::UnboundedSender<ClientMsg>,
    pending: Pending,
}

struct SessionHandle {
    tx: mpsc::UnboundedSender<ControllerMsg>,
    task: tokio::task::AbortHandle,
    clipboard: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    live: Mutex<Option<Live>>,
    sessions: Mutex<HashMap<u32, SessionHandle>>,
    next_req: AtomicU32,
    next_sid: AtomicU32,
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
}

impl ControllerService {
    pub fn start(hub: StatusHub) -> Self {
        let inner = Arc::new(Inner::default());
        let task = tauri::async_runtime::spawn(run(inner.clone(), hub));
        Self { inner, task }
    }

    pub async fn connect(
        &self,
        code: u32,
        settings: QualitySettings,
        video: Channel<InvokeResponseBody>,
        events: Channel<SessionEvent>,
        clipboard_sync: bool,
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
        live.pending.lock().unwrap().insert(req_id, otx);
        live.tx
            .send(ClientMsg::Connect { req_id, code })
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

        let (mut send, recv) = live.conn.open_bi().await.map_err(Problem::failed)?;
        let header = SessionHeader {
            session_id: grant.session_id,
            role: Role::Controller,
        }
        .encode();
        send.write_all(&header).await.map_err(Problem::failed)?;
        write_msg(&mut send, &ControllerMsg::Settings(settings.sanitized()))
            .await
            .map_err(Problem::failed)?;

        let sid = self.inner.next_sid.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = self.inner.clone();
        let tx2 = tx.clone();
        let clipboard = Arc::new(AtomicBool::new(clipboard_sync));
        let io = SessionIo {
            send,
            recv,
            video,
            events,
            clipboard: clipboard.clone(),
        };
        let handle = tokio::spawn(async move {
            session::run(sid, io, tx2, rx).await;
            inner.sessions.lock().unwrap().remove(&sid);
        });
        self.inner.sessions.lock().unwrap().insert(
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
        })
    }

    fn send(&self, sid: u32, msg: ControllerMsg) -> Result<(), Problem> {
        let sessions = self.inner.sessions.lock().unwrap();
        let s = sessions.get(&sid).ok_or(Problem::NoSession)?;
        s.tx.send(msg).map_err(|_| Problem::SessionEnded)
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

    pub fn set_clipboard_sync(&self, sid: u32, enabled: bool) -> Result<(), Problem> {
        let sessions = self.inner.sessions.lock().unwrap();
        let s = sessions.get(&sid).ok_or(Problem::NoSession)?;
        s.clipboard.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    pub fn disconnect(&self, sid: u32) {
        if let Some(s) = self.inner.sessions.lock().unwrap().remove(&sid) {
            let _ =
                s.tx.send(ControllerMsg::Input(vec![InputEvent::ReleaseAll]));
            // 给写任务一点时间把"释放按键"发出去
            let task = s.task;
            tokio::spawn(async move {
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

async fn run(inner: Arc<Inner>, hub: StatusHub) {
    let mut reconnect = Reconnect::new(hub.clone());
    loop {
        reconnect.begin();
        // 控制端不会主动要求重连：控制流正常结束也算掉线
        let result = connect_once(&inner, &hub)
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

async fn connect_once(inner: &Arc<Inner>, hub: &StatusHub) -> Result<()> {
    let sig = net::connect_signal(Role::Controller).await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<ClientMsg>();
    let pending: Pending = Arc::default();
    *inner.live.lock().unwrap() = Some(Live {
        conn: sig.conn.clone(),
        tx,
        pending: pending.clone(),
    });
    hub.set_server(ServerState::Connected);
    info!("controller connected to the server");

    let mut send = sig.send;
    let mut recv = sig.recv;
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
