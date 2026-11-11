//! 被控端配置重载（“放宽延后生效”，`docs/specs/tailcat-p2p.md` I21 / 防注入）的测试驱动。
//!
//! 尚未实现：客户端还没有 `[host] approve` 与放宽确认提示，这里只有让 `tests/host_config_reload.rs` 能编译的
//! 类型与签名，全部是空操作——不读配置、改动从不生效、从不显示提示、新会话没有任何结果，用例因正向对照
//! 不成立而以 `ASSERT:` 失败。实现时把这些方法接到客户端的配置重载与提示上（虚拟时钟），不改用例本身。
#![allow(dead_code)]

/// `[host] approve`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approve {
    Ask,
    Never,
}

/// 当前生效的配置
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effective {
    pub approve: Option<Approve>,
    pub direct_enabled: bool,
}

/// 新会话到达时被控端的处理
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 按当前 `approve` 正常处理（弹审批或免确认）
    Handled,
    /// `Bye(Busy)`
    Busy,
    /// 没有任何结果
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionId(pub u32);

/// 一台被控端：配置文件、在途会话与放宽确认提示；时间是虚拟的（秒）
#[derive(Debug, Default)]
pub struct Host {
    next: u32,
}

impl Host {
    /// 以 `config.toml` 全文启动
    pub fn start(_config: &str) -> Self {
        Self::default()
    }

    /// 已授权的会话；`input` = 带输入控制权限
    pub fn session(&mut self, _input: bool) -> SessionId {
        self.next += 1;
        SessionId(self.next)
    }

    /// 给在途会话追加输入控制权限
    pub fn grant_input(&mut self, _id: SessionId) {}

    pub fn end_session(&mut self, _id: SessionId) {}

    /// 改写配置文件（触发重载）
    pub fn edit(&mut self, _config: &str) {}

    pub fn effective(&self) -> Effective {
        Effective::default()
    }

    /// 放宽配置的本机确认提示是否正在显示
    pub fn prompt_shown(&self) -> bool {
        false
    }

    /// 本机点击“确认放宽”
    pub fn click_confirm(&mut self) {}

    /// 新会话到达
    pub fn new_session(&mut self) -> Outcome {
        Outcome::None
    }

    pub fn advance(&mut self, _secs: u64) {}
}
