//! 单实例：不产生任何文件。
//!
//! * Windows：命名互斥量（内核对象）；
//! * Linux：抽象命名空间的 Unix 套接字（只存在于内核中，进程退出自动释放，不会留下锁文件）；
//! * macOS：在 bootstrap 服务里登记一个 Mach 端口名（`CFMessagePort`），同样只在内核 / launchd 里，
//!   进程退出时端口失效、名字自动注销。

pub struct Guard {
    #[cfg(windows)]
    _handle: windows::Win32::Foundation::HANDLE,
    #[cfg(target_os = "linux")]
    _socket: Option<std::os::unix::net::UnixListener>,
    #[cfg(target_os = "macos")]
    _port: Option<objc2_core_foundation::CFRetained<objc2_core_foundation::CFMessagePort>>,
}

#[cfg(windows)]
pub fn acquire() -> Option<Guard> {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    unsafe {
        let handle = CreateMutexW(None, true, w!("Local\\Airhand.SingleInstance")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return None;
        }
        Some(Guard { _handle: handle })
    }
}

#[cfg(target_os = "linux")]
pub fn acquire() -> Option<Guard> {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener};

    // 每个用户一个实例
    let name = format!("airhand.single-instance.{}", unsafe { libc::getuid() });
    let socket =
        match SocketAddr::from_abstract_name(name).and_then(|a| UnixListener::bind_addr(&a)) {
            Ok(s) => Some(s),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => return None,
            Err(e) => {
                // 无法判断时允许运行
                tracing::warn!("single-instance check failed: {e}");
                None
            }
        };
    Some(Guard { _socket: socket })
}

#[cfg(target_os = "macos")]
pub fn acquire() -> Option<Guard> {
    use objc2_core_foundation::{CFMessagePort, CFString};

    // 每个用户一个实例（不同用户的 bootstrap 命名空间本来就是分开的，带上 uid 更保险）
    let name = CFString::from_str(&format!("app.airhand.single-instance.{}", unsafe {
        libc::getuid()
    }));
    let running = || CFMessagePort::new_remote(None, Some(&name)).is_some();
    if running() {
        return None;
    }
    // 只占用名字，不处理消息（不加入任何 run loop，回调永远不会被调用）
    unsafe extern "C-unwind" fn ignore(
        _: *mut CFMessagePort,
        _: i32,
        _: *const objc2_core_foundation::CFData,
        _: *mut std::ffi::c_void,
    ) -> *const objc2_core_foundation::CFData {
        std::ptr::null()
    }
    let port = unsafe {
        CFMessagePort::new_local(
            None,
            Some(&name),
            Some(ignore),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if port.is_none() {
        // 两个实例同时启动时，后登记的会失败；其它原因无法判断时允许运行
        if running() {
            return None;
        }
        tracing::warn!("single-instance check failed: can't register the Mach port name");
    }
    Some(Guard { _port: port })
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn acquire() -> Option<Guard> {
    Some(Guard {})
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn only_one_instance() {
        let first =
            super::acquire().expect("第一个实例应该能运行（是不是已经有 Airhand 在运行？）");
        assert!(super::acquire().is_none(), "第二个实例不应该运行");
        drop(first);
        assert!(super::acquire().is_some(), "第一个实例退出后应该可以再运行");
    }
}
