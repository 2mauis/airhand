//! 进程里唯一的 tokio 运行时，以及日志（Android 上写到 logcat，标签 `airhand`）。

use std::sync::{LazyLock, Once};

use tokio::runtime::Runtime;

pub fn rt() -> &'static Runtime {
    static RT: LazyLock<Runtime> = LazyLock::new(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("airhand-rt")
            .enable_all()
            .build()
            .expect("tokio runtime")
    });
    &RT
}

/// 初始化日志（只生效一次）。`filter` 是 `tracing` 的 EnvFilter 语法，例如 `info,quinn=warn`。
pub fn init_logging(filter: &str) {
    static ONCE: Once = Once::new();
    let filter = filter.to_string();
    ONCE.call_once(move || {
        let filter = tracing_subscriber::EnvFilter::try_new(&filter)
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let builder = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(false)
            .with_target(true);
        #[cfg(target_os = "android")]
        let builder = builder.without_time().with_writer(logcat::MakeLogcat);
        let _ = builder.try_init();
        std::panic::set_hook(Box::new(|info| {
            tracing::error!("panic: {info}");
        }));
    });
}

#[cfg(target_os = "android")]
mod logcat {
    use std::ffi::CString;
    use std::io::Write;

    use android_log_sys::{__android_log_write, LogPriority};
    use tracing::{Level, Metadata};
    use tracing_subscriber::fmt::MakeWriter;

    pub struct MakeLogcat;

    /// 一条日志：先攒在缓冲区里，drop 时一次写进 logcat
    pub struct Line {
        prio: LogPriority,
        buf: Vec<u8>,
    }

    impl Write for Line {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.buf.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Drop for Line {
        fn drop(&mut self) {
            while self.buf.last().is_some_and(|c| *c == b'\n') {
                self.buf.pop();
            }
            self.buf.retain(|c| *c != 0);
            if let Ok(msg) = CString::new(std::mem::take(&mut self.buf)) {
                unsafe {
                    __android_log_write(self.prio as i32, c"airhand".as_ptr(), msg.as_ptr());
                }
            }
        }
    }

    impl<'a> MakeWriter<'a> for MakeLogcat {
        type Writer = Line;

        fn make_writer(&'a self) -> Line {
            Line {
                prio: LogPriority::INFO,
                buf: Vec::new(),
            }
        }

        fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
            let prio = match *meta.level() {
                Level::ERROR => LogPriority::ERROR,
                Level::WARN => LogPriority::WARN,
                Level::INFO => LogPriority::INFO,
                Level::DEBUG => LogPriority::DEBUG,
                Level::TRACE => LogPriority::VERBOSE,
            };
            Line {
                prio,
                buf: Vec::new(),
            }
        }
    }
}
