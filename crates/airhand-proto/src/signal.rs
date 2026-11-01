//! 控制流（客户端 ⇄ 服务器）信令消息。
//!
//! 服务器发给客户端的都是类型（例如 [`ConnectError`]、[`ServerMsg::Incompatible`]），不是文字：
//! 客户端按自己的界面语言显示。
//!
//! # 版本握手
//!
//! 连接后客户端先发 [`ClientMsg::Hello`]，服务器回 [`ServerMsg::Welcome`] 或 [`ServerMsg::Incompatible`]。
//! 为了让**任何版本**的客户端和服务器都能互相告知"版本不一致"，这三条消息的编码永远不能改变：
//! 它们必须是各自枚举的前几个变体（序号 0 / 0 / 1），字段也不能增删。
//! 其它消息可以随 [`crate::PROTOCOL_VERSION`] 自由修改。`framing` 模块的测试锁定了它们的编码。

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::Role;

/// 客户端 → 服务器
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMsg {
    /// 连接后的第一条消息（编码永远不变，见模块说明）。
    ///
    /// `name` 为空表示不知道本机名称，由显示的一方按自己的语言显示"未知设备"。
    Hello {
        version: u16,
        role: Role,
        name: String,
        os: String,
    },
    /// 被控端申请控制码。断线重连时携带上次的控制码与密钥，以便取回同一个控制码。
    Register { reclaim: Option<Reclaim> },
    /// 控制端请求连接某个控制码对应的被控端。
    Connect { req_id: u32, code: u32 },
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Reclaim {
    pub code: u32,
    pub secret: [u8; 16],
}

/// 服务器 → 客户端
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMsg {
    /// 握手成功（编码永远不变，见模块说明）。
    Welcome { version: u16 },
    /// 协议版本不一致，客户端应停止重试，并根据双方版本号提示用户更新客户端还是服务器
    ///（编码永远不变，见模块说明）。
    Incompatible { server_version: u16 },
    /// 被控端注册成功。
    Registered { code: u32, secret: [u8; 16] },
    /// 对 [`ClientMsg::Connect`] 的应答。
    ConnectResult {
        req_id: u32,
        result: Result<SessionGrant, ConnectError>,
    },
    /// 通知被控端：有控制端发起会话，请打开会话流。
    Incoming { session_id: u64, peer: Peer },
}

/// 发起会话的控制端
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// 计算机名，为空表示对方不知道自己的名称
    pub name: String,
    pub ip: IpAddr,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionGrant {
    pub session_id: u64,
    pub host_name: String,
    pub host_os: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectError {
    /// 控制码不存在或对方不在线
    NotFound,
    /// 尝试过于频繁
    RateLimited,
    /// 对方会话数已满
    Busy,
}
