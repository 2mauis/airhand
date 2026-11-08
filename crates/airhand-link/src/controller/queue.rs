//! Bounded controller commands. Replace superseded state, never drop a key-up.
//! Overflow or stale ordered input terminates the session instead of replaying
//! a long backlog of remote operations when the network recovers.
use crate::Problem;
use airhand_proto::{
    framing,
    session::{ControllerMsg, InputEvent},
};
use bytes::Bytes;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

const MAX_MESSAGES: usize = 128;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_AGE: Duration = Duration::from_secs(2);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ordered,
    Move,
    Settings,
    Ack,
    Ping,
}
struct Item {
    kind: Kind,
    bytes: Bytes,
    at: Instant,
}
#[derive(Default)]
struct State {
    items: VecDeque<Item>,
    bytes: usize,
    failed: bool,
}
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    ready: Notify,
}
#[derive(Clone)]
pub(super) struct Sender(Arc<Shared>);
pub(super) struct Receiver(Arc<Shared>);
pub(super) fn channel() -> (Sender, Receiver) {
    let shared = Arc::new(Shared::default());
    (Sender(shared.clone()), Receiver(shared))
}
impl Sender {
    pub fn send(&self, msg: ControllerMsg) -> Result<(), Problem> {
        let kind = match &msg {
            ControllerMsg::Settings(_) => Kind::Settings,
            ControllerMsg::Ack { .. } => Kind::Ack,
            ControllerMsg::Ping { .. } => Kind::Ping,
            ControllerMsg::Input(events)
                if !events.is_empty()
                    && events
                        .iter()
                        .all(|e| matches!(e, InputEvent::MouseMove { .. })) =>
            {
                Kind::Move
            }
            _ => Kind::Ordered,
        };
        let msg = match msg {
            ControllerMsg::Input(mut events) if kind == Kind::Move => {
                ControllerMsg::Input(vec![events.pop().unwrap()])
            }
            m => m,
        };
        let bytes = Bytes::from(framing::encode(&msg).map_err(Problem::failed)?);
        let mut s = self.0.state.lock().unwrap();
        if s.failed {
            return Err(Problem::SessionEnded);
        }
        // ACK and Ping may bypass ordered input. Other state is coalesced only
        // adjacently, so a move cannot cross a mouse click or display change.
        let replace = if matches!(kind, Kind::Ack | Kind::Ping) {
            s.items.iter().position(|i| i.kind == kind)
        } else if kind != Kind::Ordered && s.items.back().is_some_and(|i| i.kind == kind) {
            Some(s.items.len() - 1)
        } else {
            None
        };
        if let Some(i) = replace {
            s.bytes -= s.items.remove(i).unwrap().bytes.len();
        }
        if s.items.len() >= MAX_MESSAGES || s.bytes + bytes.len() > MAX_BYTES {
            s.failed = true;
            s.items.clear();
            s.bytes = 0;
            drop(s);
            self.0.ready.notify_one();
            return Err(Problem::failed("controller command queue overflow"));
        }
        s.bytes += bytes.len();
        s.items.push_back(Item {
            kind,
            bytes,
            at: Instant::now(),
        });
        drop(s);
        self.0.ready.notify_one();
        Ok(())
    }
}
impl Receiver {
    pub async fn recv(&mut self) -> anyhow::Result<Bytes> {
        loop {
            let notified = self.0.ready.notified();
            {
                let mut s = self.0.state.lock().unwrap();
                anyhow::ensure!(!s.failed, "controller command queue overflow");
                if let Some(i) = s.items.pop_front() {
                    s.bytes -= i.bytes.len();
                    anyhow::ensure!(i.at.elapsed() <= MAX_AGE, "controller commands expired");
                    return Ok(i.bytes);
                }
            }
            notified.await;
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        let mut s = self.0.state.lock().unwrap();
        s.failed = true;
        s.items.clear();
        s.bytes = 0;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_coalesces_without_crossing_key_release() {
        let (tx, _rx) = channel();
        for _ in 0..1000 {
            tx.send(ControllerMsg::Input(vec![InputEvent::MouseMove {
                x: 0.5,
                y: 0.5,
            }]))
            .unwrap();
        }
        tx.send(ControllerMsg::Input(vec![InputEvent::ReleaseAll]))
            .unwrap();
        tx.send(ControllerMsg::Input(vec![InputEvent::MouseMove {
            x: 1.0,
            y: 1.0,
        }]))
        .unwrap();
        assert_eq!(tx.0.state.lock().unwrap().items.len(), 3);
    }
    #[tokio::test]
    async fn overflow_and_expiry_fail_the_reader() {
        let (tx, mut rx) = channel();
        for _ in 0..MAX_MESSAGES {
            tx.send(ControllerMsg::RequestKeyframe).unwrap();
        }
        assert!(tx.send(ControllerMsg::RequestKeyframe).is_err());
        assert!(rx.recv().await.is_err());
        let (tx, mut rx) = channel();
        tx.send(ControllerMsg::RequestKeyframe).unwrap();
        tx.0.state.lock().unwrap().items.front_mut().unwrap().at =
            Instant::now() - MAX_AGE - Duration::from_millis(1);
        assert!(rx.recv().await.is_err());
    }
}
