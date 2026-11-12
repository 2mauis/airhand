//! Test-only, byte-preserving datagram channel. Loss is applied at send time;
//! FIFO serialization and propagation are separate. No protocol events are invented.
use super::{ChannelConfig, HandshakePacket, Loss, Observed, Side};
use quinn::{
    AsyncUdpSocket, UdpPoller,
    udp::{RecvMeta, Transmit},
};
use std::{
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::mpsc, time::Instant};

#[derive(Debug)]
struct Packet {
    bytes: Vec<u8>,
    from: SocketAddr,
}
#[derive(Debug)]
struct Scheduled {
    at: Instant,
    packet: Packet,
}
#[derive(Debug)]
pub struct State {
    pub cfg: ChannelConfig,
    pub losses: Vec<Loss>,
    pub start: Instant,
    pub origin: Option<Instant>,
    pub first_host_inbound: Option<Instant>,
    pub dropped_handshake: bool,
    pub trace: Vec<String>,
    pub observed: Observed,
    next_send: [Instant; 2],
    seed: u64,
}
impl State {
    pub fn new(cfg: &ChannelConfig, losses: &[Loss]) -> Arc<Mutex<Self>> {
        let now = Instant::now();
        Arc::new(Mutex::new(Self {
            cfg: cfg.clone(),
            losses: losses.to_vec(),
            start: now,
            origin: None,
            first_host_inbound: None,
            dropped_handshake: false,
            trace: Vec::new(),
            observed: Observed::default(),
            next_send: [now; 2],
            seed: cfg.seed,
        }))
    }
}
pub fn index(s: Side) -> usize {
    if s == Side::Host { 0 } else { 1 }
}
fn addr(s: Side) -> SocketAddr {
    if s == Side::Host {
        "127.0.0.1:5000"
    } else {
        "127.0.0.1:5001"
    }
    .parse()
    .unwrap()
}

#[derive(Debug)]
pub struct Socket {
    side: Side,
    incoming: Mutex<mpsc::UnboundedReceiver<Packet>>,
    outgoing: mpsc::UnboundedSender<Scheduled>,
    state: Arc<Mutex<State>>,
}
pub fn pair(state: Arc<Mutex<State>>) -> [Arc<Socket>; 2] {
    let (host_tx, host_rx) = mpsc::unbounded_channel();
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let (host_schedule, host_pending) = mpsc::unbounded_channel();
    let (ctrl_schedule, ctrl_pending) = mpsc::unbounded_channel();
    tokio::spawn(deliver(host_pending, ctrl_tx));
    tokio::spawn(deliver(ctrl_pending, host_tx));
    [
        Arc::new(Socket {
            side: Side::Host,
            incoming: Mutex::new(host_rx),
            outgoing: host_schedule,
            state: state.clone(),
        }),
        Arc::new(Socket {
            side: Side::Controller,
            incoming: Mutex::new(ctrl_rx),
            outgoing: ctrl_schedule,
            state,
        }),
    ]
}
// A single ordered delivery queue preserves FIFO when timer deadlines round
// into the same Tokio millisecond. Jitter may still reorder distinct deadlines.
async fn deliver(mut rx: mpsc::UnboundedReceiver<Scheduled>, tx: mpsc::UnboundedSender<Packet>) {
    let mut queue = std::collections::BTreeMap::new();
    let mut sequence = 0u64;
    loop {
        let next = queue.first_key_value().map(|((at, _), _)| *at);
        tokio::select! {
            biased;
            packet = rx.recv() => match packet {
                Some(packet) => { queue.insert((packet.at, sequence), packet.packet); sequence += 1; }
                None => break,
            },
            _ = async { if let Some(at) = next { tokio::time::sleep_until(at).await; } else { std::future::pending::<()>().await; } } => {
                while queue.first_key_value().is_some_and(|((at,_),_)| *at <= Instant::now()) {
                    let (_, packet) = queue.pop_first().unwrap();
                    if tx.send(packet).is_err() { return; }
                }
            }
        }
    }
}
#[derive(Debug)]
struct Writable;
impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// QUIC v1 long-header packet types, including coalesced packets. Protected packet
// numbers/payload are skipped using the public length field, never decrypted.
pub fn long_types(mut b: &[u8]) -> Vec<u8> {
    fn varint(b: &[u8], p: &mut usize) -> Option<usize> {
        let first = *b.get(*p)?;
        let n = 1 << (first >> 6);
        let mut v = (first & 63) as usize;
        for i in 1..n {
            v = (v << 8) | *b.get(*p + i)? as usize;
        }
        *p += n;
        Some(v)
    }
    let mut types = Vec::new();
    while b.first().is_some_and(|b| b & 0x80 != 0) && b.len() >= 7 {
        let typ = (b[0] >> 4) & 3;
        types.push(typ);
        let mut p = 5;
        let Some(&n) = b.get(p) else { break };
        p += 1 + n as usize;
        let Some(&n) = b.get(p) else { break };
        p += 1 + n as usize;
        if typ == 0 {
            let Some(n) = varint(b, &mut p) else { break };
            p += n;
        }
        let Some(n) = varint(b, &mut p) else { break };
        p += n;
        let Some(rest) = b.get(p..) else { break };
        b = rest;
    }
    types
}
impl AsyncUdpSocket for Socket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(addr(self.side))
    }
    fn may_fragment(&self) -> bool {
        false
    }
    fn try_send(&self, tx: &Transmit<'_>) -> io::Result<()> {
        let now = Instant::now();
        let mut s = self.state.lock().unwrap();
        let types = long_types(tx.contents);
        if s.origin.is_none() {
            let at = now - s.start;
            s.trace.push(format!(
                "{at:?} {:?} send {} bytes long={types:?}",
                self.side,
                tx.contents.len()
            ));
            let drop = match s.cfg.handshake_drop {
                Some(HandshakePacket::HostFirstFlight) => self.side == Side::Host,
                Some(HandshakePacket::ControllerFinished) => {
                    self.side == Side::Controller && types.contains(&2)
                }
                _ => false,
            };
            if drop && !s.dropped_handshake {
                s.dropped_handshake = true;
                s.trace.push("DROP handshake datagram".into());
                return Ok(());
            }
        }
        if let Some(origin) = s.origin {
            let at = now - origin;
            if s.losses.iter().any(|l| {
                l.dir.is_none_or(|d| d.from == self.side)
                    && at >= l.from
                    && l.until.is_none_or(|u| at < u)
            }) {
                return Ok(());
            }
        }
        let i = index(self.side);
        let queue = s.next_send[i].saturating_duration_since(now);

        let serialization = s
            .cfg
            .bandwidth_bps
            .map(|bps| Duration::from_secs_f64(tx.contents.len() as f64 * 8.0 / bps as f64))
            .unwrap_or_default();
        if s.cfg
            .queue_limit
            .is_some_and(|limit| queue + serialization > limit)
        {
            return Ok(());
        }
        let depart = now.max(s.next_send[i]) + serialization;
        s.next_send[i] = depart;
        let up = s.cfg.up.unwrap_or(s.cfg.rtt / 2);
        let delay = if self.side == Side::Controller {
            up
        } else {
            s.cfg.rtt - up
        };
        s.seed = s.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let jitter = s.cfg.jitter.as_micros() as i64;
        let delta = if jitter == 0 {
            0
        } else {
            ((s.seed >> 32) % (2 * jitter + 1) as u64) as i64 - jitter
        };
        let delay = Duration::from_micros((delay.as_micros() as i64 + delta).max(0) as u64);
        let queued_rtt = s.cfg.rtt + queue + serialization;
        s.observed.max_queued_rtt = Some(
            s.observed
                .max_queued_rtt
                .unwrap_or_default()
                .max(queued_rtt),
        );
        drop(s);
        let packet = Packet {
            bytes: tx.contents.to_vec(),
            from: addr(self.side),
        };
        let _ = self.outgoing.send(Scheduled {
            at: depart + delay,
            packet,
        });
        Ok(())
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        match self.incoming.lock().unwrap().poll_recv(cx) {
            Poll::Ready(Some(packet)) => {
                let n = packet.bytes.len();
                if n > bufs[0].len() {
                    return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
                }
                bufs[0][..n].copy_from_slice(&packet.bytes);
                meta[0] = RecvMeta {
                    addr: packet.from,
                    len: n,
                    stride: n,
                    ecn: None,
                    dst_ip: Some(addr(self.side).ip()),
                };
                let now = Instant::now();
                let mut s = self.state.lock().unwrap();
                if self.side == Side::Host {
                    s.first_host_inbound.get_or_insert(now);
                }
                if let Some(origin) = s.origin {
                    s.observed.inbound.push((now - origin, self.side));
                } else {
                    let at = now - s.start;
                    s.trace
                        .push(format!("{at:?} {:?} recv {n} bytes", self.side));
                }
                Poll::Ready(Ok(1))
            }
            _ => Poll::Pending,
        }
    }
}
