//! `config.toml` 的 `[direct]` 段（`docs/specs/tailcat-p2p.md`「配置」）。
//!
//! 尚未实现：这里只有让 `tests/direct_config.rs`（先红验收测试）能编译的类型与签名，[`parse`] 一律返回错误。

use anyhow::{Result, bail};

/// `[direct]` 的取值
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectConfig {
    pub enabled: bool,
    pub upgrade_timeout_secs: u32,
    pub rx_silence_ms: u32,
    pub write_timeout_ms: u32,
    pub bitrate_cap_kbps: u32,
    pub max_sessions: u32,
}

/// 从 `config.toml` 的全文解析 `[direct]`：没有这一段或缺少某个键时取默认值，超出范围的值报错
pub fn parse(_text: &str) -> Result<DirectConfig> {
    bail!("[direct] is not implemented yet (tailcat-p2p Phase 1)")
}
