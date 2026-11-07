//! `--service`：系统服务（SYSTEM，会话 0）。
//!
//! 只做一件事：在当前的控制台会话里以 SYSTEM 身份运行被控端（`airhand.exe --agent`，见 [`super::agent`]），并看护它：
//!
//! * 控制台会话变了（切换用户、注销后回到登录界面）就把被控端挪过去；
//! * 被控端意外退出就重新启动（退避 1 → 30 秒，运行超过一分钟后从头算）；
//! * 服务停止时通知被控端退出（它会断开控制端、关闭到服务器的连接），5 秒内没有退出就强制结束。
//!
//! 被控端监视服务进程，服务进程意外退出时它也退出，不会留下没人管的被控端。

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{error, info, warn};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_CALL_NOT_IMPLEMENTED, HANDLE, NO_ERROR, WAIT_OBJECT_0,
};
use windows::Win32::Security::{
    DuplicateTokenEx, SECURITY_ATTRIBUTES, SecurityImpersonation, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ALL_ACCESS, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary, TokenSessionId,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SERVICE_ACCEPT_SESSIONCHANGE, SERVICE_ACCEPT_SHUTDOWN,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SESSIONCHANGE,
    SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING, SERVICE_START_PENDING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOP_PENDING,
    SERVICE_STOPPED, SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS, SetServiceStatus,
    StartServiceCtrlDispatcherW,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessAsUserW,
    GetCurrentProcess, GetCurrentProcessId, OpenProcessToken, PROCESS_INFORMATION, STARTUPINFOW,
    SetEvent, TerminateProcess, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use super::SERVICE_NAME;

/// 句柄，Drop 时关闭
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// 内核对象句柄可以在线程之间使用
unsafe impl Send for Owned {}
unsafe impl Sync for Owned {}

struct Globals {
    status: SendHandle,
    /// 服务要停止了（手动重置）
    stop: Owned,
    /// 会话有变化（自动重置）
    session: Owned,
}

struct SendHandle(SERVICE_STATUS_HANDLE);
unsafe impl Send for SendHandle {}
unsafe impl Sync for SendHandle {}

static GLOBALS: OnceLock<Globals> = OnceLock::new();

/// 由服务管理器启动时的入口
pub fn run() -> i32 {
    super::init_debug_logging();
    let mut name: Vec<u16> = SERVICE_NAME.encode_utf16().chain([0]).collect();
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // 阻塞到服务停止
    match unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } {
        Ok(()) => 0,
        Err(e) => {
            error!("--service is only for the service manager: {e}");
            eprintln!("--service is only for the service manager ({e})");
            1
        }
    }
}

fn report(state: SERVICE_STATUS_CURRENT_STATE, wait_hint: Duration) {
    static CHECKPOINT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    let Some(g) = GLOBALS.get() else { return };
    let pending = state == SERVICE_START_PENDING || state == SERVICE_STOP_PENDING;
    let st = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING {
            SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN | SERVICE_ACCEPT_SESSIONCHANGE
        } else {
            0
        },
        dwWin32ExitCode: NO_ERROR.0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: if pending {
            CHECKPOINT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        } else {
            0
        },
        dwWaitHint: wait_hint.as_millis() as u32,
    };
    unsafe {
        let _ = SetServiceStatus(g.status.0, &st);
    }
}

unsafe extern "system" fn handler(
    control: u32,
    _event_type: u32,
    _data: *mut core::ffi::c_void,
    _context: *mut core::ffi::c_void,
) -> u32 {
    let Some(g) = GLOBALS.get() else {
        return NO_ERROR.0;
    };
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            report(SERVICE_STOP_PENDING, Duration::from_secs(10));
            unsafe {
                let _ = SetEvent(g.stop.0);
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_SESSIONCHANGE => {
            unsafe {
                let _ = SetEvent(g.session.0);
            }
            NO_ERROR.0
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR.0,
        _ => ERROR_CALL_NOT_IMPLEMENTED.0,
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let setup = || -> Result<()> {
        let stop = Owned(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?);
        let session = Owned(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }?);
        let status = unsafe {
            RegisterServiceCtrlHandlerExW(&HSTRING::from(SERVICE_NAME), Some(handler), None)
        }
        .context("RegisterServiceCtrlHandlerEx")?;
        let _ = GLOBALS.set(Globals {
            status: SendHandle(status),
            stop,
            session,
        });
        Ok(())
    };
    if let Err(e) = setup() {
        error!("can't start the service: {e:#}");
        return;
    }
    report(SERVICE_RUNNING, Duration::ZERO);
    info!("service started (version {})", crate::config::VERSION);

    // 按 config.toml 同步开机自动启动（手动改过配置文件的话以文件为准）
    if let Err(e) = super::apply_autostart(crate::config::settings().autostart) {
        warn!("can't apply the autostart setting: {e:#}");
    }

    let g = GLOBALS.get().expect("set above");
    supervise(&g.stop, &g.session);
    info!("service stopped");
    report(SERVICE_STOPPED, Duration::ZERO);
}

/// 运行中的被控端进程
struct Agent {
    process: Owned,
    /// 通知它退出的事件（被控端继承了这个句柄）
    stop: Owned,
    session: u32,
    started: Instant,
}

impl Agent {
    fn exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 }
    }

    /// 通知被控端退出，等它一会儿，还不退出就强制结束
    fn stop(self) {
        unsafe {
            let _ = SetEvent(self.stop.0);
            if WaitForSingleObject(self.process.0, 5000) != WAIT_OBJECT_0 {
                warn!("the host didn't exit in time, terminating it");
                let _ = TerminateProcess(self.process.0, 1);
                WaitForSingleObject(self.process.0, 2000);
            }
        }
    }
}

const NO_SESSION: u32 = 0xFFFF_FFFF;

fn supervise(stop: &Owned, session_changed: &Owned) {
    let mut agent: Option<Agent> = None;
    let mut backoff = Duration::from_secs(1);
    let mut next_try = Instant::now();
    loop {
        // 0xFFFFFFFF：正在切换会话；会话 0 不能显示界面
        let console = unsafe { WTSGetActiveConsoleSessionId() };
        if let Some(a) = &agent {
            if a.exited() {
                let ran = a.started.elapsed();
                warn!(session = a.session, "the host exited after {:.0?}", ran);
                if ran > Duration::from_secs(60) {
                    backoff = Duration::from_secs(1);
                }
                next_try = Instant::now() + backoff;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                agent = None;
            } else if console != NO_SESSION && console != 0 && a.session != console {
                info!(
                    from = a.session,
                    to = console,
                    "the console session changed, moving the host"
                );
                if let Some(a) = agent.take() {
                    a.stop();
                }
                next_try = Instant::now();
            }
        }
        if agent.is_none() && console != NO_SESSION && console != 0 && Instant::now() >= next_try {
            match spawn_agent(console) {
                Ok(a) => {
                    info!(session = console, "host started");
                    agent = Some(a);
                }
                Err(e) => {
                    warn!(session = console, "can't start the host: {e:#}");
                    next_try = Instant::now() + backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }

        let mut handles = vec![stop.0, session_changed.0];
        if let Some(a) = &agent {
            handles.push(a.process.0);
        }
        let r = unsafe { WaitForMultipleObjects(&handles, false, 1000) };
        if r == WAIT_OBJECT_0 {
            break;
        }
    }
    if let Some(a) = agent {
        info!("stopping the host");
        a.stop();
    }
}

/// 以 SYSTEM 身份（复制服务自己的令牌，改成目标会话）在 `session` 的 `winsta0\default` 桌面上运行 `airhand.exe --agent`
fn spawn_agent(session: u32) -> Result<Agent> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE
                | TOKEN_QUERY
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            &mut token,
        )
        .context("OpenProcessToken")?;
        let token = Owned(token);
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            token.0,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )
        .context("DuplicateTokenEx")?;
        let primary = Owned(primary);
        SetTokenInformation(
            primary.0,
            TokenSessionId,
            &session as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
        .context("SetTokenInformation(TokenSessionId)")?;

        // 被控端继承这个事件的句柄，服务用它通知被控端退出
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        let stop =
            Owned(CreateEventW(Some(&sa), true, false, PCWSTR::null()).context("CreateEvent")?);

        let exe = std::env::current_exe()?;
        let dir = exe.parent().context("no parent directory")?.to_path_buf();
        let mut cmd: Vec<u16> = format!(
            "\"{}\" --agent --stop-event {} --parent {}",
            exe.display(),
            stop.0.0 as usize,
            GetCurrentProcessId()
        )
        .encode_utf16()
        .chain([0])
        .collect();
        let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain([0]).collect();
        let si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();
        CreateProcessAsUserW(
            Some(primary.0),
            &HSTRING::from(exe.as_os_str()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
            None,
            &HSTRING::from(dir.as_os_str()),
            &si,
            &mut pi,
        )
        .context("CreateProcessAsUser")?;
        let _ = CloseHandle(pi.hThread);
        Ok(Agent {
            process: Owned(pi.hProcess),
            stop,
            session,
            started: Instant::now(),
        })
    }
}
