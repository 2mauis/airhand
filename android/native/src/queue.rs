//! Rust → Kotlin 的阻塞队列：Rust 这边（异步任务里）只管放进去，Kotlin 的线程经 JNI 阻塞地取。
//!
//! 用拉取而不是回调：Rust 的线程不需要附加到 JVM，Kotlin 那边也不用处理来自任意线程的回调。

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::Duration;

pub struct Queue<T> {
    st: Mutex<State<T>>,
    cv: Condvar,
    cap: usize,
}

struct State<T> {
    items: VecDeque<T>,
    closed: bool,
}

impl<T> Queue<T> {
    pub fn new(cap: usize) -> Self {
        Self {
            st: Mutex::new(State {
                items: VecDeque::new(),
                closed: false,
            }),
            cv: Condvar::new(),
            cap,
        }
    }

    /// 放入一项。队列已关闭时返回 false；满了丢掉最旧的一项（取的一方卡住了，不能让内存无限增长）。
    pub fn push(&self, v: T) -> bool {
        let mut st = self.st.lock().unwrap();
        if st.closed {
            return false;
        }
        if st.items.len() >= self.cap {
            st.items.pop_front();
            tracing::warn!("queue full ({} items), dropping the oldest", self.cap);
        }
        st.items.push_back(v);
        drop(st);
        self.cv.notify_one();
        true
    }

    /// 取一项，最多等 `timeout`。超时或者队列已关闭（并且取空了）时返回 `None`。
    pub fn pop(&self, timeout: Duration) -> Option<T> {
        let st = self.st.lock().unwrap();
        let (mut st, _) = self
            .cv
            .wait_timeout_while(st, timeout, |s| s.items.is_empty() && !s.closed)
            .unwrap();
        st.items.pop_front()
    }

    /// 关闭队列：之后的 `push` 都失败，等着取的线程马上返回。
    pub fn close(&self) {
        self.st.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.st.lock().unwrap().closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn push_pop_close() {
        let q = Queue::new(2);
        assert!(q.push(1));
        assert!(q.push(2));
        assert!(q.push(3)); // 丢掉 1
        assert_eq!(q.pop(Duration::ZERO), Some(2));
        assert_eq!(q.pop(Duration::ZERO), Some(3));
        assert_eq!(q.pop(Duration::from_millis(10)), None);
        q.close();
        assert!(!q.push(4));
        assert!(q.is_closed());
    }

    #[test]
    fn close_wakes_waiters() {
        let q = Arc::new(Queue::<u32>::new(4));
        let q2 = q.clone();
        let t = std::thread::spawn(move || {
            let start = Instant::now();
            let r = q2.pop(Duration::from_secs(10));
            (r, start.elapsed())
        });
        std::thread::sleep(Duration::from_millis(50));
        q.close();
        let (r, waited) = t.join().unwrap();
        assert_eq!(r, None);
        assert!(waited < Duration::from_secs(5));
    }
}
