//! Windows：让当前线程跟随"输入桌面"。
//!
//! 同一个会话里有好几个桌面：平时是用户的 `Default`，UAC 提权确认框、锁屏、登录界面、Ctrl+Alt+Del 界面在安全桌面
//! `Winlogon` 上，屏幕保护在 `Screen-saver` 上。DXGI 桌面复制和 `SendInput` 都只对**调用线程所在的桌面**有效，
//! 而线程默认待在进程启动时的桌面上，所以采集线程和注入线程在用之前都要切到当前接收输入的桌面上。
//!
//! 只有 SYSTEM 账户能打开安全桌面：Windows 安装版的被控端以 SYSTEM 身份运行在用户的会话里（见 `crate::winsvc`），
//! 所以能采集和操作 UAC 确认框、锁屏和登录界面；直接运行的调试构建打不开它们，切换失败时保持原样。
//!
//! 线程上有窗口或者钩子时不能切换桌面，所以只在专用的线程上调用（采集线程、注入线程）。

use std::cell::Cell;

use tracing::debug;
use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetThreadDesktop,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

thread_local! {
    /// 本线程切换过去的桌面（切到下一个桌面之后才能关闭它）
    static ATTACHED: Cell<Option<HDESK>> = const { Cell::new(None) };
}

fn name(desk: HDESK) -> Option<String> {
    let mut buf = [0u16; 128];
    let mut needed = 0u32;
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 2) as u32,
            Some(&mut needed),
        )
        .ok()?;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// 把当前线程切到输入桌面。返回线程现在是否在输入桌面上（打不开输入桌面、切换失败时为 false）。
pub fn follow() -> bool {
    unsafe {
        let Ok(input) = OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0),
        ) else {
            return false;
        };
        let current = GetThreadDesktop(GetCurrentThreadId()).ok();
        let target = name(input);
        if target.is_some() && current.and_then(name) == target {
            let _ = CloseDesktop(input);
            return true;
        }
        if let Err(e) = SetThreadDesktop(input) {
            debug!("can't switch to the input desktop {target:?}: {e}");
            let _ = CloseDesktop(input);
            return false;
        }
        debug!("switched to the input desktop {target:?}");
        if let Some(old) = ATTACHED.replace(Some(input)) {
            let _ = CloseDesktop(old);
        }
        true
    }
}
