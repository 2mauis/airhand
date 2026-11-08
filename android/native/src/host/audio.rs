//! 被控端的声音分发：Kotlin 采集手机正在播放的声音（AudioPlaybackCapture，Android 10+）并编成 Opus，
//! 每包 20 ms，交给开着声音的会话。和桌面版一样，没有声音时不发包（Kotlin 那边跳过静音），
//! 停顿之后的第一包带 `start`，控制端据此区分"声音停了"和"网络抖动"。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use airhand_proto::framing;
use airhand_proto::session::{AudioFrame, HostMsg, Notice};
use bytes::Bytes;
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::queue::Queue;

/// 每个会话的声音队列，满了直接丢（不能越积越慢）
pub const QUEUE: usize = 16;

/// 两包之间隔了这么久就算停顿过：下一包带 `start`
const GAP: Duration = Duration::from_millis(100);

pub struct AudioHub {
    st: Mutex<State>,
    events: Arc<Queue<String>>,
}

#[derive(Default)]
struct State {
    subs: HashMap<u64, Sub>,
    /// 采集不了声音的原因（英文，给控制端排查问题用）
    failure: Option<String>,
}

struct Sub {
    tx: mpsc::Sender<Bytes>,
    seq: u32,
    last: Option<Instant>,
}

impl AudioHub {
    pub fn new(events: Arc<Queue<String>>) -> Self {
        Self {
            st: Mutex::default(),
            events,
        }
    }

    fn report_active(&self, active: bool) {
        info!(active, "audio wanted");
        self.events
            .push(json!({ "type": "audio", "active": active }).to_string());
    }

    /// 会话打开声音。采集已经失败了的话马上告诉它
    pub fn subscribe(&self, id: u64, tx: mpsc::Sender<Bytes>) {
        let mut st = self.st.lock().unwrap();
        if let Some(detail) = &st.failure
            && let Ok(b) = framing::encode(&HostMsg::Notice(Notice::AudioFailed {
                detail: detail.clone(),
            }))
        {
            let _ = tx.try_send(Bytes::from(b));
        }
        let first = st.subs.is_empty();
        st.subs.insert(
            id,
            Sub {
                tx,
                seq: 0,
                last: None,
            },
        );
        drop(st);
        if first {
            self.report_active(true);
        }
    }

    pub fn unsubscribe(&self, id: u64) {
        let mut st = self.st.lock().unwrap();
        let removed = st.subs.remove(&id).is_some();
        let empty = st.subs.is_empty();
        drop(st);
        if removed && empty {
            self.report_active(false);
        }
    }

    /// 一个 Opus 包
    pub fn push(&self, data: &[u8], capture_us: u64) {
        let mut st = self.st.lock().unwrap();
        let now = Instant::now();
        for (id, s) in st.subs.iter_mut() {
            let start = s.last.is_none_or(|t| now.duration_since(t) > GAP);
            s.last = Some(now);
            s.seq = s.seq.wrapping_add(1);
            let msg = HostMsg::Audio(AudioFrame {
                seq: s.seq,
                start,
                capture_us,
                data: data.to_vec(),
            });
            let Ok(b) = framing::encode_with_capacity(&msg, data.len()) else {
                continue;
            };
            if s.tx.try_send(Bytes::from(b)).is_err() {
                debug!(session = id, "audio queue full, dropping a packet");
            }
        }
    }

    /// 采集打不开（`Some`）/ 恢复了（`None`）：告诉开着声音的控制端
    pub fn set_failure(&self, failure: Option<String>) {
        let mut st = self.st.lock().unwrap();
        if st.failure == failure {
            return;
        }
        st.failure = failure.clone();
        let notice = match failure {
            Some(detail) => Notice::AudioFailed { detail },
            None => Notice::AudioRecovered,
        };
        let Ok(b) = framing::encode(&HostMsg::Notice(notice)) else {
            return;
        };
        let b = Bytes::from(b);
        for s in st.subs.values() {
            let _ = s.tx.try_send(b.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(b: &Bytes) -> HostMsg {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut buf = Vec::new();
            framing::read_msg::<_, HostMsg>(&mut &b[..], 1 << 20, &mut buf)
                .await
                .unwrap()
                .unwrap()
        })
    }

    #[test]
    fn start_flag_and_failures() {
        let events = Arc::new(Queue::new(16));
        let hub = AudioHub::new(events.clone());
        let (tx, mut rx) = mpsc::channel(QUEUE);
        hub.subscribe(1, tx);
        assert_eq!(
            events.pop(Duration::ZERO).unwrap(),
            r#"{"active":true,"type":"audio"}"#
        );
        hub.push(&[1, 2], 10);
        hub.push(&[3], 20);
        let HostMsg::Audio(a) = decode(&rx.try_recv().unwrap()) else {
            panic!()
        };
        assert!(a.start);
        assert_eq!((a.seq, a.capture_us, a.data), (1, 10, vec![1, 2]));
        let HostMsg::Audio(b) = decode(&rx.try_recv().unwrap()) else {
            panic!()
        };
        assert!(!b.start);
        assert_eq!(b.seq, 2);

        hub.set_failure(Some("no permission".into()));
        assert!(matches!(
            decode(&rx.try_recv().unwrap()),
            HostMsg::Notice(Notice::AudioFailed { .. })
        ));
        // 之后打开声音的会话马上收到
        let (tx2, mut rx2) = mpsc::channel(QUEUE);
        hub.subscribe(2, tx2);
        assert!(matches!(
            decode(&rx2.try_recv().unwrap()),
            HostMsg::Notice(Notice::AudioFailed { .. })
        ));
        hub.set_failure(None);
        assert!(matches!(
            decode(&rx.try_recv().unwrap()),
            HostMsg::Notice(Notice::AudioRecovered)
        ));

        hub.unsubscribe(1);
        assert!(events.pop(Duration::ZERO).is_none());
        hub.unsubscribe(2);
        assert_eq!(
            events.pop(Duration::ZERO).unwrap(),
            r#"{"active":false,"type":"audio"}"#
        );
    }
}
