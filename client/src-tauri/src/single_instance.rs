//! 单实例：不产生任何文件。
//!
//! * Windows：命名互斥量（内核对象）；
//! * Linux：抽象命名空间的 Unix 套接字（只存在于内核中，进程退出自动释放，不会留下锁文件）。

pub struct Guard {
    #[cfg(windows)]
    _handle: windows::Win32::Foundation::HANDLE,
    #[cfg(target_os = "linux")]
    _socket: Option<std::os::unix::net::UnixListener>,
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

#[cfg(not(any(windows, target_os = "linux")))]
pub fn acquire() -> Option<Guard> {
    Some(Guard {})
}
