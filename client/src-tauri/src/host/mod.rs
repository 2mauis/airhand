//! 被控端服务：保持与服务器的连接（自动重连、取回原控制码），并为每个控制端请求启动会话。

pub(crate) mod audio;
pub mod capture;
pub(crate) mod clock;
#[cfg(windows)]
pub(crate) mod desktop;
pub mod encoder;
mod input;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
mod pipeline;
#[cfg(target_os = "macos")]
pub(crate) mod sck;
mod session;
#[cfg(target_os = "linux")]
pub(crate) mod x11;

use std::time::Duration;

use airhand_link::ServerState;
use airhand_link::net::{self, Reconnect};
use airhand_link::tasks::OwnedTask;
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::session::ByeReason;
use airhand_proto::signal::{ClientMsg, Reclaim, ServerMsg};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Result, anyhow, bail};
use tauri::async_runtime::JoinHandle;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::config;
use crate::status::{Controller, StatusHub};

/// 在"每显示器 DPI 感知"上下文中执行（线程级）。
///
/// DXGI 返回的是物理像素坐标，而 DPI 未感知的线程调用 `GetSystemMetrics` 等 API 会得到
/// 缩放后的逻辑坐标，两者混用会导致鼠标位置错位。所有涉及坐标的调用都包在这里面。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn dpi_aware<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let r = f();
        if !old.is_invalid() {
            SetThreadDpiAwarenessContext(old);
        }
        r
    }
    #[cfg(not(windows))]
    f()
}

/// 被控端缺少的系统权限：`(屏幕与系统录音, 辅助功能)`，true = 缺少。目前只有 macOS 需要用户授权。
pub(crate) fn missing_permissions() -> (bool, bool) {
    #[cfg(target_os = "macos")]
    {
        let p = macos::permissions();
        (!p.screen, !p.input)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (false, false)
    }
}

/// 采集失败是不是因为缺少系统权限（这种情况由会话用 `Notice::MissingPermissions` 单独说明）。
pub(crate) fn missing_permission(e: &anyhow::Error) -> bool {
    #[cfg(target_os = "macos")]
    {
        e.chain().any(|c| c.is::<macos::PermissionDenied>())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = e;
        false
    }
}

/// 托盘菜单发给被控端服务的命令
#[derive(Debug, Clone, Copy)]
pub enum HostCmd {
    /// 断开所有正在控制本机的控制端
    KickAll,
    /// 断开所有控制端并更换控制码（旧控制码立即失效）
    NewCode,
}

pub struct HostService {
    task: JoinHandle<()>,
    cmd: mpsc::UnboundedSender<HostCmd>,
}

impl HostService {
    /// 启动被控端，状态写进 `hub` 的 [`AppStatus::host`](crate::status::AppStatus::host)。
    pub fn start(hub: StatusHub) -> Self {
        clock::init();
        let (cmd, rx) = mpsc::unbounded_channel();
        let task = tauri::async_runtime::spawn(run(hub, rx));
        Self { task, cmd }
    }

    pub fn send(&self, c: HostCmd) {
        let _ = self.cmd.send(c);
    }

    /// 向被控端发命令的另一个入口（系统服务里的被控端经命名管道收到托盘的命令）
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn commands(&self) -> mpsc::UnboundedSender<HostCmd> {
        self.cmd.clone()
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        // 中止任务会依次释放会话、QUIC 连接（自动向服务器发送关闭帧）
        self.task.abort();
    }
}

async fn run(hub: StatusHub, mut cmds: mpsc::UnboundedReceiver<HostCmd>) {
    // 保存过的控制码（config.toml）：第一次连接就取回它
    let mut reclaim: Option<Reclaim> = config::settings().code();
    let mut reconnect = Reconnect::new(crate::status::host_reporter(&hub));
    loop {
        reconnect.begin();
        let result = connect_once(&hub, &mut reclaim, &mut cmds).await;
        hub.send_modify(|s| {
            s.host.code = None;
            s.host.controllers.clear();
        });
        reconnect.end("host", result).await;
    }
}

/// 保存控制码（找不到配置文件的位置时什么也不做）。写不进去只是下次启动时换一个控制码，不影响这次运行
async fn save_code(code: Option<Reclaim>) {
    let r = tokio::task::spawn_blocking(move || config::save_code(code)).await;
    if let Ok(Err(e)) = r {
        warn!("can't save the control code: {e:#}");
    }
}

/// 返回 `Ok(())` 表示需要立即重连（例如更换控制码），`Err` 表示掉线。
async fn connect_once(
    hub: &StatusHub,
    reclaim: &mut Option<Reclaim>,
    cmds: &mut mpsc::UnboundedReceiver<HostCmd>,
) -> Result<()> {
    let mut sig = net::connect_signal(&config::link(), Role::Host).await?;
    write_msg(
        &mut sig.send,
        &ClientMsg::Register {
            reclaim: *reclaim,
            keep: config::persistent(),
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
    let _reader = OwnedTask(reader);

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
    let got = Reclaim { code, secret };
    if *reclaim != Some(got) {
        save_code(Some(got)).await;
    }
    *reclaim = Some(got);
    info!(code, "host connected to the server");
    hub.send_modify(|s| {
        s.host.code = Some(code);
        s.host.server = ServerState::Connected;
    });

    let mut sessions: JoinSet<u64> = JoinSet::new();
    // 每次发送都会让所有会话向控制端发送 Bye（带上这个原因）并结束
    let (kick_tx, _) = watch::channel(ByeReason::Kicked);
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Some(ServerMsg::Incoming { session_id, peer }) => {
                    info!(session_id, name = %peer.name, ip = %peer.ip, "controller wants to connect");
                    hub.send_modify(|s| s.host.controllers.push(Controller { session_id, peer }));
                    let (conn, kick) = (sig.conn.clone(), kick_tx.subscribe());
                    sessions.spawn(async move {
                        let _ = session::run(conn, session_id, kick).await;
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
                    hub.send_modify(|s| s.host.controllers.retain(|c| c.session_id != id));
                }
            }
            Some(cmd) = cmds.recv() => {
                info!(?cmd, "command from the tray");
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
                    // 解除服务器上的绑定，并忘掉保存的控制码（万一重新注册之前就退出了，下次启动也不会再用旧码）
                    let release = async {
                        write_msg(&mut sig.send, &ClientMsg::Release).await?;
                        // 服务器处理完 Release 才会读到控制流结束、断开连接
                        let _ = sig.send.finish();
                        sig.conn.closed().await;
                        anyhow::Ok(())
                    };
                    let _ = tokio::time::timeout(Duration::from_secs(2), release).await;
                    *reclaim = None;
                    save_code(None).await;
                    sig.conn.close(quinn::VarInt::from_u32(0), b"new code");
                    return Ok(());
                }
            }
            reason = sig.conn.closed() => bail!(reason),
        }
    }
}
