//! "不在系统中留下任何文件"。
//!
//! 被控端模式完全不创建 WebView，因此不会产生任何文件。
//! 控制端界面需要 WebView（Windows：WebView2；Linux：WebKitGTK），WebView 需要一个用户数据目录；
//! 这里把它放到临时目录中的随机子目录，并开启隐私模式（不持久化 Cookie/缓存），在窗口关闭和程序退出时彻底删除。
//! Linux 上还有几样东西不得不写文件：托盘图标只能以文件形式交给 AppIndicator；WebKitGTK 的媒体解码
//! （GStreamer）会写插件缓存；Mesa 会写着色器缓存。前两者都放进本进程专用的随机目录（`XDG_RUNTIME_DIR`
//! 内存盘），退出时删除；着色器磁盘缓存直接关掉（控制台只画 2D 画面，没有影响）。
//! 万一进程被强杀，下次启动时也会先清理残留目录。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

const PREFIX: &str = "airhand-webview-";
#[cfg(target_os = "linux")]
const PROCESS_PREFIX: &str = "airhand-run-";

pub fn new_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{PREFIX}{:016x}", rand::random::<u64>()))
}

/// 本进程专用的目录（托盘图标、GStreamer 缓存）：优先放在 `XDG_RUNTIME_DIR`（内存盘、仅本用户可见），
/// 没有时放临时目录。退出时整个删除。
#[cfg(target_os = "linux")]
pub fn process_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        runtime_dir().join(format!("{PROCESS_PREFIX}{:016x}", rand::random::<u64>()))
    })
    .clone()
}

/// 让 WebKitGTK 用到的库不在用户目录里写缓存。必须在启动任何线程、创建任何窗口之前调用
/// （WebKit 的子进程会继承这些环境变量）。用户自己设置过的变量保持不变。
#[cfg(target_os = "linux")]
pub fn isolate_caches() {
    let dir = process_dir();
    let _ = std::fs::create_dir_all(&dir);
    let vars = [
        // GStreamer 插件缓存（默认 ~/.cache/gstreamer-1.0）
        (
            "GST_REGISTRY",
            dir.join("gst-registry.bin").into_os_string(),
        ),
        // Mesa / NVIDIA 的着色器磁盘缓存（默认 ~/.cache/mesa_shader_cache、~/.nv）
        ("MESA_SHADER_CACHE_DISABLE", "true".into()),
        ("__GL_SHADER_DISK_CACHE", "0".into()),
    ];
    for (k, v) in vars {
        if std::env::var_os(k).is_none() {
            // SAFETY: 在 run() 的最开始调用，此时只有主线程
            unsafe { std::env::set_var(k, v) };
        }
    }
}

#[cfg(target_os = "linux")]
fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// 清理之前异常退出残留的目录（单实例锁保证不会误删正在使用的目录）。
pub fn sweep_stale() {
    #[allow(unused_mut)]
    let mut places = vec![(std::env::temp_dir(), PREFIX)];
    #[cfg(target_os = "linux")]
    places.push((runtime_dir(), PROCESS_PREFIX));
    for (dir, prefix) in places {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            if entry.file_name().to_string_lossy().starts_with(prefix) {
                let path = entry.path();
                match std::fs::remove_dir_all(&path) {
                    Ok(()) => debug!("removed leftover directory {}", path.display()),
                    Err(e) => debug!("can't remove leftover directory {}: {e}", path.display()),
                }
            }
        }
    }
}

/// 删除目录；WebView 进程退出需要一点时间，所以会重试直到超时。
pub fn remove_with_retry(path: &PathBuf, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match std::fs::remove_dir_all(path) {
            Ok(()) => return true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(e) => {
                if Instant::now() >= deadline {
                    warn!("can't remove temporary directory {}: {e}", path.display());
                    return false;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

/// 后台删除（不阻塞调用方）。
pub fn remove_in_background(path: PathBuf) {
    std::thread::spawn(move || {
        remove_with_retry(&path, Duration::from_secs(30));
    });
}
