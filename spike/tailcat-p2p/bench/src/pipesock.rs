//! Candidate (ii): a quinn `AsyncUdpSocket` whose datagrams go through two
//! inherited pipes to the helper (2-byte big-endian length prefix), so the
//! direct path opens no local port.

use std::collections::VecDeque;
use std::fmt;
use std::fs::File;
use std::io::{self, IoSliceMut, Read, Write};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};

use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, UdpPoller};

const SEND_QUEUE: usize = 1024;
const RECV_QUEUE: usize = 4096;

#[derive(Default)]
struct SendSide {
    q: VecDeque<Vec<u8>>,
    wakers: Vec<Waker>,
    closed: bool,
}

#[derive(Default)]
struct RecvSide {
    q: VecDeque<Vec<u8>>,
    waker: Option<Waker>,
    closed: bool,
}

pub struct PipeSocket {
    send: Arc<(Mutex<SendSide>, Condvar)>,
    recv: Arc<Mutex<RecvSide>>,
    _recv_dropped: Arc<AtomicU64>,
}

impl fmt::Debug for PipeSocket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PipeSocket")
    }
}

impl PipeSocket {
    /// `to_helper` is written with datagrams for the tunnel, `from_helper`
    /// yields datagrams from the tunnel.
    pub fn new(to_helper: File, from_helper: File) -> Arc<Self> {
        let send: Arc<(Mutex<SendSide>, Condvar)> = Default::default();
        let recv: Arc<Mutex<RecvSide>> = Default::default();
        let dropped = Arc::new(AtomicU64::new(0));
        {
            let send = send.clone();
            std::thread::spawn(move || writer(send, to_helper));
        }
        {
            let recv = recv.clone();
            let dropped = dropped.clone();
            std::thread::spawn(move || reader(recv, from_helper, dropped));
        }
        Arc::new(Self {
            send,
            recv,
            _recv_dropped: dropped,
        })
    }
}

fn writer(send: Arc<(Mutex<SendSide>, Condvar)>, f: File) {
    let mut w = io::BufWriter::with_capacity(256 << 10, f);
    loop {
        let batch: Vec<Vec<u8>> = {
            let (m, cv) = &*send;
            let mut s = m.lock().unwrap();
            while s.q.is_empty() && !s.closed {
                s = cv.wait(s).unwrap();
            }
            if s.closed && s.q.is_empty() {
                return;
            }
            let batch = s.q.drain(..).collect();
            for wk in s.wakers.drain(..) {
                wk.wake();
            }
            batch
        };
        for d in &batch {
            let len = (d.len() as u16).to_be_bytes();
            if w.write_all(&len).and_then(|_| w.write_all(d)).is_err() {
                return;
            }
        }
        if w.flush().is_err() {
            return;
        }
    }
}

fn reader(recv: Arc<Mutex<RecvSide>>, mut f: File, dropped: Arc<AtomicU64>) {
    let mut hdr = [0u8; 2];
    loop {
        if f.read_exact(&mut hdr).is_err() {
            break;
        }
        let n = u16::from_be_bytes(hdr) as usize;
        let mut d = vec![0u8; n];
        if f.read_exact(&mut d).is_err() {
            break;
        }
        let mut r = recv.lock().unwrap();
        if r.q.len() >= RECV_QUEUE {
            dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        r.q.push_back(d);
        if let Some(w) = r.waker.take() {
            w.wake();
        }
    }
    let mut r = recv.lock().unwrap();
    r.closed = true;
    if let Some(w) = r.waker.take() {
        w.wake();
    }
}

#[derive(Debug)]
struct Poller(Arc<(Mutex<SendSide>, Condvar)>);

impl fmt::Debug for SendSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SendSide({})", self.q.len())
    }
}

impl UdpPoller for Poller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        let mut s = self.0.0.lock().unwrap();
        if s.q.len() < SEND_QUEUE {
            Poll::Ready(Ok(()))
        } else {
            s.wakers.push(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl AsyncUdpSocket for PipeSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Poller(self.send.clone()))
    }

    fn try_send(&self, t: &Transmit) -> io::Result<()> {
        let (m, cv) = &*self.send;
        let mut s = m.lock().unwrap();
        if s.q.len() >= SEND_QUEUE {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        match t.segment_size {
            Some(seg) => {
                for c in t.contents.chunks(seg) {
                    s.q.push_back(c.to_vec());
                }
            }
            None => s.q.push_back(t.contents.to_vec()),
        }
        cv.notify_one();
        Ok(())
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut r = self.recv.lock().unwrap();
        if r.q.is_empty() {
            if r.closed {
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            r.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let mut n = 0;
        while n < bufs.len().min(meta.len()) {
            let Some(d) = r.q.pop_front() else { break };
            let len = d.len().min(bufs[n].len());
            bufs[n][..len].copy_from_slice(&d[..len]);
            let mut m = RecvMeta::default();
            m.addr = crate::quic::pipe_peer();
            m.len = len;
            m.stride = len;
            meta[n] = m;
            n += 1;
        }
        Poll::Ready(Ok(n))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(crate::quic::pipe_local())
    }

    fn may_fragment(&self) -> bool {
        false
    }
}

impl Drop for PipeSocket {
    fn drop(&mut self) {
        let (m, cv) = &*self.send;
        m.lock().unwrap().closed = true;
        cv.notify_all();
    }
}
