//! Airhand 客户端与中转服务器之间的链路，桌面客户端（`client/src-tauri`）和网页控制台（`web/`）共用：
//!
//! * [`net`]：QUIC 连接、证书固定、版本握手、断线重连；
//! * [`controller`]：控制端服务与会话。与界面无关：画面和事件交给 [`controller::SessionUi`]
//!   （桌面版是 Tauri 的 Channel，网页版是浏览器的 WebSocket）；
//! * [`Problem`]：显示给用户的问题，以类型而不是文字的形式传给界面。

pub mod controller;
pub mod flow;
pub mod net;
mod problem;
pub mod tasks;
pub mod transport;

pub use net::{LinkConfig, ServerState};
pub use problem::Problem;
