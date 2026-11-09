//! macOS：登录时自动启动（登录项）。
//!
//! 用 `SMAppService.mainApp`（macOS 13+）把 Airhand.app 自己登记为登录项：出现在"系统设置 → 通用 → 登录项"（有的系统版本叫"登录项与扩展"）
//! 的"登录时打开"里，用户也可以在那里关掉。不写任何文件（系统自己记着），App 挪了位置也照样找得到
//! （`~/Library/LaunchAgents` 里的 plist 写死了路径，App 一挪就失效，系统还照样显示"已启用"）。
//!
//! 别的 App 踩过的坑：
//! * 用户在系统设置里关掉之后，状态变成 `NotFound`（有的系统版本是 `RequiresApproval`），和"从来没登记过"分不出来，
//!   而这时再 `register()` 会把它**重新打开**（Postgres.app 每次启动都登记一次，用户怎么关都关不掉）。
//!   所以只在用户点菜单时、以及第一次运行时（默认打开，只做一次，见 `lib.rs`）登记，不会自己再登记；
//! * 只在"应用程序"文件夹（/Applications、~/Applications）里的 Airhand.app 上做：直接运行可执行文件（开发时）
//!   没有 App 可登记；从下载目录、磁盘映像里直接打开的 App，系统会把它挪到一个临时位置运行（App Translocation），
//!   登记了下次也找不到。
//!
//! 这个模块只依赖系统框架（不依赖本 crate 的其它部分）。调用会经 XPC 问系统，`set` 可能要等一会儿，不要在主线程上调用。

use std::path::{Path, PathBuf};

use objc2_service_management::{SMAppService, SMAppServiceStatus};

/// 登录时自动启动的状态（菜单里显示的）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 是 App，但不在"应用程序"文件夹里：先放进去才能打开
    NotInstalled,
    Off,
    On,
    /// 登记了，但要用户在系统设置里允许才会生效
    NeedsApproval,
}

/// 本程序所在的 App 包（`…/Airhand.app`）。直接运行可执行文件时（开发时）为 `None`：没有这个功能
pub fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    bundle_of(&exe.canonicalize().unwrap_or(exe))
}

/// 现在的状态；不是 App 包时为 `None`
pub fn state() -> Option<State> {
    let bundle = bundle()?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if !installed(&bundle, home.as_deref()) {
        return Some(State::NotInstalled);
    }
    Some(state_of(status()))
}

/// 打开（`on`）或关掉登录时自动启动，返回之后的状态。
///
/// `interactive`：用户点了菜单。要在系统设置里允许时直接打开那个页面（第一次运行时默认打开的那次不打开，免得吓人）
pub fn set(on: bool, interactive: bool) -> Result<State, String> {
    match state() {
        None => return Err("Airhand isn't running from an app bundle".into()),
        Some(State::NotInstalled) => return Ok(State::NotInstalled),
        Some(_) => {}
    }
    let svc = unsafe { SMAppService::mainAppService() };
    let step = step(on, unsafe { svc.status() });
    let result = match step {
        Step::Register => unsafe { svc.registerAndReturnError() },
        Step::Unregister => unsafe { svc.unregisterAndReturnError() },
        Step::OpenSettings | Step::Nothing => Ok(()),
    };
    result.map_err(|e| failed(if on { "register" } else { "unregister" }, &e))?;
    let after = state_of(unsafe { svc.status() });
    if interactive && on && after == State::NeedsApproval {
        open_settings();
    }
    Ok(after)
}

/// 卸载时：登记过就注销，不管 App 现在在不在"应用程序"文件夹里（可能是在那里登记之后才挪走的）
pub fn unregister() -> Result<(), String> {
    if bundle().is_none() {
        return Ok(());
    }
    let svc = unsafe { SMAppService::mainAppService() };
    if step(false, unsafe { svc.status() }) == Step::Unregister {
        unsafe { svc.unregisterAndReturnError() }.map_err(|e| failed("unregister", &e))?;
    }
    Ok(())
}

fn failed(what: &str, e: &objc2_foundation::NSError) -> String {
    // 说明文字是系统语言写的，只进日志
    format!(
        "can't {what} the login item: {} {} ({})",
        e.domain(),
        e.code(),
        e.localizedDescription()
    )
}

/// 打开"系统设置 → 通用 → 登录项"
pub fn open_settings() {
    unsafe { SMAppService::openSystemSettingsLoginItems() };
}

fn status() -> SMAppServiceStatus {
    unsafe { SMAppService::mainAppService().status() }
}

/// `…/Airhand.app/Contents/MacOS/airhand` → `…/Airhand.app`
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    let is_app = app
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    (macos.file_name()? == "MacOS" && contents.file_name()? == "Contents" && is_app)
        .then(|| app.to_path_buf())
}

/// App 在"应用程序"文件夹（或者它的子文件夹）里
fn installed(bundle: &Path, home: Option<&Path>) -> bool {
    bundle.starts_with("/Applications")
        || home.is_some_and(|h| bundle.starts_with(h.join("Applications")))
}

fn state_of(status: SMAppServiceStatus) -> State {
    match status {
        SMAppServiceStatus::Enabled => State::On,
        SMAppServiceStatus::RequiresApproval => State::NeedsApproval,
        // NotRegistered、NotFound（包括用户在系统设置里删掉了）
        _ => State::Off,
    }
}

/// 要打开 / 关掉时，按现在的状态该做什么
#[derive(Debug, PartialEq, Eq)]
enum Step {
    Register,
    Unregister,
    /// 已经登记了、在等用户允许：再登记也没用，打开系统设置让用户允许
    OpenSettings,
    Nothing,
}

fn step(on: bool, status: SMAppServiceStatus) -> Step {
    match (on, status) {
        (true, SMAppServiceStatus::Enabled) => Step::Nothing,
        (true, SMAppServiceStatus::RequiresApproval) => Step::OpenSettings,
        (true, _) => Step::Register,
        // 等用户允许的也要注销，不然用户之后在系统设置里一允许又打开了
        (false, SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval) => {
            Step::Unregister
        }
        (false, _) => Step::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_bundle() {
        let p = |s: &str| bundle_of(Path::new(s));
        assert_eq!(
            p("/Applications/Airhand.app/Contents/MacOS/airhand"),
            Some(PathBuf::from("/Applications/Airhand.app"))
        );
        // 开发时直接运行的可执行文件
        assert_eq!(p("/Users/me/airhand/target/release/airhand"), None);
        assert_eq!(p("/Applications/Airhand/Contents/MacOS/airhand"), None);
        assert_eq!(p("/x/Airhand.app/Contents/Resources/airhand"), None);
        assert_eq!(p("airhand"), None);
    }

    #[test]
    fn only_from_the_applications_folder() {
        let home = Some(Path::new("/Users/me"));
        assert!(installed(Path::new("/Applications/Airhand.app"), home));
        assert!(installed(
            Path::new("/Applications/Tools/Airhand.app"),
            home
        ));
        assert!(installed(
            Path::new("/Users/me/Applications/Airhand.app"),
            home
        ));
        // 下载目录、磁盘映像、系统挪到临时位置运行的
        assert!(!installed(
            Path::new("/Users/me/Downloads/Airhand.app"),
            home
        ));
        assert!(!installed(Path::new("/Volumes/Airhand/Airhand.app"), home));
        assert!(!installed(
            Path::new("/private/var/folders/ab/T/AppTranslocation/X/d/Airhand.app"),
            home
        ));
        assert!(!installed(Path::new("/ApplicationsX/Airhand.app"), home));
        assert!(!installed(
            Path::new("/Users/me/Applications/Airhand.app"),
            None
        ));
    }

    /// 打开：没登记的登记，等允许的打开系统设置；关掉：登记了的（包括等允许的）都注销，没登记的不动
    #[test]
    fn steps() {
        use SMAppServiceStatus as S;
        let all = [
            S::NotRegistered,
            S::Enabled,
            S::RequiresApproval,
            S::NotFound,
        ];
        let on: Vec<_> = all.iter().map(|&s| step(true, s)).collect();
        assert_eq!(
            on,
            [
                Step::Register,
                Step::Nothing,
                Step::OpenSettings,
                Step::Register
            ]
        );
        let off: Vec<_> = all.iter().map(|&s| step(false, s)).collect();
        assert_eq!(
            off,
            [
                Step::Nothing,
                Step::Unregister,
                Step::Unregister,
                Step::Nothing
            ]
        );
        assert_eq!(
            all.map(state_of),
            [State::Off, State::On, State::NeedsApproval, State::Off]
        );
    }
}
