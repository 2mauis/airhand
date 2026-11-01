//! 系统托盘：图标 + 右键菜单（模式、服务器状态、控制码、切换模式、语言、退出）。

use std::sync::OnceLock;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};

use crate::i18n::{self, Lang};
use crate::problem::Problem;
use crate::status::{AppStatus, Mode, ServerState};

const TRAY_ID: &str = "airhand";
static ICON_PNG: &[u8] = include_bytes!("../icons/tray.png");

fn icon_online() -> Image<'static> {
    static IMG: OnceLock<Image<'static>> = OnceLock::new();
    IMG.get_or_init(|| Image::from_bytes(ICON_PNG).expect("tray icon").to_owned())
        .clone()
}

/// 未连接时使用灰色图标（运行时由彩色图标去色生成，不需要额外资源文件）。
fn icon_offline() -> Image<'static> {
    static IMG: OnceLock<Image<'static>> = OnceLock::new();
    IMG.get_or_init(|| {
        let src = icon_online();
        let mut rgba = src.rgba().to_vec();
        for px in rgba.as_chunks_mut::<4>().0 {
            let l = (px[0] as u32 * 30 + px[1] as u32 * 59 + px[2] as u32 * 11) / 100;
            let l = (l * 3 / 4 + 40).min(255) as u8;
            px[0] = l;
            px[1] = l;
            px[2] = l;
            px[3] = (px[3] as u32 * 85 / 100) as u8;
        }
        Image::new_owned(rgba, src.width(), src.height())
    })
    .clone()
}

/// 能否显示托盘图标。
///
/// Linux 的托盘图标依赖 AppIndicator 库（运行时动态加载），没有安装时 tray-icon 会直接 panic
/// （发布版 panic 即退出），所以先检查一下：没有的话不显示托盘，被控端照常工作，控制码写在日志里。
pub fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let names = [
            c"libayatana-appindicator3.so.1",
            c"libappindicator3.so.1",
            c"libayatana-appindicator3.so",
            c"libappindicator3.so",
        ];
        let found = names.iter().any(|n| unsafe {
            let h = libc::dlopen(n.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL);
            if !h.is_null() {
                libc::dlclose(h);
            }
            !h.is_null()
        });
        if !found {
            tracing::warn!(
                "AppIndicator library not found, can't show the tray icon (install libayatana-appindicator3-1); the control code is logged below"
            );
        }
        found
    }
    #[cfg(not(target_os = "linux"))]
    true
}

pub fn create(app: &AppHandle) -> tauri::Result<TrayIcon> {
    let st = AppStatus::default();
    let builder = TrayIconBuilder::with_id(TRAY_ID);
    // Linux 的 AppIndicator 只接受图标文件：放到单独的临时目录里，退出时整个删掉
    #[cfg(target_os = "linux")]
    let builder = builder.temp_dir_path(crate::ephemeral::process_dir().join("tray"));
    builder
        .icon(icon_offline())
        .tooltip("Airhand")
        .menu(&build_menu(app, &st)?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| crate::on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::on_tray_left_click(tray.app_handle());
            }
        })
        .build(app)
}

fn label(app: &AppHandle, text: impl AsRef<str>) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::new(app, text, false, None::<&str>)
}

fn build_menu(app: &AppHandle, st: &AppStatus) -> tauri::Result<Menu<Wry>> {
    let menu = Menu::new(app)?;
    menu.append(&label(
        app,
        format!("Airhand  v{}", crate::config::VERSION),
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;

    let mode = match st.mode {
        Mode::Host => tr!("当前模式：被控端", "Mode: Host"),
        Mode::Controller => tr!("当前模式：控制端", "Mode: Controller"),
    };
    menu.append(&label(app, mode)?)?;
    let server = match &st.server {
        ServerState::Connecting => tr!("服务器：连接中…", "Server: connecting…"),
        ServerState::Connected => tr!("服务器：已连接 ●", "Server: connected ●"),
        // 只显示原因的种类；详细的技术信息在日志里
        ServerState::Disconnected { reason } => server_problem(reason),
    };
    menu.append(&label(app, server)?)?;

    if st.mode == Mode::Host {
        if let (ServerState::Connected, Some(code)) = (&st.server, st.code) {
            let code = airhand_proto::format_code(code);
            menu.append(&label(app, tr!("控制码：{code}", "Control code: {code}"))?)?;
            menu.append(&item(
                app,
                "copy_code",
                tr!("复制控制码", "Copy control code"),
            )?)?;
            menu.append(&item(
                app,
                "new_code",
                tr!(
                    "更换控制码（旧码立即失效）",
                    "Change control code (old code stops working)"
                ),
            )?)?;
        }
        if !st.controllers.is_empty() {
            menu.append(&PredefinedMenuItem::separator(app)?)?;
            menu.append(&label(
                app,
                format!("⚠ {}", controlled_by(st.controllers.len())),
            )?)?;
            for c in st.controllers.iter().take(5) {
                let name = if c.peer.name.is_empty() {
                    tr!("未知设备", "Unknown device")
                } else {
                    c.peer.name.clone()
                };
                menu.append(&label(app, format!("    {name} ({})", c.peer.ip))?)?;
            }
            menu.append(&item(
                app,
                "kick_all",
                tr!("断开所有控制端", "Disconnect all controllers"),
            )?)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    match st.mode {
        Mode::Host => {
            menu.append(&item(
                app,
                "to_controller",
                tr!("切换为控制端", "Switch to controller"),
            )?)?;
        }
        Mode::Controller => {
            menu.append(&item(app, "console", tr!("打开控制台", "Open console"))?)?;
            menu.append(&item(
                app,
                "to_host",
                tr!("切换为被控端", "Switch to host"),
            )?)?;
        }
    }
    menu.append(&language_menu(app)?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&item(app, "quit", tr!("退出", "Quit"))?)?;
    Ok(menu)
}

fn item(app: &AppHandle, id: &str, text: String) -> tauri::Result<MenuItem<Wry>> {
    MenuItem::with_id(app, id, text, true, None::<&str>)
}

/// 语言子菜单。标题中英并列，看不懂当前语言的人也能找到；选项各用自己的语言书写。
fn language_menu(app: &AppHandle) -> tauri::Result<Submenu<Wry>> {
    let cur = i18n::current();
    let title = if cur == Lang::Zh {
        "语言 / Language"
    } else {
        "Language / 语言"
    };
    let zh = CheckMenuItem::with_id(
        app,
        "lang_zh",
        "简体中文",
        true,
        cur == Lang::Zh,
        None::<&str>,
    )?;
    let en = CheckMenuItem::with_id(
        app,
        "lang_en",
        "English",
        true,
        cur == Lang::En,
        None::<&str>,
    )?;
    Submenu::with_items(app, title, true, &[&zh, &en])
}

fn server_problem(p: &Problem) -> String {
    match p {
        Problem::ServerUnreachable { .. } => tr!("服务器：无法连接", "Server: can't connect"),
        Problem::CertMismatch => tr!("服务器：证书指纹不匹配", "Server: certificate mismatch"),
        Problem::Incompatible { server, client } if server > client => {
            tr!(
                "服务器：版本不兼容，请更新 Airhand",
                "Server: incompatible, please update Airhand"
            )
        }
        Problem::Incompatible { .. } => tr!(
            "服务器：版本不兼容，请更新服务器",
            "Server: incompatible, please update the server"
        ),
        _ => tr!("服务器：连接已断开", "Server: disconnected"),
    }
}

fn controlled_by(n: usize) -> String {
    match n {
        1 => tr!("正在被 1 个控制端控制", "Being controlled by 1 controller"),
        _ => tr!(
            "正在被 {n} 个控制端控制",
            "Being controlled by {n} controllers"
        ),
    }
}

pub fn refresh(app: &AppHandle, st: &AppStatus) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    if let Ok(menu) = build_menu(app, st) {
        let _ = tray.set_menu(Some(menu));
    }
    let online = st.server == ServerState::Connected;
    let _ = tray.set_icon(Some(if online {
        icon_online()
    } else {
        icon_offline()
    }));
    let code_line = |code: u32| {
        let code = airhand_proto::format_code(code);
        tr!("控制码 {code}", "Control code {code}")
    };
    let tip = match (st.mode, &st.server, st.code) {
        (Mode::Host, ServerState::Connected, Some(code)) if !st.controllers.is_empty() => {
            format!(
                "Airhand · {}\n{}",
                controlled_by(st.controllers.len()),
                code_line(code)
            )
        }
        (Mode::Host, ServerState::Connected, Some(code)) => {
            format!("Airhand · {}\n{}", tr!("被控端", "Host"), code_line(code))
        }
        (Mode::Host, ServerState::Connected, None) => tr!("Airhand · 被控端", "Airhand · Host"),
        (Mode::Controller, ServerState::Connected, _) => {
            tr!("Airhand · 控制端", "Airhand · Controller")
        }
        (_, ServerState::Connecting, _) => tr!(
            "Airhand · 正在连接服务器…",
            "Airhand · Connecting to server…"
        ),
        (_, ServerState::Disconnected { .. }, _) => tr!(
            "Airhand · 未连接服务器",
            "Airhand · Not connected to server"
        ),
    };
    let _ = tray.set_tooltip(Some(tip));
}
