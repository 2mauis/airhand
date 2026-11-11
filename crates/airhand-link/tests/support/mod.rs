//! tailcat-p2p 先红验收测试的共享断言与空操作开关（`docs/specs/tailcat-p2p.md`「先红验收测试清单」）。
//!
//! * 失败信息一律以 `ASSERT:` 开头：预期红名单门禁（`tests/p2p-expected-red.mjs`）只把这种失败（或标准库
//!   `assert*!` 的 `assertion ` 开头）当作“对的红”，`unwrap` / `expect` / `todo!()` / 超时一律判为错误原因。
//! * 等待一律有上限，超时以 `ASSERT:` 失败结束；异步用例的有界等待与 `tokio::spawn` 的消息回传在
//!   同目录的 `wait.rs`（需要 tokio 的 `time` 特性，不是每个引入方都有）。
//! * 空操作开关：每个承载 A 级用例的 crate 各有 `noop-lenient` / `noop-strict` 两个 cargo feature（互斥），
//!   [`NOOP`] 是本次构建选中的极性；两个都没开时是常规作业。
//!
//! 其它 crate 的测试用 `#[macro_use] #[path = "<到本文件的相对路径>"] mod support;` 引入同一份文件（必须写在
//! 使用这些宏的模块之前）；
//! `cfg(feature = …)` 按引入它的那个 crate 的 feature 求值，所以每个引入方都要声明这两个 feature。
#![allow(dead_code, unused_macros)]

#[cfg(all(feature = "noop-lenient", feature = "noop-strict"))]
compile_error!(
    "noop-lenient 与 noop-strict 互斥：Lenient / Strict 两个作业必须分开构建（tailcat-p2p 规格 #98，S3-13）"
);

use std::fmt::Display;
use std::path::PathBuf;

/// 空操作驱动的极性（规格 #98）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Polarity {
    /// 什么都不做：不发消息、不启动 helper、闸门打开后永不关闭
    Lenient,
    /// 全部丢弃：闸门恒关、数据全部丢弃、计数为 0
    Strict,
}

impl Polarity {
    pub fn name(self) -> &'static str {
        match self {
            Self::Lenient => "lenient",
            Self::Strict => "strict",
        }
    }
}

/// 本次构建打开的空操作开关；`None` = 常规作业
pub const NOOP: Option<Polarity> = if cfg!(feature = "noop-lenient") {
    Some(Polarity::Lenient)
} else if cfg!(feature = "noop-strict") {
    Some(Polarity::Strict)
} else {
    None
};

/// 条件不成立即以 `ASSERT:` 失败
macro_rules! p2p_assert {
    ($cond:expr $(,)?) => {
        p2p_assert!($cond, "{}", stringify!($cond))
    };
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            panic!("ASSERT: {}", format!($($arg)+));
        }
    };
}

/// 相等断言，失败时带出两侧的值
macro_rules! p2p_assert_eq {
    ($left:expr, $right:expr $(,)?) => {
        p2p_assert_eq!($left, $right, "{} == {}", stringify!($left), stringify!($right))
    };
    ($left:expr, $right:expr, $($arg:tt)+) => {
        match (&$left, &$right) {
            (l, r) => {
                if !(*l == *r) {
                    panic!("ASSERT: {}（实际 {:?}，期望 {:?}）", format!($($arg)+), l, r);
                }
            }
        }
    };
}

/// `Some(v)` 取出 `v`，`None` 以 `ASSERT:` 失败
macro_rules! p2p_some {
    ($opt:expr, $($arg:tt)+) => {
        match $opt {
            Some(v) => v,
            None => panic!("ASSERT: {}", format!($($arg)+)),
        }
    };
}

/// `Ok(v)` 取出 `v`，`Err(e)` 以 `ASSERT:` 失败
macro_rules! p2p_ok {
    ($res:expr, $($arg:tt)+) => {
        match $res {
            Ok(v) => v,
            Err(e) => panic!("ASSERT: {}（{:#}）", format!($($arg)+), e),
        }
    };
}

/// 无条件以 `ASSERT:` 失败
macro_rules! p2p_fail {
    ($($arg:tt)+) => {
        panic!("ASSERT: {}", format!($($arg)+))
    };
}

/// `Option` / `Result` 的 `expect` 替代：失败时以 `ASSERT:` 开头（`expect` 的 panic 不算断言失败）
pub trait Expect<T> {
    fn p2p_expect(self, msg: &str) -> T;
}

impl<T> Expect<T> for Option<T> {
    #[track_caller]
    fn p2p_expect(self, msg: &str) -> T {
        match self {
            Some(v) => v,
            None => panic!("ASSERT: {msg}"),
        }
    }
}

impl<T, E: std::fmt::Debug> Expect<T> for Result<T, E> {
    #[track_caller]
    fn p2p_expect(self, msg: &str) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("ASSERT: {msg}（{e:?}）"),
        }
    }
}

/// 只记录、不作断言的观测值（规格：soak 与随机丢包类判据只记录，Jack 2026-11-11 决定 (a)）。
/// 输出到测试的标准输出；设了 `P2P_RECORD_DIR` 时另外追加到 `<dir>/<用例名>.txt`。
pub fn record(test: &str, line: impl Display) {
    let line = format!("RECORD: {test}: {line}");
    println!("{line}");
    if let Some(dir) = std::env::var_os("P2P_RECORD_DIR") {
        use std::io::Write;
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{test}.txt")))
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

/// 仓库根目录（向上找 `tests/p2p-a-registry.txt`）
pub fn repo_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("tests/p2p-a-registry.txt").is_file() {
            return dir;
        }
        p2p_assert!(
            dir.pop(),
            "从 {} 向上找不到仓库根目录",
            env!("CARGO_MANIFEST_DIR")
        );
    }
}
