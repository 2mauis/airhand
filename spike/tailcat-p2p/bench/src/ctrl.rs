//! Controller (控制端) side of the bench: sets up the session, runs one test,
//! prints a JSON summary on stdout.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use airhand_link::LinkConfig;
use airhand_link::net::connect_signal;
use airhand_link::transport::open_session;
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{ClientMsg, ServerMsg};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Context, Result, anyhow, bail};
use quinn::{Connection, Endpoint, RecvStream, SendStream, TokioRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

use crate::helper::{Chan, Helper};
use crate::lane::{Tx, reader};
use crate::pipesock::PipeSocket;
use crate::quic;
use crate::{Args, Log, clk_tck, cpu_ticks, now_us};

const TUNNEL_PORT: u16 = 4433;

pub(crate) struct Lanes {
    pub tx: Tx,
    pub rx: mpsc::Receiver<(i64, Value)>,
    pub video_bytes: Arc<AtomicU64>,
    /// Arrival time (µs) of the latest video bytes; -1 once the stream ended.
    pub video_last_us: Arc<std::sync::atomic::AtomicI64>,
    pub video_end_us: Arc<std::sync::atomic::AtomicI64>,
    _keep: Vec<SendStream>,
}

impl Lanes {
    pub fn new(control: (SendStream, RecvStream), video: (SendStream, RecvStream), audio: SendStream) -> Self {
        let video_bytes = Arc::new(AtomicU64::new(0));
        let video_last_us = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let video_end_us = Arc::new(std::sync::atomic::AtomicI64::new(0));
        let (vb, vl, ve) = (video_bytes.clone(), video_last_us.clone(), video_end_us.clone());
        let mut vr = video.1;
        tokio::spawn(async move {
            let mut buf = vec![0u8; 256 << 10];
            while let Ok(Some(n)) = vr.read(&mut buf).await {
                vb.fetch_add(n as u64, Ordering::Relaxed);
                vl.store(now_us(), Ordering::Relaxed);
            }
            ve.store(now_us(), Ordering::Relaxed);
        });
        Self {
            tx: Tx::new(control.0),
            rx: reader(control.1),
            video_bytes,
            video_last_us,
            video_end_us,
            _keep: vec![video.0, audio],
        }
    }

    pub async fn request(&mut self, v: Value, want: &str) -> Result<Value> {
        self.tx.send(&v).await?;
        self.expect(want, Duration::from_secs(10)).await.map(|(_, v)| v)
    }

    pub async fn expect(&mut self, want: &str, within: Duration) -> Result<(i64, Value)> {
        tokio::time::timeout(within, async {
            loop {
                let (t, v) = self.rx.recv().await.ok_or_else(|| anyhow!("lane closed"))?;
                if v.get(want).is_some() {
                    return Ok((t, v));
                }
            }
        })
        .await
        .with_context(|| format!("waiting for {want}"))?
    }
}

struct Session {
    helper: Option<Helper>,
    endpoint: Option<Endpoint>,
    conn: Option<Connection>,
    direct: Option<Lanes>,
    relay: Option<Lanes>,
    _relay_conn: Option<Connection>,
    timings: Value,
    _sig: UnixStream,
}

async fn setup(args: &Args, via: &str, with_relay: bool, log: &Log) -> Result<Session> {
    let mut sig = UnixStream::connect(args.req("sig")?).await?;
    let mut t = json!({"t_start": now_us()});
    let chan = match via {
        "udp" => Some(Chan::Udp),
        "pipe" => Some(Chan::Pipe),
        _ => None,
    };
    let mut helper = None;
    let mut req = json!({"via": via});
    if let Some(chan) = chan {
        let mut h = Helper::spawn("ctrl", chan, log.clone()).await?;
        let key = h.init(TUNNEL_PORT, args.num("ping-ms", 250), args.num("misses", 2)).await?;
        req["client_key"] = key.into();
        t["t_key"] = now_us().into();
        helper = Some(h);
    }
    t["t_req"] = now_us().into();
    sig.write_all(format!("{req}\n").as_bytes()).await?;
    let mut line = String::new();
    {
        let mut r = BufReader::new(&mut sig);
        r.read_line(&mut line).await?;
    }
    let offer: Value = serde_json::from_str(&line).context("bad offer")?;
    t["t_offer"] = now_us().into();

    let mut s = Session {
        helper: None,
        endpoint: None,
        conn: None,
        direct: None,
        relay: None,
        _relay_conn: None,
        timings: Value::Null,
        _sig: sig,
    };

    if via != "relay" {
        let fp = quic::unhex32(offer["fp"].as_str().unwrap_or_default())?;
        let token = offer["token"].as_str().unwrap_or_default().to_string();
        let mut ep;
        let target;
        match chan {
            None => {
                let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
                ep = Endpoint::new(quic::direct_endpoint_config(), None, sock, Arc::new(TokioRuntime))?;
                target = offer["quic_addr"].as_str().unwrap_or_default().parse()?;
            }
            Some(chan) => {
                let h = helper.as_mut().unwrap();
                if chan == Chan::Udp {
                    let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
                    h.set_peer(sock.local_addr()?.port()).await?;
                    ep = Endpoint::new(quic::direct_endpoint_config(), None, sock, Arc::new(TokioRuntime))?;
                    target = format!("127.0.0.1:{}", h.chan_port.unwrap()).parse()?;
                } else {
                    let (w, r) = h.pipes.take().unwrap();
                    ep = Endpoint::new_with_abstract_socket(
                        quic::direct_endpoint_config(),
                        None,
                        PipeSocket::new(w, r),
                        Arc::new(TokioRuntime),
                    )?;
                    target = quic::pipe_peer();
                }
                h.dial(offer["addr"].as_str().unwrap_or_default()).await?;
                let within = Duration::from_secs(args.num("upgrade-timeout", 30));
                let d = h.wait_direct(within).await?;
                t["t_ctrl_direct"] = d["t_us"].clone();
                if let Some(v) = h.history.lock().unwrap().iter().find(|v| v["ev"] == "dialed") {
                    t["t_dialed"] = v["t_us"].clone();
                }
            }
        }
        ep.set_default_client_config(quic::client_config(fp)?);
        let conn = tokio::time::timeout(Duration::from_secs(30), ep.connect(target, "airhand-direct")?)
            .await
            .context("direct QUIC handshake timed out")??;
        t["t_quic"] = now_us().into();
        let mut control = conn.open_bi().await?;
        let mut video = conn.open_bi().await?;
        let mut audio = conn.open_bi().await?;
        control.0.write_all(format!("{}\n", json!({"token": token})).as_bytes()).await?;
        video.0.write_all(b"\n").await?;
        audio.0.write_all(b"\n").await?;
        let mut lanes = Lanes::new(control, video, audio.0);
        let (tc, commit) = lanes.expect("commit", Duration::from_secs(20)).await?;
        t["t_commit"] = tc.into();
        t["t_host_direct"] = commit["host_direct_t_us"].clone();
        s.direct = Some(lanes);
        s.conn = Some(conn);
        s.endpoint = Some(ep);
    }
    if with_relay || via == "relay" {
        let server = args.req("relay")?;
        let code = offer["relay_code"].as_u64().context("host has no relay code")? as u32;
        let cfg = LinkConfig::new(server.as_str(), "", "", "linux")?;
        let mut sig = connect_signal(&cfg, Role::Controller)
            .await
            .map_err(|p| anyhow!("relay: {p:?}"))?;
        write_msg(&mut sig.send, &ClientMsg::Connect { req_id: 1, code, resume: None }).await?;
        let mut buf = Vec::new();
        let grant = loop {
            match read_msg::<_, ServerMsg>(&mut sig.recv, MAX_SIGNAL_MSG, &mut buf).await? {
                Some(ServerMsg::ConnectResult { result: Ok(g), .. }) => break g,
                Some(ServerMsg::ConnectResult { result: Err(e), .. }) => bail!("relay connect: {e:?}"),
                Some(_) => {}
                None => bail!("relay closed"),
            }
        };
        let ss = open_session(&sig.conn, grant.session_id, Role::Controller).await?;
        s.relay = Some(Lanes::new(ss.control, ss.video, ss.audio.0));
        s._relay_conn = Some(sig.conn.clone());
        std::mem::forget(sig);
    }
    s.helper = helper;
    s.timings = t;
    Ok(s)
}

impl Session {
    async fn close(mut self) {
        if let Some(c) = self.conn.take() {
            c.close(0u32.into(), b"bye");
        }
        if let Some(e) = self.endpoint.take() {
            e.close(0u32.into(), b"");
        }
        if let Some(h) = self.helper.take() {
            h.close().await;
        }
    }
}

pub(crate) fn summary(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!({"n": 0});
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    json!({"n": v.len(), "min": v[0], "p50": p(0.5), "p90": p(0.9), "p99": p(0.99), "max": v[v.len()-1], "mean": mean})
}

pub async fn run(args: Args) -> Result<()> {
    let log = Log::open(args.get("log"))?;
    let via = args.get("via").cloned().unwrap_or("udp".into());
    let test = args.req("test")?.clone();
    let lane_name = args.get("lane").cloned().unwrap_or(if via == "relay" { "relay".into() } else { "direct".into() });
    let out = match test.as_str() {
        "upgrade" => upgrade(&args, &via, &log).await?,
        "consent" => crate::consent::ctrl_test(&args, &via, &log).await?,
        _ => {
            let with_relay = args.get("relay").is_some() && test == "fence";
            let mut s = setup(&args, &via, with_relay, &log).await?;
            let r = match test.as_str() {
                "rtt" => rtt(lane(&mut s, &lane_name)?, args.num("n", 1000), args.num("interval-ms", 10)).await?,
                "tput" => tput(&mut s, &lane_name, args.num("mbps", 60.0), args.num("secs", 10.0), &log).await?,
                "soak" => soak(&mut s, args.num("secs", 60.0), args.num("ping-ms", 1000), args.num("k-ms", 2000), &log).await?,
                "fence" => fence(&mut s, args.num("n", 200), args.num("interval-ms", 50)).await?,
                "hold" => hold(&mut s, args.num("secs", 30.0), args.num("mbps", 5.0), args.num("ping-ms", 50), &log).await?,
                t => bail!("unknown test {t}"),
            };
            let timings = s.timings.clone();
            s.close().await;
            json!({"setup": timings, "result": r})
        }
    };
    let out = json!({"test": test, "via": via, "lane": lane_name, "out": out});
    log.write(json!({"src": "ctrl", "summary": out.clone()}));
    println!("{out}");
    Ok(())
}

fn lane<'a>(s: &'a mut Session, name: &str) -> Result<&'a mut Lanes> {
    match name {
        "relay" => s.relay.as_mut().context("no relay lanes"),
        _ => s.direct.as_mut().context("no direct lanes"),
    }
}

async fn rtt(l: &mut Lanes, n: usize, interval_ms: u64) -> Result<Value> {
    let mut v = Vec::with_capacity(n);
    for seq in 0..n {
        let t0 = Instant::now();
        l.tx.send(&json!({"cmd": "ping", "seq": seq})).await?;
        loop {
            let (_, m) = l.expect("pong", Duration::from_secs(5)).await?;
            if m["pong"] == seq {
                break;
            }
        }
        v.push(t0.elapsed().as_secs_f64() * 1e3);
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
    Ok(json!({"rtt_ms": summary(v)}))
}

async fn tput(s: &mut Session, lane_name: &str, mbps: f64, secs: f64, log: &Log) -> Result<Value> {
    let helper_pid = s.helper.as_ref().map(|h| h.pid);
    let l = lane(s, lane_name)?;
    let before = l.request(json!({"cmd": "stats"}), "stats").await?;
    let my_helper0 = helper_pid.and_then(cpu_ticks);
    let me0 = cpu_ticks(std::process::id());
    let t0 = Instant::now();
    let base = l.video_bytes.load(Ordering::Relaxed);
    l.tx.send(&json!({"cmd": "send", "mbps": mbps, "secs": secs})).await?;
    let mut per_sec = Vec::new();
    let mut last = base;
    let mut done: Option<Value> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.tick().await;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = l.video_bytes.load(Ordering::Relaxed);
                let m = (now - last) as f64 * 8.0 / 1e6;
                per_sec.push(m);
                log.write(json!({"src": "ctrl", "tput_mbps": m}));
                last = now;
                if let Some(d) = &done {
                    let want = d["bytes"].as_u64().unwrap_or(0);
                    if now - base >= want || t0.elapsed().as_secs_f64() > secs + 10.0 {
                        break;
                    }
                }
            }
            m = l.rx.recv() => {
                let (_, m) = m.context("lane closed")?;
                if m.get("send_done").is_some() { done = Some(m); }
            }
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    let received = l.video_bytes.load(Ordering::Relaxed) - base;
    let after = l.request(json!({"cmd": "stats"}), "stats").await?;
    let my_helper1 = helper_pid.and_then(cpu_ticks);
    let me1 = cpu_ticks(std::process::id());
    let tck = clk_tck() as f64;
    let pct = |a: Option<u64>, b: Option<u64>, w: f64| match (a, b) {
        (Some(a), Some(b)) => Value::from((b - a) as f64 / tck / w * 100.0),
        _ => Value::Null,
    };
    let host_wall = (after["t_us"].as_i64().unwrap_or(0) - before["t_us"].as_i64().unwrap_or(0)) as f64 / 1e6;
    let q = |k: &str| after["quic"][k].as_u64().unwrap_or(0).saturating_sub(before["quic"][k].as_u64().unwrap_or(0));
    let sent_pk = q("sent_packets");
    let lost_pk = q("lost_packets");
    // Steady-state seconds: skip the first and the trailing partial second.
    let steady: Vec<f64> = per_sec.iter().skip(1).take((secs as usize).saturating_sub(1)).cloned().collect();
    Ok(json!({
        "target_mbps": mbps, "secs": secs, "wall_s": wall,
        "sent_bytes": done.as_ref().map(|d| d["bytes"].clone()),
        "skipped_bytes": done.as_ref().map(|d| d["skipped"].clone()),
        "received_bytes": received,
        "avg_mbps": received as f64 * 8.0 / 1e6 / secs,
        "per_sec_mbps": summary(steady),
        "quic_sender": {"sent_packets": sent_pk, "lost_packets": lost_pk,
            "loss_pct": if sent_pk > 0 { lost_pk as f64 / sent_pk as f64 * 100.0 } else { 0.0 },
            "congestion_events": q("congestion_events"),
            "rtt_us_end": after["quic"]["rtt_us"], "mtu": after["quic"]["current_mtu"],
            "udp_tx_datagrams": q("udp_tx_datagrams"), "udp_tx_bytes": q("udp_tx_bytes")},
        "cpu_pct_single_core": {
            "host_helper": match (before["helper_ticks"].as_u64(), after["helper_ticks"].as_u64()) {
                (Some(a), Some(b)) => Value::from((b - a) as f64 / tck / host_wall * 100.0), _ => Value::Null },
            "host_bench_quinn": match (before["self_ticks"].as_u64(), after["self_ticks"].as_u64()) {
                (Some(a), Some(b)) => Value::from((b - a) as f64 / tck / host_wall * 100.0), _ => Value::Null },
            "ctrl_helper": pct(my_helper0, my_helper1, wall),
            "ctrl_bench_quinn": pct(me0, me1, wall),
        },
        "host_helper_counters": after["helper"].clone(),
    }))
}

fn path_flaps(paths: &[Value]) -> (usize, usize) {
    // (transitions away from direct after the first direct, total path events)
    let mut seen_direct = false;
    let mut away = 0;
    for p in paths {
        if p["path"] == "direct" {
            seen_direct = true;
        } else if seen_direct {
            away += 1;
        }
    }
    (away, paths.len())
}

async fn soak(s: &mut Session, secs: f64, ping_ms: u64, k_ms: u64, log: &Log) -> Result<Value> {
    let local_paths = |s: &Session| -> Vec<Value> {
        s.helper
            .as_ref()
            .map(|h| h.history.lock().unwrap().iter().filter(|v| v["ev"] == "path").cloned().collect())
            .unwrap_or_default()
    };
    let l = s.direct.as_mut().context("no direct lanes")?;
    l.tx.send(&json!({"cmd": "watch", "k_ms": k_ms})).await?;
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut last_pong = Instant::now();
    let mut k_misses = 0u64;
    let mut tripped = false;
    let mut rtts = Vec::new();
    let mut sent_at = std::collections::HashMap::new();
    let mut seq = 0u64;
    let mut tick = tokio::time::interval(Duration::from_millis(ping_ms));
    let mut check = tokio::time::interval(Duration::from_millis(50));
    while Instant::now() < end {
        tokio::select! {
            _ = tick.tick() => {
                sent_at.insert(seq, Instant::now());
                l.tx.send(&json!({"cmd": "ping", "seq": seq})).await?;
                seq += 1;
            }
            _ = check.tick() => {
                let gap = last_pong.elapsed();
                if gap > Duration::from_millis(k_ms) && !tripped {
                    tripped = true;
                    k_misses += 1;
                    log.write(json!({"src": "ctrl", "k_miss": true, "gap_ms": gap.as_millis() as u64}));
                }
            }
            m = l.rx.recv() => {
                let (_, m) = m.context("lane closed")?;
                if let Some(q) = m["pong"].as_u64() {
                    last_pong = Instant::now();
                    tripped = false;
                    if let Some(t) = sent_at.remove(&q) { rtts.push(t.elapsed().as_secs_f64() * 1e3); }
                }
            }
        }
    }
    let host = l.request(json!({"cmd": "stats"}), "stats").await?;
    let host_paths: Vec<Value> = host["helper_paths"].as_array().cloned().unwrap_or_default();
    let mine = local_paths(s);
    let (ha, hn) = path_flaps(&host_paths);
    let (ca, cn) = path_flaps(&mine);
    Ok(json!({
        "secs": secs, "ping_ms": ping_ms, "k_ms": k_ms, "pings": seq,
        "ctrl_k_misses": k_misses, "host_k_misses": host["k_misses"],
        "ctrl_gate_left_direct": ca, "ctrl_path_events": cn,
        "host_gate_left_direct": ha, "host_path_events": hn,
        "ping_rtt_ms": summary(rtts),
        "quic": host["quic"].clone(),
    }))
}

async fn fence(s: &mut Session, n: usize, interval_ms: u64) -> Result<Value> {
    let mut gaps = Vec::new();
    let mut relay_lat = Vec::new();
    let mut direct_lat = Vec::new();
    for id in 0..n {
        s.direct.as_mut().unwrap().tx.send(&json!({"cmd": "fence", "id": id})).await?;
        let (td, md) = s.direct.as_mut().unwrap().expect("fence", Duration::from_secs(5)).await?;
        let (tr, mr) = s.relay.as_mut().context("no relay lanes")?.expect("fence", Duration::from_secs(5)).await?;
        if md["fence"] != id || mr["fence"] != id {
            continue;
        }
        let sent = md["t_us"].as_i64().unwrap_or(0);
        gaps.push((tr - td) as f64 / 1e3);
        relay_lat.push((tr - sent) as f64 / 1e3);
        direct_lat.push((td - sent) as f64 / 1e3);
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
    Ok(json!({
        "relay_minus_direct_arrival_ms": summary(gaps),
        "relay_one_way_ms": summary(relay_lat),
        "direct_one_way_ms": summary(direct_lat),
    }))
}

/// Keeps a paced stream and a ping loop running; the harness changes the
/// network underneath and correlates the logged events.
async fn hold(s: &mut Session, secs: f64, mbps: f64, ping_ms: u64, log: &Log) -> Result<Value> {
    let l = s.direct.as_mut().context("no direct lanes")?;
    if mbps > 0.0 {
        l.tx.send(&json!({"cmd": "send", "mbps": mbps, "secs": secs})).await?;
    }
    let end = Instant::now() + Duration::from_secs_f64(secs);
    let mut seq = 0u64;
    let mut tick = tokio::time::interval(Duration::from_millis(ping_ms));
    let mut sample = tokio::time::interval(Duration::from_millis(100));
    let mut last = l.video_bytes.load(Ordering::Relaxed);
    while Instant::now() < end {
        tokio::select! {
            _ = tick.tick() => {
                let _ = tokio::time::timeout(Duration::from_millis(200), l.tx.send(&json!({"cmd": "ping", "seq": seq}))).await;
                seq += 1;
            }
            _ = sample.tick() => {
                let now = l.video_bytes.load(Ordering::Relaxed);
                log.write(json!({"src": "ctrl", "rx_bytes_100ms": now - last}));
                last = now;
            }
            m = l.rx.recv() => {
                let Some((t, m)) = m else { break };
                if m.get("pong").is_some() {
                    log.write(json!({"src": "ctrl", "pong": m["pong"], "t_rx_us": t}));
                }
            }
        }
    }
    let host = tokio::time::timeout(Duration::from_secs(15), l.request(json!({"cmd": "stats"}), "stats"))
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or(Value::Null);
    let mine: Vec<Value> = s
        .helper
        .as_ref()
        .map(|h| h.history.lock().unwrap().iter().filter(|v| v["ev"] == "path").cloned().collect())
        .unwrap_or_default();
    let ctrl_stats = match &s.helper {
        Some(h) => h.stats().await.unwrap_or(Value::Null),
        None => Value::Null,
    };
    Ok(json!({"host_paths": host["helper_paths"], "ctrl_paths": mine,
        "host_helper": host["helper"], "ctrl_helper": ctrl_stats, "quic": host["quic"]}))
}

async fn upgrade(args: &Args, via: &str, log: &Log) -> Result<Value> {
    let n = args.num("n", 20);
    let mut runs = Vec::new();
    let mut total = Vec::new();
    let mut to_direct = Vec::new();
    let mut host_ctrl_skew = Vec::new();
    let mut failures = 0;
    for i in 0..n {
        match tokio::time::timeout(Duration::from_secs(60), setup(args, via, false, log)).await {
            Ok(Ok(s)) => {
                let t = s.timings.clone();
                let g = |k: &str| t[k].as_i64();
                if let (Some(a), Some(b)) = (g("t_req"), g("t_commit")) {
                    total.push((b - a) as f64 / 1e3);
                }
                if let (Some(a), Some(b)) = (g("t_req"), g("t_ctrl_direct")) {
                    to_direct.push((b - a) as f64 / 1e3);
                }
                if let (Some(a), Some(b)) = (g("t_ctrl_direct"), g("t_host_direct")) {
                    host_ctrl_skew.push((b - a) as f64 / 1e3);
                }
                log.write(json!({"src": "ctrl", "upgrade_run": i, "timings": t.clone()}));
                runs.push(t);
                s.close().await;
            }
            Ok(Err(e)) => {
                failures += 1;
                log.write(json!({"src": "ctrl", "upgrade_run": i, "error": format!("{e:#}")}));
            }
            Err(_) => {
                failures += 1;
                log.write(json!({"src": "ctrl", "upgrade_run": i, "error": "timeout 60s"}));
            }
        }
        tokio::time::sleep(Duration::from_millis(args.num("gap-ms", 1000))).await;
    }
    Ok(json!({
        "n": n, "failures": failures,
        "request_to_commit_ms": summary(total),
        "request_to_ctrl_direct_ms": summary(to_direct),
        "host_direct_minus_ctrl_direct_ms": summary(host_ctrl_skew),
    }))
}
