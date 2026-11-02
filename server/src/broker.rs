//! 信令与会话中转。
//!
//! 服务器只做三件事：
//! 1. 为被控端分配 6 位控制码（断线重连可取回原控制码）；
//! 2. 控制端按控制码发起会话时通知被控端；
//! 3. 把双方的会话流配对后，以 `Bytes` 块为单位零拷贝透传。
//!
//! 服务器从不解析画面/输入数据。
//!
//! 防暴力猜码：控制码不存在算一次猜错（[`FAIL_LIMIT`]）。被控端掉线后，之前连上过它的控制端带着重连凭证
//! 重连（见 `airhand_proto::signal` 模块说明的"重连凭证"）：这些请求不算猜错、不受限速影响，
//! 被控端不在线时服务器记下它们（[`Waiter`]），被控端重新上线时通知它们重连。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::signal::{
    ClientMsg, ConnectError, Peer, Reclaim, ResumeToken, ServerMsg, SessionGrant,
};
use airhand_proto::{CODE_MAX, CODE_MIN, MAX_SIGNAL_MSG, PROTOCOL_VERSION, Role, SessionHeader};
use anyhow::{Context, Result, anyhow, bail};
use quinn::{RecvStream, SendStream, VarInt};
use rand::Rng;
use sha2::{Digest, Sha256};
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
/// 每条控制端连接最多同时等这么多个被控端上线，再多就忘掉最早的
const MAX_WAITING: usize = 32;

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

/// 控制码 `code`、密钥 `secret` 对应的重连凭证。密钥只有服务器和被控端知道，
/// 所以凭证只能从服务器那里得到（连上过这个被控端），也不能从凭证反推出密钥（取回控制码要用）。
/// 被控端取回控制码时密钥不变，凭证也就仍然有效（包括服务器重启之后）。
fn resume_token(code: u32, secret: &[u8; 16]) -> ResumeToken {
    let mut h = Sha256::new();
    h.update(b"airhand resume token v1\0");
    h.update(code.to_le_bytes());
    h.update(secret);
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.finalize()[..16]);
    out
}

/// 比较凭证，用时与内容无关（不能靠测量回答的快慢逐字节猜出凭证）
fn same_token(a: &ResumeToken, b: &ResumeToken) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 等被控端上线的控制端（回答过它 [`ConnectError::Offline`]）。
/// 被控端用这个控制码上线、并且凭证对得上时发 [`ServerMsg::HostOnline`]，然后忘掉。
struct Waiter {
    conn_id: u64,
    resume: ResumeToken,
    tx: mpsc::UnboundedSender<ServerMsg>,
}

/// 发起连接请求的控制端（一条连接）
struct Ctrl<'a> {
    conn_id: u64,
    /// 限速按这个来源统计（[`limiter_key`]）
    ip: IpAddr,
    peer: SocketAddr,
    name: &'a str,
    tx: &'a mpsc::UnboundedSender<ServerMsg>,
}

/// 连接请求被拒绝的原因（发给控制端的是 [`Denied::error`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Denied {
    /// 控制码不存在：算一次猜错
    Unknown,
    /// 控制码没错，但被控端正在断开：不算猜错
    Vanished,
    /// 回答带凭证的请求：被控端不在线、凭证不对、控制码不存在都是这个回答（不透露控制码是否存在）
    Offline,
    /// 猜错次数过多
    Limited,
    /// 对方会话数已满
    Busy,
}

impl Denied {
    fn error(self) -> ConnectError {
        match self {
            Denied::Unknown | Denied::Vanished => ConnectError::NotFound,
            Denied::Offline => ConnectError::Offline,
            Denied::Limited => ConnectError::RateLimited,
            Denied::Busy => ConnectError::Busy,
        }
    }
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
    /// 控制码 → 等它上线的控制端。加锁顺序：hosts → reserved → waiters
    waiters: Mutex<HashMap<u32, Vec<Waiter>>>,
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
            waiters: Mutex::default(),
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
                    let code = self.register(conn, conn_id, &name, &os, &tx, reclaim);
                    registered = true;
                    info!(%peer, conn_id, code, reclaimed = reclaim.is_some_and(|r| r.code == code), "host registered");
                }
                other => warn!(%peer, "invalid message from host: {other:?}"),
            }
        }
        Ok(())
    }

    /// 分配（或者还回原来的）控制码，发 `Registered`，并通知在等这个被控端的控制端。
    fn register(
        &self,
        conn: &quinn::Connection,
        conn_id: u64,
        name: &str,
        os: &str,
        tx: &mpsc::UnboundedSender<ServerMsg>,
        reclaim: Option<Reclaim>,
    ) -> u32 {
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
                // 服务器重启过（或者保留期过了、控制码还没给别人），控制码空闲，直接还给原设备
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
        // 在通知控制端之前（并且还持有锁）发出：被控端收到的第一条消息必须是 Registered，
        // 控制端收到通知后马上发起的会话（Incoming）只能排在它后面
        let _ = tx.send(ServerMsg::Registered { code, secret });

        // 叫醒在等这个被控端的控制端。凭证对不上的直接忘掉：它们等的是以前用过这个控制码的另一台被控端
        if let Some(list) = self.waiters.lock().unwrap().remove(&code) {
            let token = resume_token(code, &secret);
            for w in list.iter().filter(|w| same_token(&w.resume, &token)) {
                debug!(
                    code,
                    ctrl = w.conn_id,
                    "telling a waiting controller that the host is back"
                );
                let _ = w.tx.send(ServerMsg::HostOnline { code });
            }
        }
        code
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
        let ctrl = Ctrl {
            conn_id,
            ip: limiter_key(peer.ip()),
            peer,
            name: &name,
            tx: &tx,
        };
        // 这条连接在等哪些被控端上线（最早的在前面）
        let mut waiting: Vec<u32> = Vec::new();
        let result = async {
            while let Some(msg) = read_msg::<_, ClientMsg>(&mut recv, MAX_SIGNAL_MSG, buf).await? {
                let ClientMsg::Connect {
                    req_id,
                    code,
                    resume,
                } = msg
                else {
                    warn!(%peer, "invalid message from controller");
                    continue;
                };
                let result = self.connect(&ctrl, code, resume, &mut waiting);
                if let Err(denied) = result {
                    if denied == Denied::Unknown {
                        self.record_failure(ctrl.ip);
                        // 放慢猜码速度
                        tokio::time::sleep(Duration::from_millis(400)).await;
                    }
                    info!(%peer, code, "connect request failed: {denied:?}");
                }
                let result = result.map_err(Denied::error);
                let _ = tx.send(ServerMsg::ConnectResult { req_id, result });
            }
            anyhow::Ok(())
        }
        .await;
        let mut all = self.waiters.lock().unwrap();
        for code in waiting {
            forget_waiter(&mut all, code, conn_id);
        }
        result
    }

    /// `resume` 是控制端带来的重连凭证（见 [`resume_token`]）。
    fn connect(
        &self,
        ctrl: &Ctrl,
        code: u32,
        resume: Option<ResumeToken>,
        waiting: &mut Vec<u32>,
    ) -> Result<SessionGrant, Denied> {
        let hosts = self.hosts.lock().unwrap();
        let host = match resume {
            // 带凭证：凭证对得上才回答实情，否则（被控端不在线、凭证不对、控制码不存在）一律"不在线"。
            // 回答不透露控制码是否存在，所以不算猜错，也不必限速
            Some(resume) => match hosts.get(&code) {
                Some(h) if same_token(&resume_token(code, &h.secret), &resume) => h,
                _ => {
                    self.wait(ctrl, code, resume, waiting);
                    return Err(Denied::Offline);
                }
            },
            None => {
                // 先检查限速再查控制码：被限速时不透露控制码是否存在
                if self.failure_count(ctrl.ip) >= FAIL_LIMIT {
                    return Err(Denied::Limited);
                }
                hosts.get(&code).ok_or(Denied::Unknown)?
            }
        };
        // 预占一个会话槽位
        let prev = host.sessions.fetch_add(1, Ordering::AcqRel);
        let slot = SlotGuard(host.sessions.clone());
        if prev >= self.cfg.max_sessions_per_host {
            return Err(Denied::Busy);
        }
        let session_id = loop {
            let id: u64 = rand::rng().random();
            if id != 0 && !self.pending.lock().unwrap().contains_key(&id) {
                break id;
            }
        };
        let label = format!("{}@{} -> {code}", ctrl.name, ctrl.peer);
        self.pending.lock().unwrap().insert(
            session_id,
            Pending {
                host_conn: host.conn_id,
                ctrl_conn: ctrl.conn_id,
                created: Instant::now(),
                waiting: None,
                slot,
                label: label.clone(),
            },
        );
        let incoming = ServerMsg::Incoming {
            session_id,
            peer: Peer {
                name: ctrl.name.to_string(),
                ip: ctrl.peer.ip(),
            },
        };
        if host.tx.send(incoming).is_err() {
            // 被控端正在断开
            self.pending.lock().unwrap().remove(&session_id);
            return Err(match resume {
                Some(resume) => {
                    self.wait(ctrl, code, resume, waiting);
                    Denied::Offline
                }
                None => Denied::Vanished,
            });
        }
        info!(session_id, "{label}: session created");
        Ok(SessionGrant {
            session_id,
            host_name: host.name.clone(),
            host_os: host.os.clone(),
            resume: resume_token(code, &host.secret),
        })
    }

    /// 记下 `ctrl` 在等 `code` 上线（调用方持有 hosts 锁，所以不会错过同时发生的上线）。
    /// `waiting` 是这条连接在等的控制码，最多 [`MAX_WAITING`] 个
    fn wait(&self, ctrl: &Ctrl, code: u32, resume: ResumeToken, waiting: &mut Vec<u32>) {
        let mut all = self.waiters.lock().unwrap();
        let list = all.entry(code).or_default();
        list.retain(|w| w.conn_id != ctrl.conn_id);
        list.push(Waiter {
            conn_id: ctrl.conn_id,
            resume,
            tx: ctrl.tx.clone(),
        });
        waiting.retain(|c| *c != code);
        waiting.push(code);
        if waiting.len() > MAX_WAITING {
            let oldest = waiting.remove(0);
            forget_waiter(&mut all, oldest, ctrl.conn_id);
        }
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

/// 连接 `conn_id` 不再等 `code` 上线了
fn forget_waiter(all: &mut HashMap<u32, Vec<Waiter>>, code: u32, conn_id: u64) {
    if let Some(list) = all.get_mut(&code) {
        list.retain(|w| w.conn_id != conn_id);
        if list.is_empty() {
            all.remove(&code);
        }
    }
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
    use airhand_link::Problem;
    use airhand_link::controller::{
        ControllerService, Frontend, SessionEvent, SessionUi, UiClosed,
    };
    use airhand_link::net::{LinkConfig, Signal, connect_signal};
    use airhand_proto::session::QualitySettings;
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

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

    #[test]
    fn resume_token_binds_code_and_secret() {
        let t = resume_token(123_456, &[1; 16]);
        assert!(same_token(&t, &resume_token(123_456, &[1; 16])));
        assert!(!same_token(&t, &resume_token(123_457, &[1; 16])));
        assert!(!same_token(&t, &resume_token(123_456, &[2; 16])));
    }

    /// 进程内的服务器（只监听 127.0.0.1）
    async fn start() -> (Arc<Broker>, LinkConfig) {
        let ck = rcgen::generate_simple_self_signed(vec!["airhand".into()]).unwrap();
        let chain = vec![CertificateDer::from(ck.cert.der().to_vec())];
        let key = PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()).into();
        let endpoint = quinn::Endpoint::server(
            crate::server_config(chain, key).unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let server = endpoint.local_addr().unwrap().to_string();
        let broker = Broker::new(Config {
            max_sessions_per_host: 8,
        });
        let b = broker.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                tokio::spawn(b.clone().handle_incoming(incoming));
            }
        });
        let cfg = LinkConfig::new(server, "", "test", "test").unwrap();
        (broker, cfg)
    }

    async fn next(sig: &mut Signal) -> ServerMsg {
        let mut buf = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(5),
            read_msg(&mut sig.recv, MAX_SIGNAL_MSG, &mut buf),
        )
        .await
        .expect("timed out waiting for a server message")
        .unwrap()
        .expect("the server closed the control stream")
    }

    async fn register(cfg: &LinkConfig, reclaim: Option<Reclaim>) -> (Signal, Reclaim) {
        let mut sig = connect_signal(cfg, Role::Host).await.unwrap();
        write_msg(&mut sig.send, &ClientMsg::Register { reclaim })
            .await
            .unwrap();
        match next(&mut sig).await {
            ServerMsg::Registered { code, secret } => (sig, Reclaim { code, secret }),
            other => panic!("{other:?}"),
        }
    }

    /// 发一个连接请求，下一条消息必须是它的回答
    async fn ask(
        ctrl: &mut Signal,
        req_id: u32,
        code: u32,
        resume: Option<ResumeToken>,
    ) -> Result<SessionGrant, ConnectError> {
        write_msg(
            &mut ctrl.send,
            &ClientMsg::Connect {
                req_id,
                code,
                resume,
            },
        )
        .await
        .unwrap();
        match next(ctrl).await {
            ServerMsg::ConnectResult { req_id: r, result } if r == req_id => result,
            other => panic!("{other:?}"),
        }
    }

    async fn wait_until(what: &str, f: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn waiter_count(broker: &Broker) -> usize {
        broker.waiters.lock().unwrap().values().map(Vec::len).sum()
    }

    /// 带凭证的请求：凭证不对时什么也不透露；被控端掉线期间一直重连不算猜码、不受限速影响，
    /// 被控端回来时马上收到通知
    #[tokio::test]
    async fn resume_and_wait_for_the_host() {
        let (broker, cfg) = start().await;
        let ip = limiter_key("127.0.0.1".parse().unwrap());
        let (host, id) = register(&cfg, None).await;
        let code = id.code;
        let other = if code == CODE_MAX { CODE_MIN } else { code + 1 };
        let mut ctrl = connect_signal(&cfg, Role::Controller).await.unwrap();
        let token = ask(&mut ctrl, 1, code, None).await.unwrap().resume;

        // 凭证不对：被控端在线也不给连，回答和控制码不存在时一样
        let mut stranger = connect_signal(&cfg, Role::Controller).await.unwrap();
        for (req, c, t) in [(1, code, [0; 16]), (2, other, [0; 16]), (3, other, token)] {
            assert_eq!(
                ask(&mut stranger, req, c, Some(t)).await.unwrap_err(),
                ConnectError::Offline
            );
        }
        assert_eq!(broker.failure_count(ip), 0);

        // 被控端掉线
        host.conn.close(VarInt::from_u32(0), b"");
        wait_until("the host is gone", || broker.stats().0 == 0).await;
        drop(host);

        // 这个 IP 已经被限速了，带凭证的重连照样不受影响，也不算猜错
        for _ in 0..FAIL_LIMIT {
            broker.record_failure(ip);
        }
        assert_eq!(
            ask(&mut ctrl, 10, other, None).await.unwrap_err(),
            ConnectError::RateLimited
        );
        for i in 0..5 {
            assert_eq!(
                ask(&mut ctrl, 11 + i, code, Some(token)).await.unwrap_err(),
                ConnectError::Offline
            );
        }
        assert_eq!(broker.failure_count(ip), FAIL_LIMIT);

        // 被控端取回控制码：在等的控制端收到通知（只一次），凭证对不上的收不到
        let (_host, again) = register(&cfg, Some(id)).await;
        assert_eq!(again.code, code);
        assert!(matches!(next(&mut ctrl).await, ServerMsg::HostOnline { code: c } if c == code));
        assert!(ask(&mut ctrl, 20, code, Some(token)).await.is_ok());
        // 下一条消息就是回答，说明 stranger 没有收到通知
        assert_eq!(
            ask(&mut stranger, 4, other, Some(token)).await.unwrap_err(),
            ConnectError::Offline
        );

        // 每条连接最多等 MAX_WAITING 个被控端；连接断开后全部忘掉
        for i in 0..MAX_WAITING as u32 + 8 {
            let c = CODE_MIN + i;
            if c != code {
                let _ = ask(&mut stranger, 100 + i, c, Some([0; 16])).await;
            }
        }
        assert_eq!(waiter_count(&broker), MAX_WAITING);
        stranger.conn.close(VarInt::from_u32(0), b"");
        wait_until("the waiters are forgotten", || waiter_count(&broker) == 0).await;
    }

    /// 控制端服务（airhand-link）：重连时自动带上凭证；被控端不在线时回答 HostOffline，
    /// 被控端回来时调用 host_online
    #[tokio::test]
    async fn controller_service_waits_for_the_host() {
        struct NoUi;
        impl SessionUi for NoUi {
            fn packet(&self, _: Vec<u8>) -> Result<(), UiClosed> {
                Ok(())
            }
            fn event(&self, _: SessionEvent) -> Result<(), UiClosed> {
                Ok(())
            }
        }

        let (broker, cfg) = start().await;
        let ip = limiter_key("127.0.0.1".parse().unwrap());
        let (host, id) = register(&cfg, None).await;
        let (online_tx, mut online_rx) = mpsc::unbounded_channel();
        let svc = ControllerService::start(
            Arc::new(cfg.clone()),
            tokio::runtime::Handle::current(),
            Arc::new(|_| {}),
            Arc::new(move |code| {
                let _ = online_tx.send(code);
            }),
        );
        let connect = |resume| {
            svc.connect(
                id.code,
                resume,
                QualitySettings::default(),
                Arc::new(NoUi),
                Frontend::Browser,
            )
        };
        let first = loop {
            match connect(None).await {
                // 控制端服务还没连上服务器
                Err(Problem::NotConnected) => tokio::time::sleep(Duration::from_millis(20)).await,
                r => break r,
            }
        };
        let token = Some(first.unwrap().resume);

        host.conn.close(VarInt::from_u32(0), b"");
        wait_until("the host is gone", || broker.stats().0 == 0).await;
        drop(host);
        for _ in 0..FAIL_LIMIT {
            broker.record_failure(ip);
        }
        assert_eq!(connect(token).await.unwrap_err(), Problem::HostOffline);
        // 不带凭证：普通的连接请求，照常限速
        assert_eq!(connect(None).await.unwrap_err(), Problem::RateLimited);

        let (_host, _) = register(&cfg, Some(id)).await;
        let code = tokio::time::timeout(Duration::from_secs(5), online_rx.recv())
            .await
            .expect("host_online wasn't called")
            .unwrap();
        assert_eq!(code, id.code);
        assert!(connect(token).await.is_ok());
    }

    /// 服务器重启之后（什么都不记得了），控制端可能比被控端先回来：凭证照样可以用
    #[tokio::test]
    async fn resume_survives_a_server_restart() {
        let (_old, cfg) = start().await;
        let (host, id) = register(&cfg, None).await;
        let mut ctrl = connect_signal(&cfg, Role::Controller).await.unwrap();
        let token = ask(&mut ctrl, 1, id.code, None).await.unwrap().resume;
        drop((host, ctrl));

        let (_new, cfg) = start().await;
        let mut ctrl = connect_signal(&cfg, Role::Controller).await.unwrap();
        assert_eq!(
            ask(&mut ctrl, 1, id.code, Some(token)).await.unwrap_err(),
            ConnectError::Offline
        );
        let (_host, again) = register(&cfg, Some(id)).await;
        assert_eq!(again.code, id.code);
        assert!(matches!(next(&mut ctrl).await, ServerMsg::HostOnline { code } if code == id.code));
        assert!(ask(&mut ctrl, 2, id.code, Some(token)).await.is_ok());
    }
}
