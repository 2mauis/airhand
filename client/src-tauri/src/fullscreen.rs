//! 控制台窗口全屏。
//!
//! 全屏时标签栏藏起来，工具栏悬浮在顶端，鼠标碰到屏幕顶端时出现（见界面的 SessionView），
//! 所以全屏期间网页必须能收到屏幕最顶端的鼠标移动。Windows / Linux 的控制台是无边框窗口，
//! Tauri 为了能拖动窗口边缘改大小，会拦下窗口边缘一圈的鼠标事件，全屏时也照拦不误：
//!
//! * Windows：窗口上盖了一层透明子窗口（`TAURI_DRAG_RESIZE_WINDOW`），挡住顶端的几个像素。
//!   窗口变为"不可调整大小"时 Tauri 会拿掉它，所以全屏期间关掉可调整大小；
//! * Linux：webview 的 `motion-notify-event` 上接了处理函数，鼠标在边缘 5px（乘以缩放）以内时
//!   换成改大小的光标并吞掉事件。全屏期间暂停这个信号上接的处理函数（WebKit 自己处理鼠标
//!   用的是类的默认处理，不受影响）。不能用关掉可调整大小的办法：GTK 会把不可调整大小的窗口
//!   固定成最小尺寸，全屏窗口可能缩小；
//! * macOS：保留了系统标题栏，没有这个问题（系统菜单栏盖住顶端的情况由界面处理）。
//!
//! 顺序：先全屏再停用边缘，先恢复边缘再退出全屏。Windows 退出全屏时如果窗口回到最大化，
//! 要靠退出全屏引起的 WM_SIZE 让 Tauri 收起拖动区，所以要先恢复。

use tauri::WebviewWindow;

pub fn set(window: &WebviewWindow, on: bool) -> tauri::Result<()> {
    if on {
        window.set_fullscreen(true)?;
        pause_edge_resize(window, true)
    } else {
        pause_edge_resize(window, false)?;
        window.set_fullscreen(false)
    }
}

#[cfg(windows)]
fn pause_edge_resize(window: &WebviewWindow, paused: bool) -> tauri::Result<()> {
    window.set_resizable(!paused)
}

#[cfg(target_os = "linux")]
fn pause_edge_resize(window: &WebviewWindow, paused: bool) -> tauri::Result<()> {
    use glib::gobject_ffi;
    use glib::prelude::*;
    use glib::translate::IntoGlib;

    // 记在 webview 上：屏蔽和解除要配对（重复全屏、控制台关掉重开都不会弄乱）
    const PAUSED: &str = "airhand-edge-resize-paused";

    window.with_webview(move |wv| {
        let view = wv.inner();
        let obj = view.upcast_ref::<glib::Object>();
        // SAFETY: 只在主线程上读写自己的 key，类型始终是 bool
        let was = unsafe { obj.data::<bool>(PAUSED).is_some_and(|p| *p.as_ref()) };
        if was == paused {
            return;
        }
        unsafe {
            obj.set_data(PAUSED, paused);
            let signal = gobject_ffi::g_signal_lookup(
                c"motion-notify-event".as_ptr(),
                obj.type_().into_glib(),
            );
            if signal == 0 {
                return;
            }
            let block_or_unblock = if paused {
                gobject_ffi::g_signal_handlers_block_matched
            } else {
                gobject_ffi::g_signal_handlers_unblock_matched
            };
            use std::ptr::null_mut;
            block_or_unblock(
                obj.as_ptr().cast(),
                gobject_ffi::G_SIGNAL_MATCH_ID,
                signal,
                0,
                null_mut(),
                null_mut(),
                null_mut(),
            );
        }
    })
}

#[cfg(target_os = "macos")]
fn pause_edge_resize(_window: &WebviewWindow, _paused: bool) -> tauri::Result<()> {
    Ok(())
}
