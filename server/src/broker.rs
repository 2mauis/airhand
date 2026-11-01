//! 信令与会话中转。
//!
//! 服务器只做三件事：
//! 1. 为被控端分配 6 位控制码（断线重连可取回原控制码）；
//! 2. 控制端按控制码发起会话时通知被控端；
//! 3. 把双方的会话流配对后，以 `Bytes` 块为单位零拷贝透传。
//!
//! 服务器从不解析画面/输入数据。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{ClientMsg, ConnectError, Peer, Reclaim, ServerMsg, SessionGrant};
use airhand_proto::{CODE_MAX, CODE_MIN, MAX_SIGNAL_MSG, PROTOCOL_VERSION, Role, SessionHeader};
use anyhow::{Context, Result, anyhow, bail};
use quinn::{RecvStream, SendStream, VarInt};
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// 被控端掉线后，控制码保留的时长（期间只有原设备能取回）。
const RESERVE_TTL: Duration = Duration::from_secs(10 * 60);
/// 会话配对等待时长。
const PENDING_TTL: Duration = Duration::from_secs(20);
/// 失败尝试统计窗口与上限（防暴力猜码）。
const FAIL_WINDOW: Duration = Duration::from_secs(10 * 60);
const FAIL_LIMIT: usize = 20;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Config {
    pub max_sessions_per_host: usize,
}

struct HostEntry {
    conn_id: u64,
    secret: [u8; 16],
    name: String,
    os: String,
    tx: mpsc::UnboundedSender<ServerMsg>,
    conn: quinn::Connection,
    sessions: Arc<AtomicUsize>,
}

struct Reserved {
    secret: [u8; 16],
    until: Instant,
}

/// 会话槽位，Drop 时自动归还。
struct SlotGuard(Arc<AtomicUsize>);
impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Pending {
    host_conn: u64,
    ctrl_conn: u64,
    created: Instant,
    waiting: Option<(Role, SendStream, RecvStream)>,
    slot: SlotGuard,
    label: String,
}

pub struct Broker {
    cfg: Config,
    hosts: Mutex<HashMap<u32, HostEntry>>,
    reserved: Mutex<HashMap<u32, Reserved>>,
    pending: Mutex<HashMap<u64, Pending>>,
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    next_conn: AtomicU64,
}

impl Broker {
    pub fn new(cfg: Config) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            hosts: Mutex::default(),
            reserved: Mutex::default(),
            pending: Mutex::default(),
            failures: Mutex::default(),
            next_conn: AtomicU64::new(1),
        })
    }

    /// 周期性清理过期数据。
    pub async fn janitor(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            let now = Instant::now();
            self.reserved.lock().unwrap().retain(|_, r| r.until > now);
            let expired: Vec<Pending> = {
                let mut p = self.pending.lock().unwrap();
                let ids: Vec<u64> = p
                    .iter()
                    .filter(|(_, v)| now - v.created > PENDING_TTL)
                    .map(|(k, _)| *k)
                    .collect();
                ids.into_iter().filter_map(|id| p.remove(&id)).collect()
            };
            for p in expired {
                debug!("session pairing timed out: {}", p.label);
                if let Some((_, mut s, mut r)) = p.waiting {
                    let _ = s.reset(VarInt::from_u32(1));
                    let _ = r.stop(VarInt::from_u32(1));
                }
            }
            self.failures.lock().unwrap().retain(|_, v| {
                v.retain(|t| now - *t < FAIL_WINDOW);
                !v.is_empty()
            });
        }
    }

    pub fn stats(&self) -> (usize, usize) {
        (
            self.hosts.lock().unwrap().len(),
            self.pending.lock().unwrap().len(),
        )
    }

    pub async fn handle_incoming(self: Arc<Self>, incoming: quinn::Incoming) {
        let peer = incoming.remote_address();
        let conn = match timeout(HANDSHAKE_TIMEOUT, incoming).await {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                debug!(%peer, "handshake failed: {e}");
                return;
            }
            Err(_) => {
                debug!(%peer, "handshake timed out");
                return;
            }
        };
        if let Err(e) = self.handle_conn(conn.clone(), peer).await {
            debug!(%peer, "connection ended: {e:#}");
        }
        conn.close(VarInt::from_u32(0), b"bye");
    }

    async fn handle_conn(
        self: &Arc<Self>,
        conn: quinn::Connection,
        peer: SocketAddr,
    ) -> Result<()> {
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (mut send, mut recv) = timeout(HANDSHAKE_TIMEOUT, conn.accept_bi())
            .await
            .context("timed out waiting for the control stream")??;
        let mut buf = Vec::new();
        let hello: ClientMsg = timeout(
            HANDSHAKE_TIMEOUT,
            read_msg(&mut recv, MAX_SIGNAL_MSG, &mut buf),
        )
        .await
        .context("timed out waiting for Hello")??
        .ok_or_else(|| anyhow!("peer closed the stream"))?;
        let ClientMsg::Hello {
            version,
            role,
            name,
            os,
        } = hello
        else {
            bail!("the first message must be Hello");
        };
        let name: String = name.chars().take(64).collect();
        let os: String = os.chars().take(64).collect();
        if version != PROTOCOL_VERSION {
            info!(%peer, version, "rejected client: protocol version mismatch");
            write_msg(
                &mut send,
                &ServerMsg::Incompatible {
                    server_version: PROTOCOL_VERSION,
                },
            )
            .await?;
            let _ = send.finish();
            tokio::time::sleep(Duration::from_millis(500)).await;
            return Ok(());
        }
        write_msg(
            &mut send,
            &ServerMsg::Welcome {
                version: PROTOCOL_VERSION,
            },
        )
        .await?;
        info!(%peer, conn_id, ?role, %name, %os, "client connected");

        let (tx, rx) = mpsc::unbounded_channel::<ServerMsg>();
        let writer = tokio::spawn(writer_loop(send, rx));
        let acceptor = tokio::spawn(self.clone().accept_sessions(conn.clone(), conn_id, role));

        let res = match role {
            Role::Host => {
                self.host_loop(&conn, conn_id, peer, name, os, tx, recv, &mut buf)
                    .await
            }
            Role::Controller => {
                self.controller_loop(conn_id, peer, name, tx, recv, &mut buf)
                    .await
            }
        };

        acceptor.abort();
        writer.abort();
        if role == Role::Host {
            self.unregister(conn_id);
        }
        info!(%peer, conn_id, ?role, "client disconnected");
        res
    }

    #[allow(clippy::too_many_arguments)]
    async fn host_loop(
        self: &Arc<Self>,
        conn: &quinn::Connection,
        conn_id: u64,
        peer: SocketAddr,
        name: String,
        os: String,
        tx: mpsc::UnboundedSender<ServerMsg>,
        mut recv: RecvStream,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let mut registered = false;
        while let Some(msg) = read_msg::<_, ClientMsg>(&mut recv, MAX_SIGNAL_MSG, buf).await? {
            match msg {
                ClientMsg::Register { reclaim } if !registered => {
                    let (code, secret) = self.register(conn, conn_id, &name, &os, &tx, reclaim);
                    registered = true;
                    info!(%peer, conn_id, code, reclaimed = reclaim.is_some_and(|r| r.code == code), "host registered");
                    let _ = tx.send(ServerMsg::Registered { code, secret });
                }
                other => warn!(%peer, "invalid message from host: {other:?}"),
            }
        }
        Ok(())
    }

    fn register(
        &self,
        conn: &quinn::Connection,
        conn_id: u64,
        name: &str,
        os: &str,
        tx: &mpsc::UnboundedSender<ServerMsg>,
        reclaim: Option<Reclaim>,
    ) -> (u32, [u8; 16]) {
        let mut hosts = self.hosts.lock().unwrap();
        let mut reserved = self.reserved.lock().unwrap();
        let now = Instant::now();
        let mut rng = rand::rng();

        let mut chosen: Option<(u32, [u8; 16])> = None;
        if let Some(r) = reclaim.filter(|r| (CODE_MIN..=CODE_MAX).contains(&r.code)) {
            if let Some(old) = hosts.get(&r.code) {
                // 旧连接尚未超时（例如网络切换后立即重连）：密钥匹配则顶替旧连接
                if old.secret == r.secret {
                    old.conn.close(VarInt::from_u32(2), b"replaced");
                    hosts.remove(&r.code);
                    chosen = Some((r.code, r.secret));
                }
            } else if let Some(res) = reserved.get(&r.code) {
                if res.secret == r.secret && res.until > now {
                    reserved.remove(&r.code);
                    chosen = Some((r.code, r.secret));
                }
            } else {
                // 服务器重启过，控制码空闲，直接还给原设备
                chosen = Some((r.code, r.secret));
            }
        }

        let (code, secret) = chosen.unwrap_or_else(|| {
            let secret: [u8; 16] = rng.random();
            loop {
                let code = rng.random_range(CODE_MIN..=CODE_MAX);
                if !hosts.contains_key(&code) && !reserved.contains_key(&code) {
                    break (code, secret);
                }
            }
        });

        hosts.insert(
            code,
            HostEntry {
                conn_id,
                secret,
                name: name.to_string(),
                os: os.to_string(),
                tx: tx.clone(),
                conn: conn.clone(),
                sessions: Arc::new(AtomicUsize::new(0)),
            },
        );
        (code, secret)
    }

    fn unregister(&self, conn_id: u64) {
        let mut hosts = self.hosts.lock().unwrap();
        let code = hosts
            .iter()
            .find(|(_, h)| h.conn_id == conn_id)
            .map(|(c, _)| *c);
        if let Some(code) = code {
            let h = hosts.remove(&code).unwrap();
            self.reserved.lock().unwrap().insert(
                code,
                Reserved {
                    secret: h.secret,
                    until: Instant::now() + RESERVE_TTL,
                },
            );
        }
    }

    async fn controller_loop(
        self: &Arc<Self>,
        conn_id: u64,
        peer: SocketAddr,
        name: String,
        tx: mpsc::UnboundedSender<ServerMsg>,
        mut recv: RecvStream,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let ip = limiter_key(peer.ip());
        while let Some(msg) = read_msg::<_, ClientMsg>(&mut recv, MAX_SIGNAL_MSG, buf).await? {
            let ClientMsg::Connect { req_id, code } = msg else {
                warn!(%peer, "invalid message from controller");
                continue;
            };
            let result = self.connect(conn_id, ip, peer, &name, code);
            if let Err(e) = &result {
                if *e == ConnectError::NotFound {
                    self.record_failure(ip);
                    // 放慢猜码速度
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                info!(%peer, code, "connect request failed: {e:?}");
            }
            let _ = tx.send(ServerMsg::ConnectResult { req_id, result });
        }
        Ok(())
    }

    fn connect(
        &self,
        ctrl_conn: u64,
        ip: IpAddr,
        peer: SocketAddr,
        ctrl_name: &str,
        code: u32,
    ) -> Result<SessionGrant, ConnectError> {
        if self.failure_count(ip) >= FAIL_LIMIT {
            return Err(ConnectError::RateLimited);
        }
        let hosts = self.hosts.lock().unwrap();
        let host = hosts.get(&code).ok_or(ConnectError::NotFound)?;
        // 预占一个会话槽位
        let prev = host.sessions.fetch_add(1, Ordering::AcqRel);
        let slot = SlotGuard(host.sessions.clone());
        if prev >= self.cfg.max_sessions_per_host {
            return Err(ConnectError::Busy);
        }
        let session_id = loop {
            let id: u64 = rand::rng().random();
            if id != 0 && !self.pending.lock().unwrap().contains_key(&id) {
                break id;
            }
        };
        let label = format!("{ctrl_name}@{peer} -> {code}");
        self.pending.lock().unwrap().insert(
            session_id,
            Pending {
                host_conn: host.conn_id,
                ctrl_conn,
                created: Instant::now(),
                waiting: None,
                slot,
                label: label.clone(),
            },
        );
        let incoming = ServerMsg::Incoming {
            session_id,
            peer: Peer {
                name: ctrl_name.to_string(),
                ip: peer.ip(),
            },
        };
        if host.tx.send(incoming).is_err() {
            self.pending.lock().unwrap().remove(&session_id);
            return Err(ConnectError::NotFound);
        }
        info!(session_id, "{label}: session created");
        Ok(SessionGrant {
            session_id,
            host_name: host.name.clone(),
            host_os: host.os.clone(),
        })
    }

    fn failure_count(&self, ip: IpAddr) -> usize {
        let now = Instant::now();
        self.failures
            .lock()
            .unwrap()
            .get(&ip)
            .map(|v| v.iter().filter(|t| now - **t < FAIL_WINDOW).count())
            .unwrap_or(0)
    }

    fn record_failure(&self, ip: IpAddr) {
        self.failures
            .lock()
            .unwrap()
            .entry(ip)
            .or_default()
            .push(Instant::now());
    }

    async fn accept_sessions(self: Arc<Self>, conn: quinn::Connection, conn_id: u64, role: Role) {
        while let Ok((send, recv)) = conn.accept_bi().await {
            let me = self.clone();
            tokio::spawn(async move {
                if let Err(e) = me.attach_stream(conn_id, role, send, recv).await {
                    debug!(conn_id, "session stream rejected: {e:#}");
                }
            });
        }
    }

    async fn attach_stream(
        self: Arc<Self>,
        conn_id: u64,
        role: Role,
        send: SendStream,
        mut recv: RecvStream,
    ) -> Result<()> {
        let mut hdr = [0u8; SessionHeader::LEN];
        timeout(HANDSHAKE_TIMEOUT, recv.read_exact(&mut hdr))
            .await
            .context("timed out reading the session header")??;
        let reject = |mut send: SendStream, mut recv: RecvStream| {
            let _ = send.reset(VarInt::from_u32(1));
            let _ = recv.stop(VarInt::from_u32(1));
        };
        let Some(h) = SessionHeader::decode(&hdr).filter(|h| h.role == role) else {
            reject(send, recv);
            bail!("invalid session header");
        };

        let mut pending = self.pending.lock().unwrap();
        let Some(p) = pending.get_mut(&h.session_id) else {
            drop(pending);
            reject(send, recv);
            bail!("no such session: {}", h.session_id);
        };
        let expected = match role {
            Role::Host => p.host_conn,
            Role::Controller => p.ctrl_conn,
        };
        if expected != conn_id {
            drop(pending);
            reject(send, recv);
            bail!("the session doesn't belong to this connection");
        }
        match p.waiting.take() {
            None => {
                p.waiting = Some((role, send, recv));
                Ok(())
            }
            Some(w) if w.0 == role => {
                p.waiting = Some(w);
                drop(pending);
                reject(send, recv);
                bail!("duplicate session stream");
            }
            Some((_, other_send, other_recv)) => {
                let p = pending.remove(&h.session_id).unwrap();
                drop(pending);
                let session_id = h.session_id;
                info!(session_id, "{}: session established", p.label);
                tokio::spawn(async move {
                    let started = Instant::now();
                    let (up, down) = relay((send, recv), (other_send, other_recv)).await;
                    info!(
                        session_id,
                        secs = started.elapsed().as_secs(),
                        "{}: session ended ({} / {} bytes)",
                        p.label,
                        up,
                        down
                    );
                    drop(p.slot);
                });
                Ok(())
            }
        }
    }
}

async fn writer_loop(mut send: SendStream, mut rx: mpsc::UnboundedReceiver<ServerMsg>) {
    while let Some(msg) = rx.recv().await {
        if write_msg(&mut send, &msg).await.is_err() {
            break;
        }
    }
}

/// 双向透传，任一方向结束即拆除整个会话。返回两个方向各自转发的字节数。
async fn relay(a: (SendStream, RecvStream), b: (SendStream, RecvStream)) -> (u64, u64) {
    let (mut a_tx, mut a_rx) = a;
    let (mut b_tx, mut b_rx) = b;
    let ab = Arc::new(AtomicU64::new(0));
    let ba = Arc::new(AtomicU64::new(0));
    tokio::select! {
        _ = pipe(&mut a_rx, &mut b_tx, &ab) => {},
        _ = pipe(&mut b_rx, &mut a_tx, &ba) => {},
    }
    for tx in [&mut a_tx, &mut b_tx] {
        let _ = tx.finish();
    }
    // 给对端一点时间读完已发送数据，然后彻底关闭
    tokio::time::sleep(Duration::from_millis(200)).await;
    for rx in [&mut a_rx, &mut b_rx] {
        let _ = rx.stop(VarInt::from_u32(0));
    }
    (ab.load(Ordering::Relaxed), ba.load(Ordering::Relaxed))
}

async fn pipe(rx: &mut RecvStream, tx: &mut SendStream, counter: &AtomicU64) -> Result<()> {
    while let Some(chunk) = rx.read_chunk(256 * 1024, true).await? {
        counter.fetch_add(chunk.bytes.len() as u64, Ordering::Relaxed);
        tx.write_chunk(chunk.bytes).await?;
    }
    Ok(())
}

/// IPv6 按 /64 归并，防止换地址绕过限速。
fn limiter_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return IpAddr::V4(v4);
            }
            let mut seg = v6.segments();
            seg[4..].fill(0);
            IpAddr::V6(seg.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_key_groups_addresses() {
        let key = |s: &str| limiter_key(s.parse().unwrap()).to_string();
        assert_eq!(key("203.0.113.9"), "203.0.113.9");
        // 同一个 /64 里换地址也算同一个来源
        assert_eq!(key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::");
        assert_eq!(key("2001:db8:1:2:bbbb::9"), "2001:db8:1:2::");
        // IPv4 映射地址按 IPv4 计
        assert_eq!(key("::ffff:203.0.113.9"), "203.0.113.9");
    }
}
