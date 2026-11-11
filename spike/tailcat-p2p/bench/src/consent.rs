//! r4 被控端确认 (host consent) on the relay session control stream.
//!
//! Order under test (spec r4 "握手"): controller `hello` → host
//! `awaiting_approval` → (simulated user) → `host_hello { granted }` or
//! `bye { Declined | Unanswered }`. Only after a grant that includes
//! `direct` does the controller spawn its helper and send
//! `direct_request`; only then does the host spawn its helper. Messages are
//! JSON lines standing in for the postcard variants of the same names.
//!
//! `sim.prewarm = true` deliberately breaks I16 (helpers started before
//! consent) and exists only to measure what starting them afterwards costs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use airhand_link::LinkConfig;
use airhand_link::net::connect_signal;
use airhand_link::transport::open_session;
use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{ClientMsg, ServerMsg};
use airhand_proto::{MAX_SIGNAL_MSG, Role};
use anyhow::{Context, Result, anyhow, bail};
use quinn::{Connection, Endpoint, SendStream, TokioRuntime};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc};

use crate::ctrl::{Lanes, summary};
use crate::helper::{Chan, Helper, SPAWNS};
use crate::host::{SessionCtx, TUNNEL_PORT};
use crate::lane::{Tx, reader};
use crate::pipesock::PipeSocket;
use crate::quic::{self, hex};
use crate::{Args, Log, now_us};

const MEDIA_RELAY: u8 = 0;
const MEDIA_DIRECT: u8 = 1;
const MEDIA_STOP: u8 = 2;

fn spawns() -> u64 {
    SPAWNS.load(Ordering::Relaxed)
}

/// Paced stand-in for the video lane; follows `mode`, resets both streams on stop.
fn media_task(
    mut relay: SendStream,
    mbps: f64,
    mode: Arc<AtomicU8>,
    mut direct_rx: mpsc::Receiver<SendStream>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut direct: Option<SendStream> = None;
        let per_tick = ((mbps * 1e6 / 8.0) / 100.0) as usize;
        let chunk = bytes::Bytes::from(vec![0x5au8; per_tick.max(1)]);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let w = Duration::from_millis(50);
        loop {
            tick.tick().await;
            if let Ok(s) = direct_rx.try_recv() {
                direct = Some(s);
            }
            match mode.load(Ordering::Relaxed) {
                MEDIA_RELAY => {
                    let _ = tokio::time::timeout(w, relay.write_chunk(chunk.clone())).await;
                }
                MEDIA_DIRECT => {
                    if let Some(d) = direct.as_mut() {
                        let _ = tokio::time::timeout(w, d.write_chunk(chunk.clone())).await;
                    }
                }
                _ => break,
            }
        }
        let _ = relay.reset(0u32.into());
        if let Some(mut d) = direct {
            let _ = d.reset(0u32.into());
        }
    })
}

struct Direct {
    helper: Option<Helper>,
    endpoint: Endpoint,
    conn: Option<Connection>,
}

pub(crate) async fn host_session(
    tx: Tx,
    mut rx: mpsc::Receiver<(i64, Value)>,
    video: SendStream,
    hello: Value,
    ctx: SessionCtx,
) {
    let log = ctx.log.clone();
    let nonce = hello["nonce"].clone();
    let sim = hello["sim"].clone();
    let decision = sim["decision"].as_str().unwrap_or("accept").to_string();
    let delay = Duration::from_millis(sim["delay_ms"].as_u64().unwrap_or(0));
    let timeout = Duration::from_millis(sim["timeout_ms"].as_u64().unwrap_or(60_000));
    let grant_direct = sim["direct"].as_bool().unwrap_or(false) && hello["direct"].as_bool().unwrap_or(false);
    let prewarm = sim["prewarm"].as_bool().unwrap_or(false);
    let mbps = sim["mbps"].as_f64().unwrap_or(5.0);
    let chan = if hello["via"] == "pipe" { Chan::Pipe } else { Chan::Udp };
    // approve = "never": unattended host, no prompt; direct follows the host's [direct] config.
    let never = sim["approve"] == "never";
    let decision = if never { "accept".to_string() } else { decision };
    let delay = if never { Duration::ZERO } else { delay };
    let spawns0 = spawns();

    if !never {
        let _ = tx.send(&json!({"awaiting_approval": true, "t_us": now_us()})).await;
    }
    let mut pre = None;
    if prewarm {
        pre = Helper::spawn("host", chan, log.clone()).await.ok();
    }
    let mut dropped: BTreeMap<String, u64> = BTreeMap::new();
    let mut latest_settings = Value::Null;
    let start = Instant::now();
    let decide_at = tokio::time::Instant::from_std(start + delay);
    let deadline = tokio::time::Instant::from_std(start + timeout);
    let decided = loop {
        let wait_decide = async {
            if decision == "none" {
                std::future::pending::<()>().await
            } else {
                tokio::time::sleep_until(decide_at).await
            }
        };
        tokio::select! {
            _ = wait_decide => break true,
            _ = tokio::time::sleep_until(deadline) => break false,
            m = rx.recv() => {
                let Some((_, m)) = m else {
                    log.write(json!({"src": "host", "consent_end": "controller_left_while_awaiting", "nonce": nonce}));
                    return;
                };
                if m["cmd"] == "ping" {
                    let _ = tx.send(&json!({"pong": m["seq"], "t_us": now_us()})).await;
                } else if m.get("bye").is_some() {
                    log.write(json!({"src": "host", "consent_end": "controller_bye_while_awaiting", "nonce": nonce}));
                    return;
                } else if m.get("settings").is_some() {
                    latest_settings = m["settings"].clone();
                } else {
                    let k = m.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
                    *dropped.entry(k).or_default() += 1;
                }
            }
        }
    };
    let pre_counts = json!({"dropped": dropped, "helper_spawns": spawns() - spawns0, "relay_media_bytes": 0});
    if !decided || decision == "decline" {
        let reason = if decided { "Declined" } else { "Unanswered" };
        let _ = tx
            .send(&json!({"bye": reason, "t_us": now_us(), "waited_ms": start.elapsed().as_millis() as u64,
                "pre_approval": pre_counts}))
            .await;
        if let Some(h) = pre {
            h.close().await;
        }
        log.write(json!({"src": "host", "consent_end": reason, "nonce": nonce, "helper_spawns": spawns() - spawns0}));
        tokio::time::sleep(Duration::from_millis(500)).await;
        return;
    }

    let t_approved = now_us();
    let granted = json!({"input": true, "clipboard": false, "audio": false, "direct": grant_direct});
    let _ = tx
        .send(&json!({"host_hello": true, "granted": granted, "applied_settings": latest_settings,
            "pre_approval": pre_counts, "t_approved_us": t_approved, "approve": if never { "never" } else { "ask" }}))
        .await;
    let mode = Arc::new(AtomicU8::new(MEDIA_RELAY));
    let (vtx, vrx) = mpsc::channel(1);
    let media = media_task(video, mbps, mode.clone(), vrx);
    let direct: Arc<Mutex<Option<Direct>>> = Arc::new(Mutex::new(None));
    let mut declines = 0u64;
    let mut t_end = now_us();
    let end_reason = loop {
        let Some((t_rx, m)) = rx.recv().await else { break "relay_closed" };
        if m["cmd"] == "ping" {
            let _ = tx.send(&json!({"pong": m["seq"], "t_us": now_us()})).await;
            continue;
        }
        if m.get("bye").is_some() {
            t_end = t_rx;
            break "controller_bye";
        }
        if m.get("sim_kick").is_some() {
            // 断开此控制端（保留控制码）.
            t_end = now_us();
            let _ = tx.send(&json!({"bye": "Kicked", "t_stop_us": t_end})).await;
            break "kick";
        }
        if m.get("sim_stop").is_some() {
            // Stands in for the host user pressing 停止共享.
            t_end = now_us();
            let _ = tx.send(&json!({"bye": "CodeChanged", "t_stop_us": t_end})).await;
            break "stop_sharing";
        }
        let Some(req) = m.get("direct_request") else { continue };
        let epoch = req["epoch"].clone();
        let key = req["client_key"].as_str().unwrap_or_default().to_string();
        if !grant_direct || key.is_empty() {
            declines += 1;
            let _ = tx.send(&json!({"direct_decline": epoch, "reason": "not_granted"})).await;
            continue;
        }
        if direct.lock().await.is_some() {
            continue;
        }
        let t_req = t_rx;
        match start_direct(chan, &key, pre.take(), &ctx).await {
            Ok((offer, d, token, mut host_t)) => {
                host_t["req_rx_to_offer_ms"] = ((now_us() - t_req) as f64 / 1e3).into();
                let ep = d.endpoint.clone();
                *direct.lock().await = Some(d);
                let _ = tx.send(&json!({"direct_offer": offer, "epoch": epoch, "host_t": host_t})).await;
                tokio::spawn(accept_direct(
                    ep,
                    token,
                    epoch,
                    tx.clone(),
                    vtx.clone(),
                    mode.clone(),
                    direct.clone(),
                    log.clone(),
                ));
            }
            Err(e) => {
                let _ = tx.send(&json!({"direct_decline": epoch, "reason": format!("{e:#}")})).await;
            }
        }
    };

    // Both paths end together (I18 / I19).
    mode.store(MEDIA_STOP, Ordering::Relaxed);
    let _ = media.await;
    let t_media_stopped = now_us();
    let mut rec = json!({"src": "host", "consent_end": end_reason, "nonce": nonce, "t_end_us": t_end,
        "t_media_stopped_us": t_media_stopped, "declines": declines});
    if let Some(d) = direct.lock().await.take() {
        if let Some(c) = &d.conn {
            c.close(0u32.into(), b"bye");
        }
        d.endpoint.close(0u32.into(), b"");
        rec["t_direct_closed_us"] = now_us().into();
        if let Some(h) = d.helper {
            rec["helper_exited"] = h.close().await.into();
            rec["t_helper_exited_us"] = now_us().into();
        }
    }
    if let Some(h) = pre {
        h.close().await;
    }
    rec["helper_spawns"] = (spawns() - spawns0).into();
    log.write(rec);
}

async fn start_direct(
    chan: Chan,
    client_key: &str,
    pre: Option<Helper>,
    ctx: &SessionCtx,
) -> Result<(Value, Direct, [u8; 16], Value)> {
    let t0 = now_us();
    let mut h = match pre {
        Some(h) => h,
        None => Helper::spawn("host", chan, ctx.log.clone()).await?,
    };
    let t_spawned = now_us();
    let cert = quic::host_cert()?;
    let token: [u8; 16] = rand::random();
    let server_cfg = quic::server_config(&cert)?;
    let endpoint = if chan == Chan::Udp {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
        h.set_peer(sock.local_addr()?.port()).await?;
        Endpoint::new(quic::direct_endpoint_config(), Some(server_cfg), sock, Arc::new(TokioRuntime))?
    } else {
        let (w, r) = h.pipes.take().context("no pipes")?;
        Endpoint::new_with_abstract_socket(
            quic::direct_endpoint_config(),
            Some(server_cfg),
            PipeSocket::new(w, r),
            Arc::new(TokioRuntime),
        )?
    };
    let addr = h.serve(client_key, TUNNEL_PORT, &ctx.region, ctx.poll_ms).await?;
    let t_served = now_us();
    let offer = json!({"addr": addr, "fp": hex(&cert.fp), "token": hex(&token)});
    let host_t = json!({"spawn_ms": (t_spawned - t0) as f64 / 1e3, "serve_ms": (t_served - t_spawned) as f64 / 1e3});
    Ok((offer, Direct { helper: Some(h), endpoint, conn: None }, token, host_t))
}

#[allow(clippy::too_many_arguments)]
async fn accept_direct(
    ep: Endpoint,
    token: [u8; 16],
    epoch: Value,
    relay: Tx,
    vtx: mpsc::Sender<SendStream>,
    mode: Arc<AtomicU8>,
    direct: Arc<Mutex<Option<Direct>>>,
    log: Log,
) {
    let r: Result<()> = async {
        let inc = tokio::time::timeout(Duration::from_secs(30), ep.accept())
            .await
            .context("no direct QUIC connection")?
            .context("endpoint closed")?;
        let conn = inc.await?;
        let (ctl_tx, ctl_rx) = conn.accept_bi().await?;
        let (video_tx, _video_rx) = conn.accept_bi().await?;
        let (_audio_tx, _audio_rx) = conn.accept_bi().await?;
        let mut drx = reader(ctl_rx);
        let dtx = Tx::new(ctl_tx);
        let (_, first) = drx.recv().await.context("no token")?;
        if first["token"].as_str() != Some(&hex(&token)) {
            bail!("bad token");
        }
        match direct.lock().await.as_mut() {
            Some(d) => d.conn = Some(conn.clone()),
            None => bail!("session ended before commit"),
        }
        relay.send(&json!({"direct_switch": epoch, "t_us": now_us()})).await?;
        dtx.send(&json!({"commit": epoch, "t_us": now_us()})).await?;
        let _ = vtx.send(video_tx).await;
        mode.store(MEDIA_DIRECT, Ordering::Relaxed);
        while let Some((_, m)) = drx.recv().await {
            if m["cmd"] == "ping" {
                let _ = dtx.send(&json!({"pong": m["seq"], "t_us": now_us()})).await;
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = r {
        log.write(json!({"src": "host", "direct_accept_error": format!("{e:#}")}));
    }
}

#[derive(Default, Clone, Copy)]
struct Derp {
    accepts: i64,
    conns: i64,
}

fn derp_now(path: Option<&String>) -> Option<Derp> {
    let s = std::fs::read_to_string(path?).ok()?;
    let v: Value = serde_json::from_str(s.lines().rev().find(|l| l.contains("accepts"))?).ok()?;
    Some(Derp { accepts: v["accepts"].as_i64()?, conns: v["conns"].as_i64()? })
}

fn ms(a: i64, b: i64) -> Value {
    ((b - a) as f64 / 1e3).into()
}

pub(crate) async fn ctrl_test(args: &Args, via: &str, log: &Log) -> Result<Value> {
    let n = args.num("n", 1usize);
    let mut runs = Vec::new();
    for i in 0..n {
        let limit = Duration::from_secs(args.num("run-timeout", 120));
        let v = match tokio::time::timeout(limit, ctrl_run(args, via, log)).await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => json!({"error": format!("{e:#}")}),
            Err(_) => json!({"error": "run timeout"}),
        };
        log.write(json!({"src": "ctrl", "consent_run": i, "r": v.clone()}));
        runs.push(v);
        tokio::time::sleep(Duration::from_millis(args.num("gap-ms", 1000))).await;
    }
    let failures = runs.iter().filter(|r| r.get("error").is_some()).count();
    let mut keys: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for r in &runs {
        if let Some(o) = r["ms"].as_object() {
            for (k, v) in o {
                if let Some(x) = v.as_f64() {
                    keys.entry(k.clone()).or_default().push(x);
                }
            }
        }
    }
    let ms: serde_json::Map<String, Value> = keys.into_iter().map(|(k, v)| (k, summary(v))).collect();
    let mut out = json!({"n": n, "failures": failures, "ms": ms});
    if n <= 6 {
        out["runs"] = runs.into();
    } else {
        out["first_run"] = runs.first().cloned().unwrap_or(Value::Null);
        out["checks_all_runs"] = runs.iter().map(|r| r["checks"].clone()).collect::<Vec<_>>().into();
    }
    Ok(out)
}

async fn relay_lanes(args: &Args, code: u32) -> Result<(Lanes, Connection)> {
    let server = args.req("relay")?;
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
    let conn = sig.conn.clone();
    std::mem::forget(sig);
    Ok((Lanes::new(ss.control, ss.video, ss.audio.0), conn))
}

async fn ctrl_run(args: &Args, via: &str, log: &Log) -> Result<Value> {
    let decision = args.get("decision").cloned().unwrap_or("accept".into());
    let delay_ms = args.num("delay-ms", 3000u64);
    let grant = args.num("grant-direct", 1u8) == 1;
    let prewarm = args.num("prewarm", 0u8) == 1;
    let timeout_ms = args.num("approval-timeout-ms", 60_000u64);
    let end = args.get("end").cloned().unwrap_or("stop".into());
    let approve = args.get("approve").cloned().unwrap_or("ask".into());
    let mbps = args.num("mbps", 5.0f64);
    let (ping_ms, misses) = (args.num("ping-ms", 250u64), args.num("misses", 2u32));
    let derp_file = args.get("derp-counters");
    let chan = if via == "pipe" { Chan::Pipe } else { Chan::Udp };
    let spawns0 = spawns();

    // Only the relay code comes over the bench socket.
    let mut sig = UnixStream::connect(args.req("sig")?).await?;
    sig.write_all(format!("{}\n", json!({"via": "relay"})).as_bytes()).await?;
    let mut line = String::new();
    BufReader::new(&mut sig).read_line(&mut line).await?;
    let code = serde_json::from_str::<Value>(&line)?["relay_code"].as_u64().context("no relay code")? as u32;

    let derp0 = derp_now(derp_file);
    let mut helper = None;
    if prewarm {
        let mut h = Helper::spawn("ctrl", chan, log.clone()).await?;
        let key = h.init(TUNNEL_PORT, ping_ms, misses).await?;
        helper = Some((h, key));
    }
    let (mut relay, _relay_conn) = relay_lanes(args, code).await?;
    let nonce: u64 = rand::random::<u64>() >> 12;
    let t_hello = now_us();
    relay
        .tx
        .send(&json!({"hello": true, "direct": true, "via": via, "nonce": nonce,
            "sim": {"approve": approve, "decision": decision, "delay_ms": delay_ms, "direct": grant, "prewarm": prewarm,
                    "timeout_ms": timeout_ms, "mbps": mbps}}))
        .await?;
    // Sent before approval: the host must defer settings and drop the rest (I15).
    let probes = if approve == "never" { vec![] } else { vec![
        json!({"settings": {"bitrate_kbps": 1000}}),
        json!({"settings": {"bitrate_kbps": 2000}}),
        json!({"input": {"key": 30}}),
        json!({"clipboard": "probe"}),
        json!({"direct_request": {"epoch": 0, "client_key": "probe"}}),
        json!({"cmd": "ping", "seq": 0}),
    ] };
    for m in probes {
        relay.tx.send(&m).await?;
    }
    let (mut awaiting, mut pong, mut t_awaiting) = (false, false, 0);
    let wait = Duration::from_millis(timeout_ms + 15_000);
    let (t_hh, hh) = loop {
        let (t, m) = tokio::time::timeout(wait, relay.rx.recv())
            .await
            .context("no host decision")?
            .context("relay closed")?;
        if m.get("awaiting_approval").is_some() {
            awaiting = true;
            t_awaiting = t;
        } else if m.get("pong").is_some() {
            pong = true;
        } else if m.get("host_hello").is_some() || m.get("bye").is_some() {
            break (t, m);
        }
    };
    let derp_hh = derp_now(derp_file);
    let derp_delta = |a: Option<Derp>, b: Option<Derp>| match (a, b) {
        (Some(a), Some(b)) => Value::from(b.accepts - a.accepts),
        _ => Value::Null,
    };
    let mut out = json!({
        "approve": approve, "decision": decision, "prewarm": prewarm, "grant_direct": grant, "end": end,
        "checks": {
            "awaiting_approval_first": awaiting,
            "pong_while_awaiting": pong,
            "relay_media_bytes_before_approval": relay.video_bytes.load(Ordering::Relaxed),
            "ctrl_helper_spawns_before_approval": spawns() - spawns0,
            "host_pre_approval": hh["pre_approval"].clone(),
            "derp_accepts_delta_before_approval": derp_delta(derp0, derp_hh),
            "derp_conns_at_approval": derp_hh.map(|d| d.conns),
        },
        "ms": {},
    });
    if awaiting {
        out["ms"]["hello_to_awaiting_rx"] = ms(t_hello, t_awaiting);
    }
    out["ms"]["hello_to_host_hello_rx"] = ms(t_hello, t_hh);
    if let Some(reason) = hh.get("bye") {
        out["bye"] = reason.clone();
        out["ms"]["hello_to_bye_rx"] = ms(t_hello, t_hh);
        tokio::time::sleep(Duration::from_secs(1)).await;
        out["checks"]["ctrl_helper_spawns_total"] = (spawns() - spawns0).into();
        out["checks"]["derp_accepts_delta_total"] = derp_delta(derp0, derp_now(derp_file));
        if let Some((h, _)) = helper {
            h.close().await;
        }
        return Ok(out);
    }
    out["granted"] = hh["granted"].clone();
    out["applied_settings"] = hh["applied_settings"].clone();
    let t_approved = hh["t_approved_us"].as_i64().unwrap_or(t_hh);
    out["ms"]["approval_to_host_hello_rx"] = ms(t_approved, t_hh);

    let mut direct_state: Option<(Helper, Endpoint, Connection, Lanes)> = None;
    if hh["granted"]["direct"] != true {
        // A non-compliant controller asks anyway; the host must decline without spawning.
        relay.tx.send(&json!({"direct_request": {"epoch": 1, "client_key": "probe"}})).await?;
        let (_, d) = relay.expect("direct_decline", Duration::from_secs(10)).await?;
        out["checks"]["direct_declined"] = d["reason"].clone();
    } else {
        let t_s0 = now_us();
        let (mut h, key) = match helper.take() {
            Some(x) => x,
            None => {
                let mut h = Helper::spawn("ctrl", chan, log.clone()).await?;
                out["ms"]["ctrl_spawn"] = ms(t_s0, now_us());
                let k = h.init(TUNNEL_PORT, ping_ms, misses).await?;
                (h, k)
            }
        };
        let t_key = now_us();
        out["ms"]["ctrl_helper_ready"] = ms(t_s0, t_key);
        relay.tx.send(&json!({"direct_request": {"epoch": 1, "client_key": key, "via": via}})).await?;
        let t_req = now_us();
        let (t_offer, offer) = relay.expect("direct_offer", Duration::from_secs(30)).await?;
        out["ms"]["req_to_offer_rx"] = ms(t_req, t_offer);
        for k in ["spawn_ms", "serve_ms", "req_rx_to_offer_ms"] {
            out["ms"][format!("host_{k}").trim_end_matches("_ms")] = offer["host_t"][k].clone();
        }
        let o = &offer["direct_offer"];
        let fp = quic::unhex32(o["fp"].as_str().unwrap_or_default())?;
        let token = o["token"].as_str().unwrap_or_default().to_string();
        let (mut ep, target) = if chan == Chan::Udp {
            let sock = std::net::UdpSocket::bind("127.0.0.1:0")?;
            h.set_peer(sock.local_addr()?.port()).await?;
            let ep = Endpoint::new(quic::direct_endpoint_config(), None, sock, Arc::new(TokioRuntime))?;
            (ep, format!("127.0.0.1:{}", h.chan_port.context("no chan port")?).parse()?)
        } else {
            let (w, r) = h.pipes.take().context("no pipes")?;
            let ep = Endpoint::new_with_abstract_socket(
                quic::direct_endpoint_config(),
                None,
                PipeSocket::new(w, r),
                Arc::new(TokioRuntime),
            )?;
            (ep, quic::pipe_peer())
        };
        h.dial(o["addr"].as_str().unwrap_or_default()).await?;
        let d = h.wait_direct(Duration::from_secs(args.num("upgrade-timeout", 30))).await?;
        let t_cd = d["t_us"].as_i64().unwrap_or(0);
        out["ms"]["offer_rx_to_ctrl_direct"] = ms(t_offer, t_cd);
        ep.set_default_client_config(quic::client_config(fp)?);
        let conn = tokio::time::timeout(Duration::from_secs(30), ep.connect(target, "airhand-direct")?)
            .await
            .context("direct QUIC handshake timed out")??;
        let t_quic = now_us();
        out["ms"]["ctrl_direct_to_quic"] = ms(t_cd, t_quic);
        let mut control = conn.open_bi().await?;
        let mut video = conn.open_bi().await?;
        let mut audio = conn.open_bi().await?;
        control.0.write_all(format!("{}\n", json!({"token": token})).as_bytes()).await?;
        video.0.write_all(b"\n").await?;
        audio.0.write_all(b"\n").await?;
        let mut lanes = Lanes::new(control, video, audio.0);
        let (t_commit, _) = lanes.expect("commit", Duration::from_secs(20)).await?;
        let (t_switch, _) = relay.expect("direct_switch", Duration::from_secs(5)).await?;
        out["ms"]["quic_to_commit"] = ms(t_quic, t_commit);
        out["ms"]["approval_to_commit"] = ms(t_approved, t_commit);
        out["ms"]["host_hello_rx_to_commit"] = ms(t_hh, t_commit);
        out["ms"]["req_to_commit"] = ms(t_req, t_commit);
        out["checks"]["switch_before_commit_us"] = (t_commit - t_switch).into();
        let (r0, d0) = (relay.video_bytes.load(Ordering::Relaxed), lanes.video_bytes.load(Ordering::Relaxed));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let r1 = relay.video_bytes.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(1200)).await;
        out["checks"]["direct_media_bytes_1500ms"] = (lanes.video_bytes.load(Ordering::Relaxed) - d0).into();
        out["checks"]["relay_media_bytes_300ms_to_1500ms_after_commit"] =
            (relay.video_bytes.load(Ordering::Relaxed) - r1).into();
        let _ = r0;
        direct_state = Some((h, ep, conn, lanes));
    }

    match end.as_str() {
        "stop" | "kick" => {
            let cmd = if end == "stop" { "sim_stop" } else { "sim_kick" };
            relay.tx.send(&json!({ cmd: true })).await?;
            let (t_bye, bye) = relay.expect("bye", Duration::from_secs(5)).await?;
            let t_stop = bye["t_stop_us"].as_i64().unwrap_or(t_bye);
            out["bye"] = bye["bye"].clone();
            out["ms"]["stop_to_bye_rx"] = ms(t_stop, t_bye);
            let closed = async {
                if let Some((_, _, c, _)) = &direct_state {
                    c.closed().await;
                }
                now_us()
            };
            if let Ok(t) = tokio::time::timeout(Duration::from_secs(5), closed).await {
                if direct_state.is_some() {
                    out["ms"]["stop_to_direct_conn_closed"] = ms(t_stop, t);
                }
            }
            let t_wait = Instant::now();
            while relay.video_end_us.load(Ordering::Relaxed) == 0 && t_wait.elapsed() < Duration::from_secs(5) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let rl = relay.video_last_us.load(Ordering::Relaxed);
            let re = relay.video_end_us.load(Ordering::Relaxed);
            out["ms"]["stop_to_last_relay_media"] = ms(t_stop, rl);
            if re != 0 {
                out["ms"]["stop_to_relay_media_stream_end"] = ms(t_stop, re);
            }
            if let Some((_, _, _, l)) = &direct_state {
                out["ms"]["stop_to_last_direct_media"] = ms(t_stop, l.video_last_us.load(Ordering::Relaxed));
            }
            if let Some((h, ep, c, _)) = direct_state.take() {
                c.close(0u32.into(), b"");
                ep.close(0u32.into(), b"");
                let t0 = now_us();
                out["checks"]["ctrl_helper_exited"] = h.close().await.into();
                out["ms"]["ctrl_bye_rx_to_helper_exit"] = ms(t_bye.max(t0), now_us());
            }
        }
        _ => {
            let t_b = now_us();
            relay.tx.send(&json!({"bye": true})).await?;
            if let Some((h, ep, c, _)) = direct_state.take() {
                c.close(0u32.into(), b"bye");
                ep.close(0u32.into(), b"");
                out["checks"]["ctrl_helper_exited"] = h.close().await.into();
                out["ms"]["ctrl_bye_to_own_helper_exit"] = ms(t_b, now_us());
            }
            out["t_ctrl_bye_us"] = t_b.into();
        }
    }
    if let Some((h, _)) = helper {
        h.close().await;
    }
    out["checks"]["ctrl_helper_spawns_total"] = (spawns() - spawns0).into();

    if let Some(p) = args.get("host-log") {
        let t_wait = Instant::now();
        while t_wait.elapsed() < Duration::from_secs(6) {
            let s = std::fs::read_to_string(p).unwrap_or_default();
            let hit = s
                .lines()
                .filter_map(|l| serde_json::from_str::<Value>(l).ok())
                .find(|v| v.get("consent_end").is_some() && v["nonce"] == nonce);
            if let Some(v) = hit {
                let te = v["t_end_us"].as_i64().unwrap_or(0);
                for k in ["t_media_stopped_us", "t_direct_closed_us", "t_helper_exited_us"] {
                    if let Some(t) = v[k].as_i64() {
                        out["ms"][format!("host_end_to_{}", k.trim_start_matches("t_").trim_end_matches("_us"))] =
                            ms(te, t);
                    }
                }
                if end != "stop" {
                    if let Some(t) = v["t_helper_exited_us"].as_i64() {
                        out["ms"]["ctrl_bye_to_host_helper_exit"] = ms(out["t_ctrl_bye_us"].as_i64().unwrap_or(te), t);
                    }
                }
                out["host_end"] = v;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    out["checks"]["derp_accepts_delta_total"] = derp_delta(derp0, derp_now(derp_file));
    Ok(out)
}
