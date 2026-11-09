//! 信令与会话中转。
//!
//! 服务器只做三件事：
//! 1. 为被控端分配 6 位控制码（断线重连可取回原控制码；保存了控制码的被控端一直为它保留，见 [`crate::codes`]）；
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

use crate::codes::Bindings;

/// 被控端掉线后，控制码保留的时长（期间只有原设备能取回）。固定的控制码（`keep`）一直保留，见 [`crate::codes`]。
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
    /// 固定控制码的绑定表文件，`None` 表示只在内存里
    pub db: Option<std::path::PathBuf>,
}

/// Bounded signaling output. A non-reading peer is disconnected on overflow,
/// rather than retaining arbitrarily many replies or silently losing grants.
#[derive(Clone)]
struct SignalTx {
    tx: mpsc::Sender<ServerMsg>,
    conn: quinn::Connection,
}
impl SignalTx {
    fn send(&self, msg: ServerMsg) -> Result<(), ()> {
        self.tx.try_send(msg).map_err(|_| {
            self.conn
                .close(VarInt::from_u32(3), b"signal queue overflow");
        })
    }
}

struct HostEntry {
    conn_id: u64,
    ready: bool,
    secret: [u8; 16],
    /// 被控端发过 [`ClientMsg::Release`]：断开后不再保留这个控制码
    released: bool,
    name: String,
    os: String,
    tx: SignalTx,
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
    tx: SignalTx,
}

/// 发起连接请求的控制端（一条连接）
struct Ctrl<'a> {
    conn_id: u64,
    /// 限速按这个来源统计（[`limiter_key`]）
    ip: IpAddr,
    peer: SocketAddr,
    name: &'a str,
    tx: &'a SignalTx,
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
    host_streams: [Option<(SendStream, RecvStream)>; 3],
    ctrl_streams: [Option<(SendStream, RecvStream)>; 3],
    slot: SlotGuard,
    label: String,
}

pub struct Broker {
    cfg: Config,
    hosts: Mutex<HashMap<u32, HostEntry>>,
    reserved: Mutex<HashMap<u32, Reserved>>,
    pending: Mutex<HashMap<u64, Pending>>,
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    /// 固定的控制码。加锁顺序：hosts → reserved → bindings → waiters
    bindings: Mutex<Bindings>,
    /// 控制码 → 等它上线的控制端
    waiters: Mutex<HashMap<u32, Vec<Waiter>>>,
    next_conn: AtomicU64,
}

impl Broker {
    pub fn new(cfg: Config) -> Arc<Self> {
        let bindings = Bindings::open(cfg.db.as_deref());
        Arc::new(Self {
            cfg,
            hosts: Mutex::default(),
            reserved: Mutex::default(),
            pending: Mutex::default(),
            failures: Mutex::default(),
            bindings: Mutex::new(bindings),
            waiters: Mutex::default(),
            next_conn: AtomicU64::new(1),
        })
    }

    /// 周期性清理过期数据。
    pub async fn janitor(self: Arc<Self>) {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        let mut next_expiry = Instant::now();
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
                for (mut s, mut r) in p.host_streams.into_iter().chain(p.ctrl_streams).flatten() {
                    let _ = s.reset(VarInt::from_u32(1));
                    let _ = r.stop(VarInt::from_u32(1));
                }
            }
            self.failures.lock().unwrap().retain(|_, v| {
                v.retain(|t| now - *t < FAIL_WINDOW);
                !v.is_empty()
            });
            if now >= next_expiry {
                let hosts = self.hosts.lock().unwrap();
                let mut bindings = self.bindings.lock().unwrap();
                bindings.expire(|code| hosts.contains_key(&code));
                next_expiry = now + Duration::from_secs(3600);
            }
        }
    }

    pub async fn flush_bindings(&self) {
        let flush = self.bindings.lock().unwrap().flush_barrier();
        if !tokio::task::spawn_blocking(move || flush(Duration::from_secs(6)))
            .await
            .unwrap_or(false)
        {
            warn!(
                "fixed control codes: persistence barrier did not complete; keeping in-memory ownership"
            );
        }
    }

    /// (在线的被控端, 等待配对的会话, 固定的控制码)
    pub fn stats(&self) -> (usize, usize, usize) {
        (
            self.hosts.lock().unwrap().len(),
            self.pending.lock().unwrap().len(),
            self.bindings.lock().unwrap().len(),
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

        let (tx, rx) = mpsc::channel::<ServerMsg>(64);
        let tx = SignalTx {
            tx,
            conn: conn.clone(),
        };
        let writer_conn = conn.clone();
        let writer = tokio::spawn(async move {
            writer_loop(send, rx).await;
            writer_conn.close(VarInt::from_u32(3), b"signal writer ended");
        });
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
        tx: SignalTx,
        mut recv: RecvStream,
        buf: &mut Vec<u8>,
    ) -> Result<()> {
        let mut registered = false;
        while let Some(msg) = read_msg::<_, ClientMsg>(&mut recv, MAX_SIGNAL_MSG, buf).await? {
            match msg {
                ClientMsg::Register { reclaim, keep } if !registered => {
                    let code = self.register(conn, conn_id, &name, &os, &tx, reclaim, keep)?;
                    // Persistence waits outside ALL broker locks. Ordinary
                    // registrations are committed before they become visible.
                    self.flush_bindings().await;
                    self.announce(conn_id, code);
                    registered = true;
                    info!(%peer, conn_id, code, keep, reclaimed = reclaim.is_some_and(|r| r.code == code), "host registered");
                }
                ClientMsg::Release if registered => {
                    if let Some(code) = self.release(conn_id) {
                        info!(%peer, conn_id, code, "host released its control code");
                    }
                }
                other => warn!(%peer, "invalid message from host: {other:?}"),
            }
        }
        Ok(())
    }

    /// 分配（或者还回原来的）控制码，发 `Registered`，并通知在等这个被控端的控制端。
    /// `keep`：被控端保存了控制码，一直为它保留（绑定）。
    #[allow(clippy::too_many_arguments)]
    fn register(
        &self,
        conn: &quinn::Connection,
        conn_id: u64,
        name: &str,
        os: &str,
        tx: &SignalTx,
        reclaim: Option<Reclaim>,
        keep: bool,
    ) -> Result<u32> {
        let mut hosts = self.hosts.lock().unwrap();
        let mut reserved = self.reserved.lock().unwrap();
        let mut bindings = self.bindings.lock().unwrap();
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
            } else if bindings.check(r.code, &r.secret) != Some(false) {
                // 绑定给这台被控端的，或者控制码空闲（服务器重启过，或者保留期过了、还没给别人）：还给原设备
                chosen = Some((r.code, r.secret));
            }
        }

        let (code, secret) = match chosen {
            Some(chosen) => chosen,
            None => {
                let count = CODE_MAX - CODE_MIN + 1;
                let start = rng.random_range(0..count);
                let code = (0..count)
                    .map(|i| CODE_MIN + (start + i) % count)
                    .find(|c| {
                        !hosts.contains_key(c)
                            && !reserved.contains_key(c)
                            && !bindings.contains(*c)
                    })
                    .ok_or_else(|| anyhow!("control code namespace exhausted"))?;
                (code, rng.random())
            }
        };
        if keep {
            bindings.bind(code, &secret);
        } else if bindings.check(code, &secret) == Some(true) {
            // 这台被控端不再保存控制码了
            bindings.remove(code);
        }
        drop(bindings);

        hosts.insert(
            code,
            HostEntry {
                conn_id,
                ready: false,
                secret,
                released: false,
                name: name.to_string(),
                os: os.to_string(),
                tx: tx.clone(),
                conn: conn.clone(),
                sessions: Arc::new(AtomicUsize::new(0)),
            },
        );
        Ok(code)
    }

    fn announce(&self, conn_id: u64, code: u32) {
        let mut hosts = self.hosts.lock().unwrap();
        let Some(h) = hosts.get_mut(&code).filter(|h| h.conn_id == conn_id) else {
            return;
        };
        h.ready = true;
        let secret = h.secret;
        let tx = &h.tx;
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
    }

    fn unregister(&self, conn_id: u64) {
        let mut hosts = self.hosts.lock().unwrap();
        let code = hosts
            .iter()
            .find(|(_, h)| h.conn_id == conn_id)
            .map(|(c, _)| *c);
        if let Some(code) = code {
            let h = hosts.remove(&code).unwrap();
            if h.released {
                return;
            }
            self.reserved.lock().unwrap().insert(
                code,
                Reserved {
                    secret: h.secret,
                    until: Instant::now() + RESERVE_TTL,
                },
            );
            self.bindings.lock().unwrap().touch(code);
        }
    }

    /// 被控端放弃控制码：解除绑定，断开后也不保留。返回它的控制码
    fn release(&self, conn_id: u64) -> Option<u32> {
        let mut hosts = self.hosts.lock().unwrap();
        let (code, h) = hosts.iter_mut().find(|(_, h)| h.conn_id == conn_id)?;
        h.released = true;
        let code = *code;
        self.bindings.lock().unwrap().remove(code);
        Some(code)
    }

    async fn controller_loop(
        self: &Arc<Self>,
        conn_id: u64,
        peer: SocketAddr,
        name: String,
        tx: SignalTx,
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
        if !host.ready {
            return Err(match resume {
                Some(token) => {
                    self.wait(ctrl, code, token, waiting);
                    Denied::Offline
                }
                None => Denied::Vanished,
            });
        }
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
                host_streams: std::array::from_fn(|_| None),
                ctrl_streams: std::array::from_fn(|_| None),
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
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                stream = conn.accept_bi() => {
                    let Ok((send, recv)) = stream else { break; };
                    let me = self.clone();
                    tasks.spawn(async move {
                        if let Err(e) = me.attach_stream(conn_id, role, send, recv).await {
                            debug!(conn_id, "session stream rejected: {e:#}");
                        }
                    });
                }
                Some(_) = tasks.join_next() => {}
            }
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
        let slot = match role {
            Role::Host => &mut p.host_streams[h.lane as usize],
            Role::Controller => &mut p.ctrl_streams[h.lane as usize],
        };
        if slot.is_some() {
            drop(pending);
            reject(send, recv);
            bail!("duplicate session lane");
        }
        send.set_priority(match h.lane {
            airhand_proto::SessionLane::Control => 10,
            airhand_proto::SessionLane::Audio => 5,
            airhand_proto::SessionLane::Video => 0,
        })?;
        *slot = Some((send, recv));
        if !p
            .host_streams
            .iter()
            .chain(&p.ctrl_streams)
            .all(Option::is_some)
        {
            return Ok(());
        }
        let p = pending.remove(&h.session_id).unwrap();
        drop(pending);
        let session_id = h.session_id;
        info!(session_id, "{}: session established (3 lanes)", p.label);
        tokio::spawn(async move {
            let mut relays = tokio::task::JoinSet::new();
            for (a, b) in p.host_streams.into_iter().zip(p.ctrl_streams) {
                relays.spawn(relay(a.unwrap(), b.unwrap()));
            }
            // Every lane is required; closing any one tears down the whole session.
            let _ = relays.join_next().await;
            drop(relays);
            info!(session_id, "{}: session ended", p.label);
            drop(p.slot);
        });
        Ok(())
    }
}

async fn writer_loop(mut send: SendStream, mut rx: mpsc::Receiver<ServerMsg>) {
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

    /// 进程内的服务器（只监听 127.0.0.1），固定控制码只记在内存里
    async fn start() -> (Arc<Broker>, LinkConfig) {
        start_with(None).await
    }

    async fn start_with(db: Option<std::path::PathBuf>) -> (Arc<Broker>, LinkConfig) {
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
            db,
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
        register_with(cfg, reclaim, false).await
    }

    async fn register_with(
        cfg: &LinkConfig,
        reclaim: Option<Reclaim>,
        keep: bool,
    ) -> (Signal, Reclaim) {
        let mut sig = connect_signal(cfg, Role::Host).await.unwrap();
        write_msg(&mut sig.send, &ClientMsg::Register { reclaim, keep })
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

    #[tokio::test]
    async fn blocked_video_lane_does_not_block_control_or_audio() {
        use airhand_link::transport::{FromHost, HostReceiver, open_session};
        use airhand_proto::framing::read_packet;
        use airhand_proto::media::{self, AudioHeader};
        use airhand_proto::session::{ByeReason, HostMsg};
        let (_broker, cfg) = start().await;
        let (mut host, id) = register(&cfg, None).await;
        let mut ctrl = connect_signal(&cfg, Role::Controller).await.unwrap();
        let grant = ask(&mut ctrl, 1, id.code, None).await.unwrap();
        assert!(matches!(next(&mut host).await, ServerMsg::Incoming { .. }));
        let h = open_session(&host.conn, grant.session_id, Role::Host)
            .await
            .unwrap();
        let c = open_session(&ctrl.conn, grant.session_id, Role::Controller)
            .await
            .unwrap();
        let (mut host_control, _host_control_rx) = h.control;
        let (mut host_audio, _host_audio_rx) = h.audio;
        let (_ctrl_control_tx, mut ctrl_control) = c.control;
        let (_ctrl_audio_tx, mut ctrl_audio) = c.audio;
        let _video = c.video; // deliberately never read it
        let (mut video_tx, _video_rx) = h.video;
        let stalled = tokio::spawn(async move {
            let chunk = vec![0u8; 1024 * 1024];
            for _ in 0..128 {
                video_tx.write_all(&chunk).await.unwrap();
            }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !stalled.is_finished(),
            "video must be blocked by the unread lane"
        );
        write_msg(&mut host_control, &HostMsg::Pong { t: 77 })
            .await
            .unwrap();
        let mut buf = Vec::new();
        let msg: HostMsg = timeout(
            Duration::from_secs(2),
            read_msg(&mut ctrl_control, airhand_proto::MAX_CONTROL_MSG, &mut buf),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(matches!(msg, HostMsg::Pong { t: 77 }));
        let audio = AudioHeader {
            seq: 1,
            start: true,
            capture_us: 0,
        };
        host_audio
            .write_all(&media::frame(&audio.encode(), &[&[1]]))
            .await
            .unwrap();
        let packet = timeout(
            Duration::from_secs(2),
            read_packet(&mut ctrl_audio, media::MAX_AUDIO_PACKET),
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(AudioHeader::decode(&packet), Some(audio));
        stalled.abort();
        let _ = stalled.await;
        // Also verify receiver cancellation does not damage framing, and clean
        // media EOF cannot hide a deliberate terminal control message.
        let grant = ask(&mut ctrl, 2, id.code, None).await.unwrap();
        let _ = next(&mut host).await;
        let h = open_session(&host.conn, grant.session_id, Role::Host)
            .await
            .unwrap();
        let c = open_session(&ctrl.conn, grant.session_id, Role::Controller)
            .await
            .unwrap();
        let (_send, recv) = c.control;
        let mut receiver = HostReceiver::new(recv, c.video, c.audio);
        assert!(
            timeout(Duration::from_millis(10), receiver.recv())
                .await
                .is_err()
        );
        let (mut ctl, _r) = h.control;
        let (mut audio, _r2) = h.audio;
        let (mut video, _r3) = h.video;
        write_msg(&mut ctl, &HostMsg::Bye(ByeReason::Kicked))
            .await
            .unwrap();
        ctl.finish().unwrap();
        audio.finish().unwrap();
        video.finish().unwrap();
        assert!(matches!(
            timeout(Duration::from_secs(2), receiver.recv())
                .await
                .unwrap()
                .unwrap(),
            FromHost::Control(HostMsg::Bye(ByeReason::Kicked))
        ));
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
        let code = id.code.to_string();
        let connect = |resume| {
            svc.connect(
                &code,
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

    /// 固定的控制码：服务器重启（只剩数据库里的绑定）之后，只有原来的被控端能取回，别人拿不走；
    /// 被控端放弃（Release）后解除绑定，也不再保留
    #[tokio::test]
    async fn fixed_codes_are_never_handed_out() {
        // 放在 target/ 里：两个服务器的后台任务在测试结束前一直开着数据库，Windows 上删不掉
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../target/test-tmp")
            .join(format!("broker-{:016x}", rand::random::<u64>()));
        let file = dir.join("airhand.db");
        let (old, cfg) = start_with(Some(file.clone())).await;
        let (host, id) = register_with(&cfg, None, true).await;
        host.conn.close(VarInt::from_u32(0), b"");
        wait_until("the host is gone", || old.stats().0 == 0).await;
        old.flush_bindings().await;
        drop((host, old));

        let (broker, cfg) = start_with(Some(file.clone())).await;
        assert_eq!(broker.stats().2, 1);
        // 冒领：知道控制码、不知道密钥
        let thief = Reclaim {
            code: id.code,
            secret: [0xEE; 16],
        };
        let (_thief, got) = register(&cfg, Some(thief)).await;
        assert_ne!(got.code, id.code);
        // 原来的被控端
        let (mut host, again) = register_with(&cfg, Some(id), true).await;
        assert_eq!(again, id);

        // 放弃控制码：解除绑定，断开后不保留
        write_msg(&mut host.send, &ClientMsg::Release)
            .await
            .unwrap();
        wait_until("the binding is removed", || broker.stats().2 == 0).await;
        host.conn.close(VarInt::from_u32(0), b"");
        wait_until("the host is gone", || broker.stats().0 == 1).await;
        assert!(!broker.reserved.lock().unwrap().contains_key(&id.code));
        // Persistence is asynchronous; await the disk barrier outside broker locks.
        broker.flush_bindings().await;
        assert!(!Bindings::open(Some(&file)).contains(id.code));
        let _ = std::fs::remove_dir_all(dir);
    }
}
