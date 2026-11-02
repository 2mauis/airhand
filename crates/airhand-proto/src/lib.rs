//! Airhand 协议定义。
//!
//! 整体结构：
//!
//! ```text
//!  被控端(Host) ──QUIC──┐                 ┌──QUIC── 控制端(Controller)
//!                      │  airhand-server │
//!   控制流(bi #0) ──────┤   信令 + 中转   ├────── 控制流(bi #0)
//!   会话流(bi #n) ══════╪═════ 字节透传 ══╪══════ 会话流(bi #n)
//! ```
//!
//! * 每条 QUIC 连接上客户端打开的**第一条**双向流是"控制流"，承载 [`signal`] 消息。
//! * 之后每建立一个远控会话，双方各自打开一条新的双向流，先写入 [`SessionHeader`]，
//!   服务器据此把两条流配对，然后**零拷贝透传**（服务器不解析画面数据）。
//! * 会话流上承载 [`session`] 消息。
//!
//! 所有消息使用 `u32 LE 长度前缀 + postcard` 编码，紧凑且解析极快。

pub mod framing;
pub mod session;
pub mod signal;

/// 协议版本，服务器与客户端不一致时拒绝连接（见 [`signal`] 模块的"版本握手"）。
///
/// 任何不兼容的改动（增删字段、调整枚举变体顺序……）都要把它加 1。
pub const PROTOCOL_VERSION: u16 = 5;

/// QUIC ALPN 标识。与版本握手一样永远不变，否则旧客户端连 TLS 握手都过不了，无法得知版本不一致。
pub const ALPN: &[u8] = b"airhand/1";

/// 控制码范围：6 位数字，首位不为 0，避免口头报码时产生歧义。
pub const CODE_MIN: u32 = 100_000;
pub const CODE_MAX: u32 = 999_999;

/// 控制流单条消息上限。
pub const MAX_SIGNAL_MSG: usize = 64 * 1024;
/// 会话流单条消息上限（一个 4K 关键帧也远小于这个值）。
pub const MAX_SESSION_MSG: usize = 32 * 1024 * 1024;

/// 会话流头部魔数。
pub const SESSION_MAGIC: [u8; 4] = *b"AHS1";

/// 连接角色。
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// 被控端
    Host,
    /// 控制端
    Controller,
}

/// 会话流头部（固定 13 字节）：`magic[4] | session_id u64 LE | role u8`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionHeader {
    pub session_id: u64,
    pub role: Role,
}

impl SessionHeader {
    pub const LEN: usize = 13;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let mut b = [0u8; Self::LEN];
        b[..4].copy_from_slice(&SESSION_MAGIC);
        b[4..12].copy_from_slice(&self.session_id.to_le_bytes());
        b[12] = match self.role {
            Role::Host => 1,
            Role::Controller => 2,
        };
        b
    }

    pub fn decode(b: &[u8; Self::LEN]) -> Option<Self> {
        if b[..4] != SESSION_MAGIC {
            return None;
        }
        let session_id = u64::from_le_bytes(b[4..12].try_into().ok()?);
        let role = match b[12] {
            1 => Role::Host,
            2 => Role::Controller,
            _ => return None,
        };
        Some(Self { session_id, role })
    }
}

/// 把控制码格式化为 `123 456` 形式，便于阅读。
pub fn format_code(code: u32) -> String {
    let s = format!("{code:06}");
    format!("{} {}", &s[..3], &s[3..])
}

/// 解析用户输入的控制码（允许空格/短横线分隔）。
pub fn parse_code(input: &str) -> Option<u32> {
    let digits: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let code: u32 = digits.parse().ok()?;
    (CODE_MIN..=CODE_MAX).contains(&code).then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = SessionHeader {
            session_id: 0xdead_beef_1234,
            role: Role::Controller,
        };
        assert_eq!(SessionHeader::decode(&h.encode()), Some(h));
        let mut bad = h.encode();
        bad[0] = b'X';
        assert_eq!(SessionHeader::decode(&bad), None);
    }

    #[test]
    fn code_parse() {
        assert_eq!(parse_code("123 456"), Some(123456));
        assert_eq!(parse_code("123-456"), Some(123456));
        assert_eq!(parse_code("012345"), None);
        assert_eq!(parse_code("12345"), None);
        assert_eq!(parse_code("12345a"), None);
        assert_eq!(format_code(123456), "123 456");
    }
}
