//! 全局运行状态（模式 / 被控端 / 控制端与服务器的连接），托盘菜单据此刷新。

use std::sync::Arc;

use airhand_link::ServerState;
use airhand_link::net::Report;
use airhand_proto::signal::Peer;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 被控端（默认）。Windows 安装版的被控端一直在系统服务里运行，这里只表示控制台没有打开
    Host,
    /// 控制端（控制台打开着）
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStatus {
    pub mode: Mode,
    /// 被控端
    pub host: HostStatus,
    /// 控制端与服务器的连接（控制端模式）
    pub controller: ServerState,
    /// Windows 安装版：被控端运行在系统服务里（见 [`crate::winsvc`]）；被控端在本进程里运行时为 `None`
    pub service: Option<ServiceStatus>,
}

impl Default for AppStatus {
    fn default() -> Self {
        Self {
            mode: Mode::Host,
            host: HostStatus::default(),
            controller: ServerState::Connecting,
            service: None,
        }
    }
}

impl AppStatus {
    /// 托盘要不要显示被控端的状态：被控端在服务里运行时一直显示，在本进程里运行时只在被控端模式显示
    pub fn shows_host(&self) -> bool {
        self.service.is_some() || self.mode == Mode::Host
    }
}

/// 被控端的状态。Windows 安装版由系统服务里的被控端经命名管道发给托盘（见 `winsvc::ipc`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStatus {
    pub server: ServerState,
    /// 控制码（已连接时有效）
    pub code: Option<u32>,
    /// 正在控制本机的控制端
    pub controllers: Vec<Controller>,
}

impl Default for HostStatus {
    fn default() -> Self {
        Self {
            server: ServerState::Connecting,
            code: None,
            controllers: Vec::new(),
        }
    }
}

/// Windows 安装版：系统服务里的被控端
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceStatus {
    /// 托盘连上了服务里的被控端（没连上时 [`AppStatus::host`] 没有意义）
    pub connected: bool,
    /// 服务在运行（没连上的话：被控端正在启动，或者在另一个用户的会话里）
    pub running: bool,
    /// 本会话不是控制台会话：被控端在屏幕前的另一个用户那里运行（快速切换用户、远程桌面）
    pub elsewhere: bool,
    /// 开机自动启动
    pub autostart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Controller {
    pub session_id: u64,
    pub peer: Peer,
}

pub type StatusHub = Arc<watch::Sender<AppStatus>>;

pub fn new_hub() -> StatusHub {
    Arc::new(watch::Sender::new(AppStatus::default()))
}

/// 把被控端与服务器的连接状态写进 `hub`（给 [`airhand_link::net::Reconnect`] 用）
pub fn host_reporter(hub: &StatusHub) -> Report {
    let hub = hub.clone();
    Arc::new(move |s| {
        hub.send_if_modified(|st| {
            let changed = st.host.server != s;
            st.host.server = s;
            changed
        });
    })
}

/// 把控制端与服务器的连接状态写进 `hub`
pub fn controller_reporter(hub: &StatusHub) -> Report {
    let hub = hub.clone();
    Arc::new(move |s| {
        hub.send_if_modified(|st| {
            let changed = st.controller != s;
            st.controller = s;
            changed
        });
    })
}
