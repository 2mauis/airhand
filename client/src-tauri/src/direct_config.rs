//! `config.toml` 的 `[direct]` 段（`docs/specs/tailcat-p2p.md`「配置」）。
//!
//! 尚未实现：这里只有让 `tests/direct_config.rs`（先红验收测试）能编译的类型与签名。[`parse`] 目前是空操作：
//! 不读取文本、不做范围检查，返回全零的 [`DirectConfig`]，先红用例因断言不满足而失败。

use anyhow::Result;

/// `[direct]` 的取值
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectConfig {
    pub enabled: bool,
    pub upgrade_timeout_secs: u32,
    pub rx_silence_ms: u32,
    pub ack_stall_ms: u32,
    pub handshake_watchdog_ms: u32,
    pub write_timeout_ms: u32,
    pub bitrate_cap_kbps: u32,
    pub max_sessions: u32,
    pub derp_map_url: String,
}

/// 从 `config.toml` 的全文解析 `[direct]`：没有这一段或缺少某个键时取默认值，超出范围的值报错
pub fn parse(_text: &str) -> Result<DirectConfig> {
    Ok(DirectConfig::default())
}
