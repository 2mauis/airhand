//! 全局运行状态（模式 / 服务器连接 / 控制码 / 会话），托盘菜单据此刷新。

use std::sync::Arc;

use airhand_proto::signal::Peer;
use serde::Serialize;
use tokio::sync::watch;

use crate::problem::Problem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 被控端（默认）
    Host,
    /// 控制端
    Controller,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ServerState {
    Connecting,
    Connected,
    /// 断开的原因（类型，由托盘 / 控制台按各自的界面语言显示）
    Disconnected {
        reason: Problem,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStatus {
    pub mode: Mode,
    pub server: ServerState,
    /// 被控端的控制码（已连接时有效）
    pub code: Option<u32>,
    /// 正在控制本机的控制端
    pub controllers: Vec<Controller>,
}

impl Default for AppStatus {
    fn default() -> Self {
        Self {
            mode: Mode::Host,
            server: ServerState::Connecting,
            code: None,
            controllers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub session_id: u64,
    pub peer: Peer,
}

pub type StatusHub = Arc<watch::Sender<AppStatus>>;

pub fn new_hub() -> StatusHub {
    Arc::new(watch::Sender::new(AppStatus::default()))
}

pub trait HubExt {
    fn set_server(&self, s: ServerState);
}

impl HubExt for StatusHub {
    fn set_server(&self, s: ServerState) {
        self.send_if_modified(|st| {
            let changed = st.server != s;
            st.server = s;
            changed
        });
    }
}
