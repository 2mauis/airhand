//! 被控端时钟：画面帧和声音包的采集时间都换算到同一个单调时钟（微秒），控制端据此做音画同步。
//!
//! 只要两种时间戳来自同一个时钟，控制端就不需要知道两台电脑之间的时钟差：
//! 未知的时钟差对画面和声音是一样的，比较时会抵消掉。

use std::sync::LazyLock;
use std::time::{Duration, Instant};

static EPOCH: LazyLock<Instant> = LazyLock::new(|| {
    let now = Instant::now();
    // 留出余量：比首次调用更早的采集时间也不会被截断为 0
    now.checked_sub(Duration::from_secs(60)).unwrap_or(now)
});

/// 提前确定时钟零点（被控端服务启动时调用）。
pub fn init() {
    LazyLock::force(&EPOCH);
}

/// 某一时刻在被控端时钟上的位置（微秒）。
pub fn us(t: Instant) -> u64 {
    t.saturating_duration_since(*EPOCH).as_micros() as u64
}

/// 系统 API 给出的时间戳只比现在早这么多以内才可信
#[cfg(windows)]
const MAX_AGE: Duration = Duration::from_secs(5);

/// `QueryPerformanceCounter` 计数（DXGI 的 `LastPresentTime`）对应的时刻。
#[cfg(windows)]
pub fn from_qpc(ticks: i64) -> Option<Instant> {
    let (now, freq) = qpc();
    from_age(now.checked_sub(ticks)?, freq)
}

/// 以 100 ns 为单位的 QPC 时间（WASAPI `GetBuffer` 的 `u64QPCPosition`）对应的时刻。
#[cfg(windows)]
pub fn from_qpc_100ns(t: u64) -> Option<Instant> {
    let (now, freq) = qpc();
    let now_100ns = (now as i128 * 10_000_000 / freq as i128) as i64;
    from_age(now_100ns.checked_sub(t as i64)?, 10_000_000)
}

#[cfg(windows)]
fn qpc() -> (i64, i64) {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    let (mut now, mut freq) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceFrequency(&mut freq);
        let _ = QueryPerformanceCounter(&mut now);
    }
    (now, freq.max(1))
}

/// `age` 个时钟周期（频率 `freq`）之前的时刻；时间戳稍晚于现在（读取顺序造成的误差）按现在处理。
#[cfg(windows)]
fn from_age(age: i64, freq: i64) -> Option<Instant> {
    let age = Duration::from_nanos((age.max(0) as i128 * 1_000_000_000 / freq as i128) as u64);
    if age > MAX_AGE {
        return None;
    }
    Instant::now().checked_sub(age)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn qpc_conversion() {
        let (now, freq) = qpc();
        let t = from_qpc(now - freq / 10).unwrap();
        let ago = t.elapsed().as_secs_f64();
        assert!((0.099..0.105).contains(&ago), "{ago}");

        let now_100ns = (now as i128 * 10_000_000 / freq as i128) as u64;
        let t = from_qpc_100ns(now_100ns - 500_000).unwrap();
        let ago = t.elapsed().as_secs_f64();
        assert!((0.049..0.055).contains(&ago), "{ago}");

        assert!(from_qpc(now - freq * 60).is_none(), "过旧的时间戳应被拒绝");
        init();
        let u = us(Instant::now());
        assert!(u >= 60_000_000, "零点之前应留有余量: {u}");
    }
}
