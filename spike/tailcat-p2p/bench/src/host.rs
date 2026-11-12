//! Host (被控端) side of the bench.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use airhand_link::LinkConfig;
use airhand_link::net::connect_signal;
use airhand_link::transport::open_session;
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{ClientMsg, ServerMsg};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Context, Result, bail};
use quinn::{Connection, Endpoint, SendStream, TokioRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::helper::{Chan, Helper};
use crate::lane::{Tx, paced_write, reader};
use crate::pipesock::PipeSocket;
use crate::quic::{self, hex};
use crate::{Args, Log, clk_tck, cpu_ticks, now_us};

pub(crate) const TUNNEL_PORT: u16 = 4433;

#[derive(Default)]
pub(crate) struct Shared {
    relay_code: AtomicU64,
    relay_control: Mutex<Option<Tx>>,
}

pub async fn run(args: Args) -> Result<()> {
    let log = Log::open(args.get("log"))?;
    let shared = Arc::new(Shared::default());
    let region: Value = match args.get("region") {
        Some(f) => serde_json::from_str(&std::fs::read_to_string(f)?)?,
        None => Value::Null,
    };
    let poll_ms = args.num("poll-ms", 100u64);
    if let Some(server) = args.get("relay") {
        let sh = shared.clone();
        let server = server.clone();
        let log = log.clone();
        let region = region.clone();
        tokio::spawn(async move {
            if let Err(e) = relay_host(&server, sh, log, region, poll_ms).await {
                eprintln!("relay host: {e:#}");
                std::process::exit(1);
            }
        });
        for _ in 0..100 {
            if shared_code(&args, &shared) != 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let path = args.req("sig")?;
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let listen = args.get("listen").cloned().unwrap_or("0.0.0.0:0".into());
    eprintln!("host ready");
    loop {
        let (s, _) = listener.accept().await?;
        let ctx = SessionCtx {
            shared: shared.clone(),
            region: region.clone(),
            listen: listen.clone(),
            poll_ms,
            log: log.clone(),
        };
        tokio::spawn(async move {
            if let Err(e) = session(s, ctx).await {
                eprintln!("session: {e:#}");
            }
        });
    }
}

fn shared_code(_: &Args, s: &Shared) -> u64 {
    s.relay_code.load(Ordering::Relaxed)
}

#[derive(Clone)]
pub(crate) struct SessionCtx {
    shared: Arc<Shared>,
    pub region: Value,
    listen: String,
    pub poll_ms: u64,
    pub log: Log,
}

async fn session(s: UnixStream, ctx: SessionCtx) -> Result<()> {
    let (r, mut w) = s.into_split();
    let mut lines = BufReader::new(r).lines();
    let req: Value = serde_json::from_str(&lines.next_line().await?.context("no request")?)?;
    let via = req["via"].as_str().unwrap_or("udp").to_string();
    let relay_code = ctx.shared.relay_code.load(Ordering::Relaxed);
    if via == "relay" {
        w.write_all(format!("{}\n", json!({"relay_code": relay_code})).as_bytes()).await?;
        let _ = lines.next_line().await;
        return Ok(());
    }
    let cert = quic::host_cert()?;
    let token: [u8; 16] = rand::random();
    let server_cfg = quic::server_config(&cert)?;
    let mut helper = None;
    let mut offer = json!({"fp": hex(&cert.fp), "token": hex(&token), "relay_code": relay_code});
    let endpoint = match via.as_str() {
        "plain" => {
            let sock = std::net::UdpSocket::bind(&ctx.listen)?;
            offer["quic_addr"] = sock.local_addr()?.to_string().into();
            Endpoint::new(quic::direct_endpoint_config(), Some(server_cfg), sock, Arc::new(TokioRuntime))?
        }
        "udp" | "pipe" => {
            let chan = if via == "udp" { Chan::Udp } else { Chan::Pipe };
            let mut h = Helper::spawn("host", chan, ctx.log.clone()).await?;
            let ep = if chan == Chan::Udp {
                let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
                h.set_peer(sock.local_addr()?.port()).await?;
                Endpoint::new(quic::direct_endpoint_config(), Some(server_cfg), sock, Arc::new(TokioRuntime))?
            } else {
                let (w, r) = h.pipes.take().unwrap();
                Endpoint::new_with_abstract_socket(
                    quic::direct_endpoint_config(),
                    Some(server_cfg),
                    PipeSocket::new(w, r),
                    Arc::new(TokioRuntime),
                )?
            };
            let key = req["client_key"].as_str().unwrap_or_default();
            let addr = h.serve(key, TUNNEL_PORT, &ctx.region, ctx.poll_ms).await?;
            offer["addr"] = addr.into();
            helper = Some(h);
            ep
        }
        v => bail!("unknown via {v}"),
    };
    w.write_all(format!("{offer}\n").as_bytes()).await?;
    offer["addr"] = Value::Null;

    let helper = helper.map(Arc::new);
    let result = async {
        let inc = tokio::time::timeout(Duration::from_secs(60), endpoint.accept())
            .await
            .context("no direct QUIC connection")?
            .context("endpoint closed")?;
        let conn = inc.await?;
        let (ctl_tx, ctl_rx) = conn.accept_bi().await?;
        let (video_tx, _video_rx) = conn.accept_bi().await?;
        let (_audio_tx, _audio_rx) = conn.accept_bi().await?;
        let mut rx = reader(ctl_rx);
        let tx = Tx::new(ctl_tx);
        let (_, first) = rx.recv().await.context("no token")?;
        if first["token"].as_str() != Some(&hex(&token)) {
            bail!("bad token");
        }
        let host_direct = helper.as_ref().and_then(|h| {
            h.history
                .lock()
                .unwrap()
                .iter()
                .find(|v| v["ev"] == "path" && v["path"] == "direct")
                .and_then(|v| v["t_us"].as_i64())
        });
        tx.send(&json!({"commit": true, "host_direct_t_us": host_direct, "t_us": now_us()}))
            .await?;
        control_loop(tx, rx, video_tx, Some(conn.clone()), helper.clone(), ctx.clone()).await;
        conn.close(0u32.into(), b"bye");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    endpoint.close(0u32.into(), b"");
    if let Some(h) = helper {
        if let Ok(h) = Arc::try_unwrap(h) {
            h.close().await;
        }
    }
    result
}

/// Serves bench commands on a control lane; `video` carries bulk data.
async fn control_loop(
    tx: Tx,
    mut rx: tokio::sync::mpsc::Receiver<(i64, Value)>,
    video: SendStream,
    conn: Option<Connection>,
    helper: Option<Arc<Helper>>,
    ctx: SessionCtx,
) {
    let video = Arc::new(Mutex::new(video));
    let last_ping = Arc::new(AtomicI64::new(now_us()));
    let k_misses = Arc::new(AtomicU64::new(0));
    let mut watch: Option<tokio::task::JoinHandle<()>> = None;
    while let Some((_, m)) = rx.recv().await {
        let Some(cmd) = m["cmd"].as_str() else { continue };
        match cmd {
            "ping" => {
                last_ping.store(now_us(), Ordering::Relaxed);
                let _ = tx.send(&json!({"pong": m["seq"], "t_us": now_us()})).await;
            }
            "send" => {
                let (mbps, secs) = (m["mbps"].as_f64().unwrap_or(10.0), m["secs"].as_f64().unwrap_or(5.0));
                let (tx, video) = (tx.clone(), video.clone());
                tokio::spawn(async move {
                    let mut v = video.lock().await;
                    let r = paced_write(&mut v, mbps, secs).await;
                    let (sent, skipped) = r.unwrap_or((0, 0));
                    let _ = tx.send(&json!({"send_done": true, "bytes": sent, "skipped": skipped})).await;
                });
            }
            "watch" => {
                let k = m["k_ms"].as_i64().unwrap_or(2000) * 1000;
                let (last, misses, log) = (last_ping.clone(), k_misses.clone(), ctx.log.clone());
                last.store(now_us(), Ordering::Relaxed);
                if let Some(w) = watch.take() {
                    w.abort();
                }
                watch = Some(tokio::spawn(async move {
                    let mut tripped = false;
                    loop {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let gap = now_us() - last.load(Ordering::Relaxed);
                        if gap > k && !tripped {
                            tripped = true;
                            misses.fetch_add(1, Ordering::Relaxed);
                            log.write(json!({"src": "host", "k_miss": true, "gap_us": gap}));
                        } else if gap <= k {
                            tripped = false;
                        }
                    }
                }));
            }
            "fence" => {
                let id = m["id"].clone();
                let relay = ctx.shared.relay_control.lock().await.clone();
                let t = now_us();
                let msg = json!({"fence": id, "t_us": t});
                if let Some(r) = relay {
                    let (a, b) = tokio::join!(r.send(&msg), tx.send(&msg));
                    let _ = (a, b);
                } else {
                    let _ = tx.send(&msg).await;
                }
            }
            "stats" => {
                let mut out = json!({"stats": true, "t_us": now_us(), "clk_tck": clk_tck(),
                    "self_ticks": cpu_ticks(std::process::id()),
                    "k_misses": k_misses.load(Ordering::Relaxed)});
                if let Some(c) = &conn {
                    let s = c.stats();
                    out["quic"] = json!({
                        "sent_packets": s.path.sent_packets, "lost_packets": s.path.lost_packets,
                        "lost_bytes": s.path.lost_bytes, "rtt_us": s.path.rtt.as_micros() as u64,
                        "cwnd": s.path.cwnd, "congestion_events": s.path.congestion_events,
                        "udp_tx_datagrams": s.udp_tx.datagrams, "udp_tx_bytes": s.udp_tx.bytes,
                        "udp_rx_datagrams": s.udp_rx.datagrams, "current_mtu": s.path.current_mtu,
                    });
                }
                if let Some(h) = &helper {
                    out["helper_ticks"] = cpu_ticks(h.pid).into();
                    if let Ok(st) = h.stats().await {
                        out["helper"] = st;
                    }
                    let hist = h.history.lock().unwrap();
                    out["helper_paths"] = hist
                        .iter()
                        .filter(|v| v["ev"] == "path")
                        .map(|v| json!({"path": v["path"], "t_us": v["t_us"]}))
                        .collect::<Vec<_>>()
                        .into();
                }
                let _ = tx.send(&out).await;
            }
            _ => {}
        }
    }
    if let Some(w) = watch {
        w.abort();
    }
}

async fn relay_host(server: &str, shared: Arc<Shared>, log: Log, region: Value, poll_ms: u64) -> Result<()> {
    let cfg = LinkConfig::new(server, "", "", "linux")?;
    let mut sig = connect_signal(&cfg, Role::Host)
        .await
        .map_err(|p| anyhow::anyhow!("relay: {p:?}"))?;
    write_msg(&mut sig.send, &ClientMsg::Register { reclaim: None, keep: false }).await?;
    let mut buf = Vec::new();
    loop {
        let Some(msg) = read_msg::<_, ServerMsg>(&mut sig.recv, MAX_SIGNAL_MSG, &mut buf).await? else {
            bail!("relay closed")
        };
        match msg {
            ServerMsg::Registered { code, .. } => shared.relay_code.store(code as u64, Ordering::Relaxed),
            ServerMsg::Incoming { session_id, .. } => {
                let conn = sig.conn.clone();
                let shared = shared.clone();
                let log = log.clone();
                let (region, poll_ms) = (region.clone(), poll_ms);
                tokio::spawn(async move {
                    let Ok(s) = open_session(&conn, session_id, Role::Host).await else { return };
                    let tx = Tx::new(s.control.0);
                    let mut rx = reader(s.control.1);
                    let ctx = SessionCtx {
                        shared: shared.clone(),
                        region,
                        listen: String::new(),
                        poll_ms,
                        log,
                    };
                    let _audio = s.audio;
                    let Some(first) = rx.recv().await else { return };
                    if first.1.get("hello").is_some() {
                        crate::consent::host_session(tx, rx, s.video.0, first.1, ctx).await;
                        return;
                    }
                    let (ftx, frx) = tokio::sync::mpsc::channel(1024);
                    tokio::spawn(async move {
                        let _ = ftx.send(first).await;
                        while let Some(m) = rx.recv().await {
                            if ftx.send(m).await.is_err() {
                                break;
                            }
                        }
                    });
                    *shared.relay_control.lock().await = Some(tx.clone());
                    control_loop(tx, frx, s.video.0, Some(conn), None, ctx).await;
                    *shared.relay_control.lock().await = None;
                });
            }
            _ => {}
        }
    }
}
