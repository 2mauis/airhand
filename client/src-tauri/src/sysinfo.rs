//! 本机标识（计算机名 / 操作系统），仅用于展示。计算机名未知时为空字符串。

use std::sync::OnceLock;

pub fn host_identity() -> (String, String) {
    static CACHE: OnceLock<(String, String)> = OnceLock::new();
    CACHE.get_or_init(|| (hostname(), os_name())).clone()
}

fn hostname() -> String {
    // macOS："系统设置 → 通用 → 共享"里的电脑名称（例如"张三的 MacBook Pro"），比主机名好认
    #[cfg(target_os = "macos")]
    if let Some(name) = mac_computer_name() {
        return name;
    }
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(key)
            && !v.trim().is_empty()
        {
            return v;
        }
    }
    #[cfg(unix)]
    for path in ["/proc/sys/kernel/hostname", "/etc/hostname"] {
        if let Ok(v) = std::fs::read_to_string(path)
            && !v.trim().is_empty()
        {
            return v.trim().to_string();
        }
    }
    // 不知道名称：发空字符串，由显示的一方按自己的语言显示"未知设备"
    String::new()
}

#[cfg(windows)]
fn os_name() -> String {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};

    fn reg(name: &str) -> Option<String> {
        let key = HSTRING::from(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let name = HSTRING::from(name);
        let mut buf = [0u16; 256];
        let mut len = (buf.len() * 2) as u32;
        let ok = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                PCWSTR(key.as_ptr()),
                PCWSTR(name.as_ptr()),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut len),
            )
        };
        if ok.is_err() {
            return None;
        }
        let n = (len as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
    }

    let build: u32 = reg("CurrentBuild")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let product = reg("ProductName").unwrap_or_else(|| "Windows".into());
    // Windows 11 的注册表 ProductName 仍然写着 Windows 10
    let product = if build >= 22000 {
        product.replace("Windows 10", "Windows 11")
    } else {
        product
    };
    match reg("DisplayVersion") {
        Some(v) => format!("{product} {v}"),
        None => product,
    }
}

/// 发行版名称，例如 "Ubuntu 24.04.1 LTS"。
#[cfg(target_os = "linux")]
fn os_name() -> String {
    let release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default();
    parse_os_release(&release).unwrap_or_else(|| "Linux".into())
}

#[cfg(target_os = "linux")]
fn parse_os_release(text: &str) -> Option<String> {
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
            .filter(|v| !v.is_empty())
    };
    field("PRETTY_NAME").or_else(|| field("NAME"))
}

#[cfg(target_os = "macos")]
fn mac_computer_name() -> Option<String> {
    use objc2_core_foundation::{CFRetained, CFString};

    #[link(name = "SystemConfiguration", kind = "framework")]
    unsafe extern "C" {
        fn SCDynamicStoreCopyComputerName(
            store: *const std::ffi::c_void,
            encoding: *mut u32,
        ) -> *mut CFString;
    }
    // 返回 +1 引用
    let name = std::ptr::NonNull::new(unsafe {
        SCDynamicStoreCopyComputerName(std::ptr::null(), std::ptr::null_mut())
    })
    .map(|p| unsafe { CFRetained::from_raw(p) })?;
    let name = name.to_string();
    (!name.trim().is_empty()).then_some(name)
}

/// 例如 "macOS 15.3.1"
#[cfg(target_os = "macos")]
fn os_name() -> String {
    let v = objc2_foundation::NSProcessInfo::processInfo().operatingSystemVersion();
    if v.patchVersion > 0 {
        format!(
            "macOS {}.{}.{}",
            v.majorVersion, v.minorVersion, v.patchVersion
        )
    } else {
        format!("macOS {}.{}", v.majorVersion, v.minorVersion)
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn os_name() -> String {
    std::env::consts::OS.to_string()
}

#[cfg(all(test, target_os = "macos"))]
mod mac_tests {
    #[test]
    fn identity() {
        let (name, os) = super::host_identity();
        println!("{name:?} / {os:?}");
        assert!(!name.is_empty());
        assert!(os.starts_with("macOS "), "{os}");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn os_release() {
        let ubuntu = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            super::parse_os_release(ubuntu).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(
            super::parse_os_release("NAME=Fedora\nVERSION=40").as_deref(),
            Some("Fedora")
        );
        assert_eq!(
            super::parse_os_release("NAME='Arch Linux'").as_deref(),
            Some("Arch Linux")
        );
        assert_eq!(super::parse_os_release(""), None);
    }
}
