//! Glue-free PV investigation. Imports Quinn/Tokio/Rustls only: no airhand
//! transport config, signal source, or acceptance fixture. All times are virtual.
//! cargo run -p airhand-link --example pv_raw -- handshake|burst|priority
use quinn::{
    AsyncUdpSocket, UdpPoller,
    udp::{RecvMeta, Transmit},
};
use std::{
    future::{Future, poll_fn},
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::{sync::mpsc, time::Instant};
fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}
#[derive(Debug)]
struct Lab {
    start: Instant,
    origin: Option<Instant>,
    first: Option<Instant>,
    drop_first: bool,
    rtt: Duration,
    jitter: bool,
    seed: u64,
    rate: bool,
    burst: bool,
    next: [Instant; 2],
    trace: Vec<String>,
    progress: [Vec<Duration>; 2],
    sent: [Vec<Duration>; 2],
    inbound: [Vec<Duration>; 2],
    media: u64,
}
#[derive(Debug)]
struct Packet {
    data: Vec<u8>,
    from: SocketAddr,
}
#[derive(Debug)]
struct Wire {
    side: usize,
    rx: Mutex<mpsc::UnboundedReceiver<Packet>>,
    tx: mpsc::UnboundedSender<(Instant, Packet)>,
    lab: Arc<Mutex<Lab>>,
}
fn addr(side: usize) -> SocketAddr {
    format!("127.0.0.1:{}", 6000 + side).parse().unwrap()
}
#[derive(Debug)]
struct Ready;
impl UdpPoller for Ready {
    fn poll_writable(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl AsyncUdpSocket for Wire {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(addr(self.side))
    }
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Ready)
    }
    fn may_fragment(&self) -> bool {
        false
    }
    fn try_send(&self, t: &Transmit<'_>) -> io::Result<()> {
        let now = Instant::now();
        let mut l = self.lab.lock().unwrap();
        if l.origin.is_none() {
            let at = now - l.start;
            l.trace.push(format!(
                "{at:?} side {} send {} bytes first={:02x}",
                self.side,
                t.contents.len(),
                t.contents[0]
            ));
            if self.side == 0 && l.drop_first {
                l.drop_first = false;
                l.trace.push("DROP first host datagram".into());
                return Ok(());
            }
        }
        if let Some(origin) = l.origin {
            let at = now - origin;
            if l.burst && at >= ms(5000) && at < ms(5100) {
                return Ok(());
            }
        }
        let queued = l.next[self.side].saturating_duration_since(now);
        let serialization = if l.rate {
            Duration::from_secs_f64(t.contents.len() as f64 * 8.0 / 20_000_000.0)
        } else {
            Duration::ZERO
        };
        if l.rate && queued + serialization > ms(150) {
            return Ok(());
        }
        let depart = now.max(l.next[self.side]) + serialization;
        l.next[self.side] = depart;
        // Same seeded uniform per-datagram jitter as the specified fixture, but
        // independent code. Burst uses d1=0, d2=400ms (first failing split).
        let mut delay = if l.burst {
            if self.side == 0 {
                l.rtt
            } else {
                Duration::ZERO
            }
        } else {
            l.rtt / 2
        };
        l.seed = l.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        if l.jitter {
            let j = ((l.seed >> 32) % 100001) as i64 - 50000;
            delay = Duration::from_micros((delay.as_micros() as i64 + j).max(0) as u64);
        }
        drop(l);
        let p = Packet {
            data: t.contents.to_vec(),
            from: addr(self.side),
        };
        let _ = self.tx.send((depart + delay, p));
        Ok(())
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        match self.rx.lock().unwrap().poll_recv(cx) {
            Poll::Ready(Some(p)) => {
                let n = p.data.len();
                bufs[0][..n].copy_from_slice(&p.data);
                meta[0] = RecvMeta {
                    addr: p.from,
                    len: n,
                    stride: n,
                    ecn: None,
                    dst_ip: Some(addr(self.side).ip()),
                };
                let now = Instant::now();
                let mut l = self.lab.lock().unwrap();
                if self.side == 0 {
                    l.first.get_or_insert(now);
                }
                if let Some(origin) = l.origin {
                    l.inbound[self.side].push(now - origin);
                } else {
                    let at = now - l.start;
                    l.trace
                        .push(format!("{at:?} side {} recv {n} bytes", self.side));
                }
                Poll::Ready(Ok(1))
            }
            _ => Poll::Pending,
        }
    }
}
fn transport() -> quinn::TransportConfig {
    let mut t = quinn::TransportConfig::default();
    t.max_concurrent_uni_streams(32u32.into())
        .max_concurrent_bidi_streams(256u32.into());
    t.keep_alive_interval(Some(ms(250)))
        .max_idle_timeout(Some(ms(60000).try_into().unwrap()));
    t.stream_receive_window((16u32 * 1024 * 1024).into())
        .receive_window((64u32 * 1024 * 1024).into())
        .send_window(64 * 1024 * 1024);
    t.initial_mtu(1200).min_mtu(1200).mtu_discovery_config(None);
    t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    t
}
async fn endpoints(
    lab: Arc<Mutex<Lab>>,
) -> (
    quinn::Endpoint,
    quinn::Endpoint,
    quinn::Connection,
    quinn::Connection,
) {
    let (ht, hr) = mpsc::unbounded_channel();
    let (ct, cr) = mpsc::unbounded_channel();
    let (hq, hqr) = mpsc::unbounded_channel::<(Instant, Packet)>();
    let (cq, cqr) = mpsc::unbounded_channel::<(Instant, Packet)>();
    // Independent delivery loops: sort by exact arrival time and sequence, so
    // millisecond timer rounding cannot introduce extra packet reordering.
    for (mut queue, out) in [(hqr, ct), (cqr, ht)] {
        tokio::spawn(async move {
            let mut packets = std::collections::BTreeMap::new();
            let mut seq = 0u64;
            loop {
                let next = packets.first_key_value().map(|((time, _), _)| *time);
                tokio::select! {biased;
                    p=queue.recv()=>match p {Some((at,p))=>{packets.insert((at,seq),p);seq+=1;},None=>break},
                    _=async{match next{Some(t)=>tokio::time::sleep_until(t).await,None=>std::future::pending::<()>().await}}=>{
                        while packets.first_key_value().is_some_and(|((at,_),_)|*at<=Instant::now()) {
                            let (_,p)=packets.pop_first().unwrap();if out.send(p).is_err(){return;}
                        }
                    }
                }
            }
        });
    }
    let hs = Arc::new(Wire {
        side: 0,
        rx: Mutex::new(hr),
        tx: hq,
        lab: lab.clone(),
    });
    let cs = Arc::new(Wire {
        side: 1,
        rx: Mutex::new(cr),
        tx: cq,
        lab,
    });
    let cert = rustls::pki_types::CertificateDer::from(
        include_bytes!("../tests/direct_quinn/fixtures/cert.der").to_vec(),
    );
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(
        include_bytes!("../tests/direct_quinn/fixtures/key.der").to_vec(),
    );
    let mut server = quinn::ServerConfig::with_single_cert(vec![cert.clone()], key.into()).unwrap();
    server.transport_config(Arc::new(transport()));
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).unwrap();
    let mut client = quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap();
    client.transport_config(Arc::new(transport()));
    let mut ec = quinn::EndpointConfig::default();
    ec.max_udp_payload_size(1232).unwrap();
    let h = quinn::Endpoint::new_with_abstract_socket(
        ec.clone(),
        Some(server),
        hs,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let mut c =
        quinn::Endpoint::new_with_abstract_socket(ec, None, cs, Arc::new(quinn::TokioRuntime))
            .unwrap();
    c.set_default_client_config(client);
    let connecting = c.connect(h.local_addr().unwrap(), "localhost").unwrap();
    let (hc, cc) = tokio::time::timeout(ms(5000), async {
        tokio::join!(async { h.accept().await.unwrap().await.unwrap() }, async {
            connecting.await.unwrap()
        })
    })
    .await
    .expect("raw handshake timeout");
    (h, c, hc, cc)
}
fn probes(
    conn: quinn::Connection,
    side: usize,
    priority: i32,
    lab: Arc<Mutex<Lab>>,
    tasks: &mut tokio::task::JoinSet<()>,
) {
    let accept = conn.clone();
    tasks.spawn(async move {
        while let Ok(mut r) = accept.accept_uni().await {
            if r.read_to_end(1).await.is_err() {
                break;
            }
        }
    });
    tasks.spawn(async move {
        let in_flight=Arc::new(AtomicUsize::new(0));let mut waiters=tokio::task::JoinSet::new();let mut ticks=tokio::time::interval(ms(250));
        loop {tokio::select! {
            _=conn.closed()=>break,
            Some(_)=waiters.join_next(),if !waiters.is_empty()=>{},
            _=ticks.tick()=>{
                if in_flight.load(Ordering::SeqCst)>=16 {continue;}
                let mut open=std::pin::pin!(conn.open_uni());
                let Poll::Ready(Ok(mut s))=poll_fn(|cx|Poll::Ready(open.as_mut().poll(cx))).await else{continue;};
                s.set_priority(priority).unwrap();let lab=lab.clone();let count=in_flight.clone();count.fetch_add(1,Ordering::SeqCst);
                waiters.spawn(async move {
                    if s.write_all(&[1]).await.is_ok() && s.finish().is_ok() {
                        {let mut l=lab.lock().unwrap();let origin=l.origin.unwrap();l.sent[side].push(Instant::now()-origin);}
                        if let Ok(None)=s.stopped().await {let mut l=lab.lock().unwrap();let origin=l.origin.unwrap();l.progress[side].push(Instant::now()-origin);}
                    }
                    count.fetch_sub(1,Ordering::SeqCst);
                });
            }
        }}
    });
}
async fn run(mode: &str, rtt: u64, priority: i32) {
    let now = Instant::now();
    let lab = Arc::new(Mutex::new(Lab {
        start: now,
        origin: None,
        first: None,
        drop_first: mode == "handshake",
        rtt: ms(rtt),
        jitter: mode == "burst",
        seed: 1,
        rate: mode == "priority",
        burst: mode == "burst",
        next: [now; 2],
        trace: Vec::new(),
        progress: Default::default(),
        sent: Default::default(),
        inbound: Default::default(),
        media: 0,
    }));
    let (_he, _ce, h, c) = endpoints(lab.clone()).await;
    let origin = Instant::now();
    {
        let mut l = lab.lock().unwrap();
        let elapsed = origin - l.first.unwrap();
        println!(
            "mode={mode} rtt={rtt}ms host_handshake_since_first={elapsed:?} H_with_IPC={:?}",
            elapsed + ms(50)
        );
        if mode == "handshake" {
            println!("{}", l.trace.join("\n"));
            return;
        }
        l.origin = Some(origin);
    }
    let mut tasks = tokio::task::JoinSet::new();
    probes(h.clone(), 0, priority, lab.clone(), &mut tasks);
    probes(c.clone(), 1, priority, lab.clone(), &mut tasks);
    if mode == "priority" {
        let send = h.clone();
        tasks.spawn(async move {
            let (mut s, _r) = send.open_bi().await.unwrap();
            let data = vec![0u8; 65536];
            while s.write_all(&data).await.is_ok() {}
        });
        let recv = c.clone();
        let lab = lab.clone();
        tasks.spawn(async move {
            let (_s, mut r) = recv.accept_bi().await.unwrap();
            let mut data = vec![0; 65536];
            while let Ok(Some(n)) = r.read(&mut data).await {
                lab.lock().unwrap().media += n as u64;
            }
        });
    }
    let end = origin + ms(if mode == "priority" { 60000 } else { 10100 });
    let mut ticks = tokio::time::interval(ms(5));
    let mut max_srtt = Duration::ZERO;
    loop {
        tokio::select! {biased;_=tokio::time::sleep_until(end)=>break,_=ticks.tick()=>{max_srtt=max_srtt.max(h.stats().path.rtt).max(c.stats().path.rtt);}}
    }
    let l = lab.lock().unwrap();
    for side in 0..2 {
        let p = &l.progress[side];
        let gap = p.windows(2).map(|w| w[1] - w[0]).max();
        let seconds = (0..60)
            .filter(|s| p.iter().any(|t| t.as_secs() == *s))
            .count();
        println!(
            "side={side} priority={priority} sent={} progress={} progress_seconds={seconds}/60 max_gap={gap:?} last={:?}",
            l.sent[side].len(),
            p.len(),
            p.last()
        );
        if mode == "burst" {
            println!(
                "near burst: {:?}",
                p.iter()
                    .filter(|t| **t > ms(4000) && **t < ms(7000))
                    .collect::<Vec<_>>()
            );
        }
    }
    println!(
        "media_bytes={} max_measured_srtt={max_srtt:?} host_close={:?} controller_close={:?}",
        l.media,
        h.close_reason(),
        c.close_reason()
    );
}
fn main() {
    let mode = std::env::args().nth(1).unwrap_or("handshake".into());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .unwrap()
        .block_on(async {
            match mode.as_str() {
                "handshake" => {
                    for rtt in [100, 200, 300, 350, 375, 400] {
                        run("handshake", rtt, i32::MAX).await;
                    }
                }
                "burst" => run("burst", 400, i32::MAX).await,
                "priority" => {
                    for priority in [0, i32::MAX] {
                        run("priority", 250, priority).await;
                    }
                }
                _ => panic!("handshake|burst|priority"),
            }
        });
}
