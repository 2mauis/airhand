//! Airhand 客户端：同一个二进制既是被控端也是控制端。
//!
//! * 启动后默认是**被控端**：只有托盘图标，没有任何窗口；
//! * 托盘菜单可切换为**控制端**：打开现代化控制台，可同时控制多台电脑；
//! * 没有配置文件，运行过程中不在磁盘上留下任何文件（见 [`ephemeral`]）；
//! * 界面支持中文 / 英文，默认跟随系统语言（见 [`i18n`]）。

// 最先声明：其它模块都要用到 `tr!`
#[macro_use]
mod i18n;
mod clipboard;
mod config;
mod controller;
#[cfg(test)]
mod e2e;
mod ephemeral;
pub mod host;
mod net;
mod problem;
mod single_instance;
pub mod status;
mod sysinfo;
mod tray;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_proto::session::{InputEvent, QualitySettings};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tracing::{info, warn};

use controller::session::SessionEvent;
use controller::{ControllerService, SessionInfo};
use host::HostService;
use i18n::Lang;
use problem::Problem;
use status::{Mode, ServerState, StatusHub};

const CONSOLE: &str = "main";

pub struct AppState {
    hub: StatusHub,
    services: Mutex<Services>,
    webview_dirs: Mutex<Vec<PathBuf>>,
}

#[derive(Default)]
struct Services {
    host: Option<HostService>,
    controller: Option<Arc<ControllerService>>,
}

impl AppState {
    fn controller(&self) -> Result<Arc<ControllerService>, Problem> {
        self.services
            .lock()
            .unwrap()
            .controller
            .clone()
            .ok_or(Problem::NotController)
    }
}

// ───────────────────────────── 模式切换 ─────────────────────────────

/// 切换后台服务（不涉及窗口）。
fn switch_services(app: &AppHandle, mode: Mode) {
    let state = app.state::<AppState>();
    let current = state.hub.borrow().mode;
    if current != mode {
        info!(?mode, "switching mode");
        let mut sv = state.services.lock().unwrap();
        // 先停止旧服务（Drop 即中止并断开连接），再启动新服务
        sv.host = None;
        sv.controller = None;
        state.hub.send_modify(|s| {
            s.mode = mode;
            s.server = ServerState::Connecting;
            s.code = None;
            s.controllers.clear();
        });
        match mode {
            Mode::Host => sv.host = Some(HostService::start(state.hub.clone())),
            Mode::Controller => {
                sv.controller = Some(Arc::new(ControllerService::start(state.hub.clone())))
            }
        }
    }
}

fn set_mode(app: &AppHandle, mode: Mode) {
    switch_services(app, mode);
    match mode {
        Mode::Controller => open_console(app),
        Mode::Host => {
            if let Some(w) = app.get_webview_window(CONSOLE) {
                let _ = w.destroy();
            }
        }
    }
}

fn open_console(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(CONSOLE) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let app = app.clone();
    // Windows 上在主线程同步创建 WebView2 可能死锁，放到异步任务中创建
    tauri::async_runtime::spawn(async move {
        let dir = ephemeral::new_dir();
        app.state::<AppState>()
            .webview_dirs
            .lock()
            .unwrap()
            .push(dir.clone());
        let built = WebviewWindowBuilder::new(&app, CONSOLE, WebviewUrl::App("index.html".into()))
            .title(console_title())
            // 首屏就用当前语言（之后的切换通过 lang-changed 事件通知）
            .initialization_script(format!(
                "window.__AIRHAND_LANG__ = {:?};",
                i18n::current().code()
            ))
            .inner_size(1360.0, 860.0)
            .min_inner_size(960.0, 620.0)
            .center()
            .data_directory(dir)
            .incognito(true)
            .disable_drag_drop_handler()
            .background_color(tauri::window::Color(11, 13, 20, 255))
            .theme(Some(tauri::Theme::Dark))
            .build();
        match built {
            Ok(w) => {
                let _ = w.set_focus();
            }
            Err(e) => warn!("can't open the console: {e}"),
        }
    });
}

fn console_title() -> String {
    tr!("Airhand 控制台", "Airhand Console")
}

/// 切换界面语言（托盘菜单或控制台）。
fn set_lang(app: &AppHandle, lang: Lang) {
    if i18n::set(lang) {
        info!(?lang, "switching UI language");
    }
    // 总是通知状态循环刷新托盘菜单：勾选项被点击时系统会自动翻转勾选状态，需要重建菜单恢复
    app.state::<AppState>().hub.send_modify(|_| {});
}

fn on_console_destroyed(app: &AppHandle) {
    // 关闭控制台 = 回到默认的被控端模式
    switch_services(app, Mode::Host);
    let dirs: Vec<PathBuf> =
        std::mem::take(&mut *app.state::<AppState>().webview_dirs.lock().unwrap());
    for d in dirs {
        ephemeral::remove_in_background(d);
    }
}

pub(crate) fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "to_controller" => set_mode(app, Mode::Controller),
        "to_host" => set_mode(app, Mode::Host),
        "console" => open_console(app),
        "lang_zh" => set_lang(app, Lang::Zh),
        "lang_en" => set_lang(app, Lang::En),
        "kick_all" | "new_code" => {
            let cmd = if id == "kick_all" {
                host::HostCmd::KickAll
            } else {
                host::HostCmd::NewCode
            };
            if let Some(h) = &app.state::<AppState>().services.lock().unwrap().host {
                h.send(cmd);
            }
        }
        "copy_code" => {
            if let Some(code) = app.state::<AppState>().hub.borrow().code {
                match clipboard::set_text(&format!("{code:06}")) {
                    Ok(()) => info!("control code copied"),
                    Err(e) => warn!("copying failed: {e}"),
                }
            }
        }
        "quit" => {
            let state = app.state::<AppState>();
            let mut sv = state.services.lock().unwrap();
            sv.host = None;
            sv.controller = None;
            drop(sv);
            app.exit(0);
        }
        _ => {}
    }
}

pub(crate) fn on_tray_left_click(app: &AppHandle) {
    if app.state::<AppState>().hub.borrow().mode == Mode::Controller {
        open_console(app);
    }
}

// ───────────────────────────── 前端命令 ─────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    server: &'static str,
    pinned: bool,
    hostname: String,
    lang: Lang,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: config::VERSION,
        server: config::SERVER_ADDR,
        pinned: !config::SERVER_CERT_SHA256.is_empty(),
        hostname: sysinfo::host_identity().0,
        lang: i18n::current(),
    }
}

/// 控制台切换界面语言
#[tauri::command]
fn set_language(app: AppHandle, lang: Lang) {
    set_lang(&app, lang);
}

#[tauri::command]
fn server_status(state: tauri::State<'_, AppState>) -> ServerState {
    state.hub.borrow().server.clone()
}

#[tauri::command]
async fn ctrl_connect(
    state: tauri::State<'_, AppState>,
    code: String,
    settings: QualitySettings,
    video: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    clipboard: bool,
) -> Result<SessionInfo, Problem> {
    let code = airhand_proto::parse_code(&code).ok_or(Problem::InvalidCode)?;
    let svc = state.controller()?;
    svc.connect(code, settings, video, events, clipboard).await
}

#[tauri::command]
async fn ctrl_clipboard(
    state: tauri::State<'_, AppState>,
    sid: u32,
    enabled: bool,
) -> Result<(), Problem> {
    state.controller()?.set_clipboard_sync(sid, enabled)
}

/// 开启/关闭某个会话的远程声音
#[tauri::command]
async fn ctrl_audio(
    state: tauri::State<'_, AppState>,
    sid: u32,
    enabled: bool,
) -> Result<(), Problem> {
    state.controller()?.set_audio(sid, enabled)
}

#[tauri::command]
async fn ctrl_input(
    state: tauri::State<'_, AppState>,
    sid: u32,
    events: Vec<InputEvent>,
) -> Result<(), Problem> {
    state.controller()?.input(sid, events)
}

#[tauri::command]
async fn ctrl_settings(
    state: tauri::State<'_, AppState>,
    sid: u32,
    settings: QualitySettings,
) -> Result<(), Problem> {
    state.controller()?.settings(sid, settings)
}

#[tauri::command]
async fn ctrl_keyframe(state: tauri::State<'_, AppState>, sid: u32) -> Result<(), Problem> {
    state.controller()?.keyframe(sid)
}

#[tauri::command]
async fn ctrl_display(
    state: tauri::State<'_, AppState>,
    sid: u32,
    index: u8,
) -> Result<(), Problem> {
    state.controller()?.select_display(sid, index)
}

#[tauri::command]
async fn ctrl_disconnect(state: tauri::State<'_, AppState>, sid: u32) -> Result<(), Problem> {
    state.controller()?.disconnect(sid);
    Ok(())
}

/// 读取本机剪贴板文本（用于"粘贴到远端"）
#[tauri::command]
fn clipboard_text() -> Result<String, Problem> {
    clipboard::get_text().map_err(|_| Problem::NoClipboardText)
}

#[tauri::command]
async fn switch_to_host(app: AppHandle) -> Result<(), Problem> {
    let a = app.clone();
    app.run_on_main_thread(move || set_mode(&a, Mode::Host))
        .map_err(Problem::failed)
}

// ───────────────────────────── 入口 ─────────────────────────────

pub fn run() {
    init_logging();
    let Some(_instance) = single_instance::acquire() else {
        // 已经有一个实例在运行（托盘里），直接退出
        return;
    };
    enable_dpi_awareness();
    // 先清理上次残留的目录，再创建本次的（同一个前缀）
    ephemeral::sweep_stale();
    #[cfg(target_os = "linux")]
    ephemeral::isolate_caches();

    let hub = status::new_hub();
    let app = tauri::Builder::default()
        .manage(AppState {
            hub: hub.clone(),
            services: Mutex::default(),
            webview_dirs: Mutex::default(),
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            set_language,
            server_status,
            ctrl_connect,
            ctrl_input,
            ctrl_settings,
            ctrl_keyframe,
            ctrl_display,
            ctrl_disconnect,
            ctrl_clipboard,
            ctrl_audio,
            clipboard_text,
            switch_to_host,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            if tray::available() {
                tray::create(&handle)?;
            }
            #[cfg(target_os = "linux")]
            exit_on_signals(handle.clone());
            // 默认以被控端启动
            app.state::<AppState>().services.lock().unwrap().host =
                Some(HostService::start(hub.clone()));
            // 仅调试构建：方便开发时直接进入控制端
            #[cfg(debug_assertions)]
            if std::env::var_os("AIRHAND_DEV_CONTROLLER").is_some() {
                set_mode(&handle, Mode::Controller);
            }

            // 状态（或界面语言）变化 → 刷新托盘 / 通知控制台
            let mut rx = hub.subscribe();
            tauri::async_runtime::spawn(async move {
                let mut lang = None;
                loop {
                    let st = rx.borrow_and_update().clone();
                    tray::refresh(&handle, &st);
                    if st.mode == Mode::Controller {
                        let _ = handle.emit_to(CONSOLE, "server-status", &st.server);
                    }
                    let cur = i18n::current();
                    if lang.replace(cur) != Some(cur) {
                        if let Some(w) = handle.get_webview_window(CONSOLE) {
                            let _ = w.set_title(&console_title());
                        }
                        let _ = handle.emit_to(CONSOLE, "lang-changed", cur);
                    }
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == CONSOLE && matches!(event, WindowEvent::Destroyed) {
                on_console_destroyed(window.app_handle());
            }
        })
        .build(tauri::generate_context!())
        .expect("can't initialize Tauri");

    app.run(|app, event| match event {
        // 托盘程序：关闭所有窗口时不退出
        RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        RunEvent::Exit => {
            let state = app.state::<AppState>();
            {
                let mut sv = state.services.lock().unwrap();
                sv.host = None;
                sv.controller = None;
            }
            if let Some(w) = app.get_webview_window(CONSOLE) {
                let _ = w.destroy();
            }
            let dirs: Vec<PathBuf> = std::mem::take(&mut *state.webview_dirs.lock().unwrap());
            for d in dirs {
                ephemeral::remove_with_retry(&d, Duration::from_secs(3));
            }
            // Linux：托盘图标、GStreamer 缓存所在的目录（内存盘 XDG_RUNTIME_DIR 里），退出时删掉
            #[cfg(target_os = "linux")]
            ephemeral::remove_with_retry(&ephemeral::process_dir(), Duration::from_secs(1));
        }
        _ => {}
    });
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("AIRHAND_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,quinn=warn,quinn_proto=warn"));
    // 只输出到 stderr，绝不写日志文件
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

/// Linux：收到 SIGTERM（注销/关机）、SIGINT（终端里按 Ctrl+C）、SIGHUP 时正常退出，照样清理临时目录。
///
/// 信号处理函数里只做一件事：往管道里写一个字节（异步信号安全），由单独的线程读到后通知事件循环退出。
#[cfg(target_os = "linux")]
fn exit_on_signals(app: AppHandle) {
    use std::sync::atomic::{AtomicI32, Ordering};
    static PIPE: AtomicI32 = AtomicI32::new(-1);

    extern "C" fn on_signal(_: libc::c_int) {
        let fd = PIPE.load(Ordering::Relaxed);
        if fd >= 0 {
            unsafe { libc::write(fd, [1u8].as_ptr().cast(), 1) };
        }
    }

    let mut fds = [0; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return;
    }
    PIPE.store(fds[1], Ordering::Relaxed);
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        loop {
            match unsafe { libc::read(fds[0], b.as_mut_ptr().cast(), 1) } {
                1 => break,
                -1 if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {
                    continue;
                }
                _ => return,
            }
        }
        info!("received an exit signal");
        app.exit(0);
    });
}

fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        // 清单中已声明；这里再设置一次以防万一（已设置时会返回错误，忽略即可）
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
