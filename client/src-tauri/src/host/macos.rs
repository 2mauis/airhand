//! macOS 被控端的公共部分：系统权限、显示器枚举、CoreMedia 时间换算、线程优先级。
//!
//! macOS 上采集屏幕 / 系统声音需要"屏幕与系统录音"权限，注入键鼠需要"辅助功能"权限，
//! 都只能由本机用户在"系统设置 → 隐私与安全性"里授予。被控端启动时会请求一次（系统弹出对话框），
//! 没有授权时托盘菜单里会一直提示，并且可以直接打开对应的设置页面。

use std::ffi::c_void;
use std::sync::Once;
use std::time::Instant;

use airhand_proto::session::DisplayInfo;
use anyhow::{Result, anyhow};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFString};
use objc2_core_graphics::{
    CGDirectDisplayID, CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayIsMain,
    CGDisplayMirrorsDisplay, CGDisplayMode, CGError, CGGetActiveDisplayList,
};
use objc2_core_media::{CMClock, CMTime, CMTimeFlags};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> u8;
    static kAXTrustedCheckOptionPrompt: *const c_void;
}

/// 被控端需要的系统权限是否已经授予。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    /// 屏幕与系统录音（采集画面和声音）
    pub screen: bool,
    /// 辅助功能（注入键盘和鼠标）
    pub input: bool,
}

pub fn permissions() -> Permissions {
    Permissions {
        screen: screen_allowed(),
        input: input_allowed(),
    }
}

/// 能否采集屏幕和系统声音
pub fn screen_allowed() -> bool {
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// 能否注入键盘和鼠标事件
pub fn input_allowed() -> bool {
    unsafe { AXIsProcessTrusted() != 0 || CGPreflightPostEventAccess() }
}

/// 请求还没有授予的权限（系统弹出对话框）。每次运行只请求一次：
/// 对话框里点了"拒绝"之后，系统不会再弹，需要用户自己去系统设置里打开。
pub fn request_permissions() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let p = permissions();
        tracing::info!(screen = p.screen, input = p.input, "macOS permissions");
        unsafe {
            if !p.screen {
                CGRequestScreenCaptureAccess();
            }
            if !p.input {
                let key = &*(kAXTrustedCheckOptionPrompt as *const CFString);
                let options = CFDictionary::<CFString, CFBoolean>::from_slices(
                    &[key],
                    &[CFBoolean::new(true)],
                );
                AXIsProcessTrustedWithOptions(
                    objc2_core_foundation::CFRetained::as_ptr(&options)
                        .as_ptr()
                        .cast(),
                );
            }
        }
    });
}

/// 系统设置里的隐私页面
#[derive(Debug, Clone, Copy)]
pub enum PrivacyPane {
    ScreenRecording,
    Accessibility,
}

/// 打开"系统设置 → 隐私与安全性"里对应的页面。
pub fn open_privacy_settings(pane: PrivacyPane) {
    let anchor = match pane {
        PrivacyPane::ScreenRecording => "Privacy_ScreenCapture",
        PrivacyPane::Accessibility => "Privacy_Accessibility",
    };
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{anchor}");
    // 在后台等 open 退出（不等的话会留下僵尸进程），不阻塞菜单事件
    std::thread::spawn(move || {
        if let Err(e) = std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
        {
            tracing::warn!("can't open System Settings: {e}");
        }
    });
}

/// 没有"屏幕与系统录音"权限，无法采集画面和声音。
///
/// 会话已经用 [`Notice::MissingPermissions`](airhand_proto::session::Notice::MissingPermissions)
/// 告诉控制端了，所以采集线程认出这个错误后不再把它当作采集失败发过去（见 [`super::missing_permission`]）。
#[derive(Debug)]
pub struct PermissionDenied;

impl std::fmt::Display for PermissionDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the Screen & System Audio Recording permission isn't granted (System Settings → Privacy & Security)",
        )
    }
}

impl std::error::Error for PermissionDenied {}

pub fn check_screen_permission() -> Result<()> {
    if !screen_allowed() {
        return Err(PermissionDenied.into());
    }
    Ok(())
}

// ───────────────────────────── 显示器 ─────────────────────────────

/// 一个显示器。macOS 的全局坐标以"点"为单位（Retina 屏 1 点 = 2 像素），采集按物理像素。
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    pub id: CGDirectDisplayID,
    /// 在全局坐标系中的位置和大小（点，原点是主显示器左上角，y 向下）
    pub x: f64,
    pub y: f64,
    pub points_w: f64,
    pub points_h: f64,
    /// 物理像素
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

impl Display {
    /// 每点多少像素
    pub fn scale(&self) -> f64 {
        if self.points_w > 0.0 {
            self.width as f64 / self.points_w
        } else {
            1.0
        }
    }
}

/// 列出所有显示器：主显示器排第一，其余按坐标从左到右。镜像显示器只算一个。
pub fn displays() -> Vec<Display> {
    let mut ids = [0 as CGDirectDisplayID; 32];
    let mut n = 0u32;
    if unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) }
        != CGError::Success
    {
        return Vec::new();
    }
    let mut list: Vec<Display> = ids[..(n as usize).min(ids.len())]
        .iter()
        .filter(|&&id| CGDisplayMirrorsDisplay(id) == 0)
        .map(|&id| {
            let b = CGDisplayBounds(id);
            let mode = CGDisplayCopyDisplayMode(id);
            let (pw, ph) = mode
                .as_deref()
                .map(|m| {
                    (
                        CGDisplayMode::pixel_width(Some(m)),
                        CGDisplayMode::pixel_height(Some(m)),
                    )
                })
                .filter(|&(w, h)| w > 0 && h > 0)
                .unwrap_or((b.size.width as usize, b.size.height as usize));
            Display {
                id,
                x: b.origin.x,
                y: b.origin.y,
                points_w: b.size.width,
                points_h: b.size.height,
                width: pw as u32,
                height: ph as u32,
                primary: CGDisplayIsMain(id),
            }
        })
        .filter(|d| d.width > 0 && d.height > 0)
        .collect();
    list.sort_by(|a, b| {
        (!a.primary, a.x, a.y)
            .partial_cmp(&(!b.primary, b.x, b.y))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !list.iter().any(|d| d.primary)
        && let Some(first) = list.first_mut()
    {
        first.primary = true;
    }
    list
}

/// 第 `index` 个显示器（与 [`displays`] 的顺序一致）。
pub fn display(index: u8) -> Result<Display> {
    displays()
        .into_iter()
        .nth(index as usize)
        .ok_or_else(|| anyhow!("display {index} doesn't exist"))
}

/// 发给控制端的显示器列表：位置是全局坐标（点），宽高是采集的物理像素。
pub fn display_infos() -> Vec<DisplayInfo> {
    displays()
        .into_iter()
        .enumerate()
        .map(|(i, d)| DisplayInfo {
            index: i as u8,
            name: if d.primary {
                "Main".into()
            } else {
                format!("Display {}", d.id)
            },
            x: d.x.round() as i32,
            y: d.y.round() as i32,
            width: d.width,
            height: d.height,
            primary: d.primary,
        })
        .collect()
}

// ───────────────────────────── 时间 ─────────────────────────────

/// CoreMedia 时间戳（ScreenCaptureKit 的画面帧和声音包都用主机时钟）对应的时刻。
pub fn instant_from_host_time(t: CMTime) -> Option<Instant> {
    if !t.flags.contains(CMTimeFlags::Valid) || t.timescale <= 0 {
        return None;
    }
    let now = unsafe { CMClock::host_time_clock().time() };
    let age = unsafe { now.seconds() - t.seconds() };
    crate::host::clock::ago(age)
}

/// 采集、编码、声音线程提到"用户交互"级别：Apple 芯片上不会被放到能效核上，延迟更稳定。
pub fn raise_thread_qos() {
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_displays() {
        let list = displays();
        println!("{list:#?}");
        // 在没有显示器的机器上（例如 CI）列表可以为空，但有的话主显示器一定排第一
        if let Some(first) = list.first() {
            assert!(first.primary);
            assert!(first.scale() >= 1.0);
            assert_eq!(display(0).unwrap(), *first);
        }
        assert!(display(200).is_err());
    }

    /// "缺少权限"包在别的错误说明里也要认得出来（采集线程据此不把它当作采集失败）
    #[test]
    fn permission_error_is_recognized() {
        let e = anyhow::Error::from(PermissionDenied).context("opening the capture");
        assert!(crate::host::missing_permission(&e));
        assert!(!crate::host::missing_permission(&anyhow!(
            "SCStreamErrorDomain -3801"
        )));
    }

    #[test]
    fn host_time_conversion() {
        let now = unsafe { CMClock::host_time_clock().time() };
        // 100 ms 之前
        let earlier = unsafe { CMTime::new(now.value - now.timescale as i64 / 10, now.timescale) };
        let t = instant_from_host_time(earlier).unwrap();
        let ago = t.elapsed().as_secs_f64();
        assert!((0.099..0.11).contains(&ago), "{ago}");
        let invalid = CMTime {
            flags: CMTimeFlags(0),
            ..now
        };
        assert!(instant_from_host_time(invalid).is_none());
    }
}
