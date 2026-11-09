//! 控制端发给被控端的命令队列（有界）。
//!
//! * 被后来的取代的状态不排队：连续的绝对移动只留最后一个，连续的相对移动加起来，
//!   画质设置只留最新的，确认和 Ping 各只留一个（可以越过排着队的输入）；
//! * 移动只和相邻的移动合并，不会越过点击、按键（松开按键永远不会丢）；
//! * 排满了、或者排着的输入太旧了（网络断了一阵子），会话直接失败，
//!   不在网络恢复时把一大串过时的远程操作补发出去。
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Ordered,
    /// 只有绝对移动：只留最后一个
    Move,
    /// 只有相对移动：加起来
    Delta,
    Settings,
    Ack,
    Ping,
}

struct Item {
    kind: Kind,
    /// 相对移动的总量（合并时用）
    delta: (i32, i32),
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

fn encode(msg: &ControllerMsg) -> Result<Bytes, Problem> {
    Ok(Bytes::from(framing::encode(msg).map_err(Problem::failed)?))
}

fn delta_msg((dx, dy): (i32, i32)) -> ControllerMsg {
    ControllerMsg::Input(vec![InputEvent::MouseDelta { dx, dy }])
}

impl Sender {
    pub fn send(&self, msg: ControllerMsg) -> Result<(), Problem> {
        let mut msg = msg;
        if let ControllerMsg::Input(events) = &mut msg {
            InputEvent::coalesce(events);
            if events.is_empty() {
                return Ok(());
            }
        }
        let kind = match &msg {
            ControllerMsg::Settings(_) => Kind::Settings,
            ControllerMsg::Ack { .. } => Kind::Ack,
            ControllerMsg::Ping { .. } => Kind::Ping,
            ControllerMsg::Input(e) if matches!(e[..], [InputEvent::MouseMove { .. }]) => {
                Kind::Move
            }
            ControllerMsg::Input(e) if matches!(e[..], [InputEvent::MouseDelta { .. }]) => {
                Kind::Delta
            }
            _ => Kind::Ordered,
        };
        let delta = match &msg {
            ControllerMsg::Input(e) => match e[..] {
                [InputEvent::MouseDelta { dx, dy }] => (dx, dy),
                _ => (0, 0),
            },
            _ => (0, 0),
        };
        let mut bytes = encode(&msg)?;
        let mut s = self.0.state.lock().unwrap();
        if s.failed {
            return Err(Problem::SessionEnded);
        }
        let mut item = Item {
            kind,
            delta,
            bytes: Bytes::new(),
            at: Instant::now(),
        };
        // 确认和 Ping 可以越过排着队的输入；其它状态只和紧挨着的合并，移动不会越过点击或切换显示器
        let replace = if matches!(kind, Kind::Ack | Kind::Ping) {
            s.items.iter().position(|i| i.kind == kind)
        } else if kind != Kind::Ordered && s.items.back().is_some_and(|i| i.kind == kind) {
            Some(s.items.len() - 1)
        } else {
            None
        };
        if let Some(i) = replace {
            let old = s.items.remove(i).unwrap();
            s.bytes -= old.bytes.len();
            if kind == Kind::Delta {
                item.delta = (
                    old.delta.0.saturating_add(delta.0),
                    old.delta.1.saturating_add(delta.1),
                );
                item.at = old.at;
                bytes = encode(&delta_msg(item.delta))?;
            }
        }
        item.bytes = bytes;
        if s.items.len() >= MAX_MESSAGES || s.bytes + item.bytes.len() > MAX_BYTES {
            s.failed = true;
            s.items.clear();
            s.bytes = 0;
            drop(s);
            self.0.ready.notify_one();
            return Err(Problem::failed("controller command queue overflow"));
        }
        s.bytes += item.bytes.len();
        s.items.push_back(item);
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
    use airhand_proto::session::MouseButton;

    fn queued(tx: &Sender) -> Vec<ControllerMsg> {
        tx.0.state
            .lock()
            .unwrap()
            .items
            .iter()
            .map(|i| framing::decode(&i.bytes[4..]).unwrap())
            .collect()
    }

    fn input(events: Vec<InputEvent>) -> ControllerMsg {
        ControllerMsg::Input(events)
    }

    #[test]
    fn movement_coalesces_without_crossing_key_release() {
        let (tx, _rx) = channel();
        for _ in 0..1000 {
            tx.send(input(vec![InputEvent::MouseMove { x: 0.5, y: 0.5 }]))
                .unwrap();
        }
        tx.send(input(vec![InputEvent::ReleaseAll])).unwrap();
        tx.send(input(vec![InputEvent::MouseMove { x: 1.0, y: 1.0 }]))
            .unwrap();
        assert_eq!(tx.0.state.lock().unwrap().items.len(), 3);
    }

    #[test]
    fn relative_movement_adds_up() {
        let (tx, _rx) = channel();
        for _ in 0..500 {
            tx.send(input(vec![
                InputEvent::MouseDelta { dx: 1, dy: -2 },
                InputEvent::MouseDelta { dx: 1, dy: 0 },
            ]))
            .unwrap();
        }
        let up = InputEvent::MouseButton {
            button: MouseButton::Right,
            down: false,
        };
        tx.send(input(vec![up.clone()])).unwrap();
        tx.send(input(vec![InputEvent::MouseDelta { dx: 5, dy: 5 }]))
            .unwrap();
        let q = queued(&tx);
        assert_eq!(q.len(), 3);
        assert!(
            matches!(&q[0], ControllerMsg::Input(e) if e[..] == [InputEvent::MouseDelta { dx: 1000, dy: -1000 }])
        );
        assert!(matches!(&q[1], ControllerMsg::Input(e) if e[..] == [up]));
        assert!(
            matches!(&q[2], ControllerMsg::Input(e) if e[..] == [InputEvent::MouseDelta { dx: 5, dy: 5 }])
        );
        // 绝对移动和相对移动不互相合并
        tx.send(input(vec![InputEvent::MouseMove { x: 0.1, y: 0.1 }]))
            .unwrap();
        tx.send(input(vec![InputEvent::MouseDelta { dx: 1, dy: 1 }]))
            .unwrap();
        assert_eq!(queued(&tx).len(), 5);
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
