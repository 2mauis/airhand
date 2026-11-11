//! 异步用例的有界等待与后台任务的断言回传（规格 #139 (10)(11)：等待一律设上限；`tokio::spawn` 内的断言
//! 消息带回再抛出）。只给依赖 tokio `time` 特性的测试用。
#![allow(dead_code)]

use std::future::Future;
use std::time::Duration;

/// `limit` 内完成返回 `Some`，否则 `None`（调用方用 `p2p_some!` 转成 `ASSERT:` 失败）
pub async fn within<T>(limit: Duration, fut: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(limit, fut).await.ok()
}

/// 把 `tokio::spawn` 出去的任务里的 panic 消息带回当前线程再抛出（保留任务内的 `ASSERT:` 前缀）
pub fn rethrow<T>(res: Result<T, tokio::task::JoinError>) -> T {
    match res {
        Ok(v) => v,
        Err(e) if e.is_panic() => {
            let payload = e.into_panic();
            let msg = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            if msg.starts_with("ASSERT:") {
                panic!("{msg}");
            }
            panic!("ASSERT: 后台任务异常退出：{msg}");
        }
        Err(e) => panic!("ASSERT: 后台任务被取消：{e}"),
    }
}
