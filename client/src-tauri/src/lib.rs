//! Airhand 客户端：同一个二进制既是被控端也是控制端。
//!
//! * 启动后默认是**被控端**：只有托盘图标，没有任何窗口；
//! * 托盘菜单可切换为**控制端**：打开现代化控制台，可同时控制多台电脑；
//! * Windows 需要安装：被控端作为系统服务运行（见 `winsvc`），托盘只负责显示和操作；
//!   调试构建直接运行时被控端在本进程里运行。macOS / Linux 的被控端一直在本进程里运行；
//! * 配置：Windows 是安装目录下的 `config.toml`（见 [`config`]）；macOS / Linux 没有配置文件，
//!   运行过程中不在磁盘上留下任何文件（见 [`ephemeral`]）；
//! * 界面支持中文 / 英文，默认跟随系统语言（见 [`i18n`]）。

// 最先声明：其它模块都要用到 `tr!`
#[macro_use]
mod i18n;
mod clipboard;
mod config;
#[cfg(test)]
mod e2e;
mod ephemeral;
pub mod host;
mod single_instance;
pub mod status;
mod sysinfo;
mod tray;
#[cfg(windows)]
mod winsvc;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_link::controller::{
    ControllerService, Frontend, HostOnline, SessionEvent, SessionInfo, SessionUi, UiClosed,
};
use airhand_link::{Problem, ServerState};
use airhand_proto::session::{InputEvent, QualitySettings};
use airhand_proto::signal::ResumeToken;
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tracing::{info, warn};

use host::{HostCmd, HostService};
use i18n::Lang;
use status::{HostStatus, Mode, StatusHub};

const CONSOLE: &str = "main";

pub struct AppState {
    hub: StatusHub,
    services: Mutex<Services>,
    webview_dirs: Mutex<Vec<PathBuf>>,
    /// Windows 安装版：被控端在系统服务里运行（托盘只负责显示和操作），切换模式不影响它
    service_mode: bool,
}

#[derive(Default)]
struct Services {
    host: Option<Hosting>,
    controller: Option<Arc<ControllerService>>,
}

/// 被控端在哪里运行
enum Hosting {
    /// 本进程里
    InProcess(HostService),
    /// Windows 安装版：系统服务里，托盘经命名管道和它通信
    #[cfg(windows)]
    Service(winsvc::ipc::AgentLink),
}

impl Hosting {
    fn send(&self, cmd: HostCmd) {
        match self {
            Hosting::InProcess(h) => h.send(cmd),
            #[cfg(windows)]
            Hosting::Service(link) => link.send(match cmd {
                HostCmd::KickAll => winsvc::ipc::ToAgent::KickAll,
                HostCmd::NewCode => winsvc::ipc::ToAgent::NewCode,
            }),
        }
    }
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

/// 切换后台服务（不涉及窗口）。被控端在系统服务里运行时（Windows 安装版）只启停控制端。
fn switch_services(app: &AppHandle, mode: Mode) {
    let state = app.state::<AppState>();
    let current = state.hub.borrow().mode;
    if current != mode {
        info!(?mode, "switching mode");
        let in_process = !state.service_mode;
        let mut sv = state.services.lock().unwrap();
        // 先停止旧服务（Drop 即中止并断开连接），再启动新服务
        sv.controller = None;
        if in_process {
            sv.host = None;
        }
        state.hub.send_modify(|s| {
            s.mode = mode;
            s.controller = ServerState::Connecting;
            if in_process {
                s.host = HostStatus::default();
            }
        });
        match mode {
            Mode::Host if in_process => {
                sv.host = Some(Hosting::InProcess(HostService::start(state.hub.clone())));
                #[cfg(target_os = "macos")]
                host::macos::request_permissions();
            }
            Mode::Host => {}
            Mode::Controller => {
                // 在等的被控端上线了：告诉控制台，等它的会话马上重连
                let handle = app.clone();
                let host_online: HostOnline = Arc::new(move |code| {
                    let _ = handle.emit_to(CONSOLE, "host-online", code);
                });
                sv.controller = Some(Arc::new(ControllerService::start(
                    config::link(),
                    tauri::async_runtime::handle().inner().clone(),
                    status::controller_reporter(&state.hub),
                    host_online,
                )))
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

/// macOS：是否在程序坞里显示图标（也决定能不能用 Command+Tab 切换到控制台）。
/// 只有托盘图标时是"辅助程序"，打开控制台时变成普通程序。
#[cfg(target_os = "macos")]
fn show_in_dock(app: &AppHandle, visible: bool) {
    let policy = if visible {
        tauri::ActivationPolicy::Regular
    } else {
        tauri::ActivationPolicy::Accessory
    };
    let _ = app.set_activation_policy(policy);
}

fn open_console(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    show_in_dock(app, true);
    if let Some(w) = app.get_webview_window(CONSOLE) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let app = app.clone();
    // Windows 上在主线程同步创建 WebView2 可能死锁，放到异步任务中创建
    tauri::async_runtime::spawn(async move {
        let builder =
            WebviewWindowBuilder::new(&app, CONSOLE, WebviewUrl::App("index.html".into()));
        // WebView2 / WebKitGTK 需要一个数据目录（放在临时目录里，用完删除，见 ephemeral）；
        // macOS 的 WKWebView 没有这个概念，隐私模式下本来就不往磁盘写东西
        #[cfg(not(target_os = "macos"))]
        let builder = {
            let dir = ephemeral::new_dir();
            app.state::<AppState>()
                .webview_dirs
                .lock()
                .unwrap()
                .push(dir.clone());
            builder.data_directory(dir)
        };
        let built = builder
            .title(console_title())
            // 首屏就用当前语言（之后的切换通过 lang-changed 事件通知）
            .initialization_script(format!(
                "window.__AIRHAND_LANG__ = {:?};",
                i18n::current().code()
            ))
            .inner_size(1360.0, 860.0)
            .min_inner_size(960.0, 620.0)
            .center()
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
    #[cfg(target_os = "macos")]
    show_in_dock(app, false);
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
                HostCmd::KickAll
            } else {
                HostCmd::NewCode
            };
            if let Some(h) = &app.state::<AppState>().services.lock().unwrap().host {
                h.send(cmd);
            }
        }
        #[cfg(windows)]
        "autostart" => {
            let state = app.state::<AppState>();
            let current = state.hub.borrow().service.map(|s| s.autostart);
            if let (Some(current), Some(Hosting::Service(link))) =
                (current, &state.services.lock().unwrap().host)
            {
                link.send(winsvc::ipc::ToAgent::SetAutostart { on: !current });
            }
            // 勾选项被点击时系统会自动翻转勾选状态：重建菜单，以被控端确认后的设置为准
            state.hub.send_modify(|_| {});
        }
        #[cfg(windows)]
        "start_service" => {
            std::thread::spawn(|| {
                if let Err(e) = winsvc::start() {
                    warn!("can't start the Airhand service: {e:#}");
                }
            });
        }
        #[cfg(target_os = "macos")]
        "perm_screen" => {
            host::macos::open_privacy_settings(host::macos::PrivacyPane::ScreenRecording)
        }
        #[cfg(target_os = "macos")]
        "perm_input" => host::macos::open_privacy_settings(host::macos::PrivacyPane::Accessibility),
        "copy_code" => {
            if let Some(code) = app.state::<AppState>().hub.borrow().host.code {
                match clipboard::set_text(&format!("{code:06}")) {
                    Ok(()) => info!("control code copied"),
                    Err(e) => warn!("copying failed: {e}"),
                }
            }
        }
        "quit" => {
            let state = app.state::<AppState>();
            // Windows 安装版：退出 = 被控端也停止（和直接运行时一样，退出后本机不能再被控制）
            #[cfg(windows)]
            if state.service_mode
                && let Err(e) = winsvc::stop()
            {
                warn!("can't stop the Airhand service: {e:#}");
            }
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
    server: String,
    pinned: bool,
    hostname: String,
    lang: Lang,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: config::VERSION,
        server: config::settings().server.clone(),
        pinned: config::pinned(),
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
    state.hub.borrow().controller.clone()
}

/// 会话的画面和事件经 Tauri 的 Channel 交给控制台窗口
struct ConsoleUi {
    video: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
}

impl SessionUi for ConsoleUi {
    fn packet(&self, packet: Vec<u8>) -> Result<(), UiClosed> {
        self.video
            .send(InvokeResponseBody::Raw(packet))
            .map_err(|_| UiClosed)
    }

    fn event(&self, event: SessionEvent) -> Result<(), UiClosed> {
        self.events.send(event).map_err(|_| UiClosed)
    }
}

#[tauri::command]
async fn ctrl_connect(
    state: tauri::State<'_, AppState>,
    code: String,
    resume: Option<ResumeToken>,
    settings: QualitySettings,
    video: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    clipboard: bool,
) -> Result<SessionInfo, Problem> {
    let code = airhand_proto::parse_code(&code).ok_or(Problem::InvalidCode)?;
    let svc = state.controller()?;
    let ui = Arc::new(ConsoleUi { video, events });
    let frontend = Frontend::Desktop {
        clipboard: Box::new(clipboard::ClipSync::new()),
        sync: clipboard,
    };
    svc.connect(code, resume, settings, ui, frontend).await
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
    // Windows：安装包和服务管理器用的命令行（--install、--service、--agent……）
    #[cfg(windows)]
    if let Some(code) = winsvc::cli() {
        std::process::exit(code);
    }
    init_logging();
    let Some(_instance) = single_instance::acquire() else {
        // 已经有一个实例在运行（托盘里），直接退出
        return;
    };
    // Windows：安装好的 Airhand 由系统服务运行被控端；发布版必须安装，调试构建直接运行时被控端在本进程里运行
    #[cfg(windows)]
    let service_mode = winsvc::installed_here();
    #[cfg(all(windows, not(debug_assertions)))]
    if !service_mode {
        winsvc::show_not_installed();
        return;
    }
    #[cfg(not(windows))]
    let service_mode = false;
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
            service_mode,
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
            let hosting = if service_mode {
                start_service_link(&handle, &hub)
            } else {
                Hosting::InProcess(HostService::start(hub.clone()))
            };
            app.state::<AppState>().services.lock().unwrap().host = Some(hosting);
            // 仅调试构建：方便开发时直接进入控制端
            let dev_controller =
                cfg!(debug_assertions) && std::env::var_os("AIRHAND_DEV_CONTROLLER").is_some();
            if dev_controller {
                set_mode(&handle, Mode::Controller);
            }
            #[cfg(target_os = "macos")]
            {
                // 直接进入控制端时用不到被控端的权限，切回被控端时再请求
                if !dev_controller {
                    host::macos::request_permissions();
                }
                watch_permissions(hub.clone());
            }

            // 状态（或界面语言）变化 → 刷新托盘 / 通知控制台
            let mut rx = hub.subscribe();
            tauri::async_runtime::spawn(async move {
                let mut lang = None;
                loop {
                    let st = rx.borrow_and_update().clone();
                    tray::refresh(&handle, &st);
                    if st.mode == Mode::Controller {
                        let _ = handle.emit_to(CONSOLE, "server-status", &st.controller);
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
    // macOS：和其它平台一样启动后只有托盘（菜单栏）图标，程序坞里没有图标；打开控制台时才显示。
    // 打包的 .app 在 Info.plist 里声明了 LSUIElement，这里是给直接运行的可执行文件用的
    #[cfg(target_os = "macos")]
    let app = {
        let mut app = app;
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
        app
    };

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

/// Windows 安装版：被控端在系统服务里运行。打开 Airhand 时服务没有运行就启动它；服务停止后托盘也退出。
#[cfg(windows)]
fn start_service_link(app: &AppHandle, hub: &StatusHub) -> Hosting {
    hub.send_modify(|s| {
        s.service = Some(status::ServiceStatus {
            connected: false,
            running: false,
            elsewhere: false,
            autostart: config::settings().autostart,
        })
    });
    std::thread::spawn(|| {
        if let Err(e) = winsvc::start() {
            warn!("can't start the Airhand service: {e:#}");
        }
    });
    let app = app.clone();
    Hosting::Service(winsvc::ipc::AgentLink::start(hub.clone(), move || {
        app.exit(0)
    }))
}

#[cfg(not(windows))]
fn start_service_link(_app: &AppHandle, _hub: &StatusHub) -> Hosting {
    unreachable!("the host only runs as a service on Windows")
}

/// macOS：用户在系统设置里授予 / 收回权限时刷新托盘菜单（菜单里有未授权的提示）。
#[cfg(target_os = "macos")]
fn watch_permissions(hub: StatusHub) {
    tauri::async_runtime::spawn(async move {
        let mut last = host::macos::permissions();
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let now = host::macos::permissions();
            if now != last {
                info!(
                    screen = now.screen,
                    input = now.input,
                    "macOS permissions changed"
                );
                last = now;
                hub.send_modify(|_| {});
            }
        }
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

pub(crate) fn enable_dpi_awareness() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        // 清单中已声明；这里再设置一次以防万一（已设置时会返回错误，忽略即可）
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}
