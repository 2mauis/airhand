//! Single-pair macOS MVP. There is no AirHand broker in this service.
mod files;
pub mod helper;
mod keychain;
#[cfg(target_os = "macos")]
mod macos;
mod tasks;
#[cfg(target_os = "macos")]
pub use macos::observe_sleep;

use airhand_link::{
    controller::{ControllerService, DirectSession, Frontend, SessionInfo, SessionUi},
    direct::{
        Secret,
        pairing::{self, DirectHello, DirectReady, Invitation, PendingRequest, Request},
        tls::{self, Identity},
    },
    host::HostSession,
    transport,
};
use airhand_proto::{
    framing::{read_msg, write_msg},
    session::{ByeReason, QualitySettings},
};
use anyhow::{Result, anyhow, ensure};
use helper::Helper;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tasks::OwnedTask;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{oneshot, watch};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn program() -> Result<PathBuf> {
    let executable = std::env::current_exe().map_err(|_| anyhow!("helper unavailable"))?;
    let path = executable
        .parent()
        .ok_or_else(|| anyhow!("helper unavailable"))?
        .join("airhand-p2p");
    if path.is_file() {
        return Ok(path);
    }
    #[cfg(debug_assertions)]
    {
        let arch = if cfg!(target_arch = "aarch64") {
            "aarch64"
        } else {
            "x86_64"
        };
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(format!("airhand-p2p-{arch}-apple-darwin"));
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(anyhow!("signed helper sidecar is missing"))
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    role: String,
    pending: PendingRequest,
    request: Vec<u8>,
    invitation: Option<Invitation>,
    invitation_bytes: Vec<u8>,
    node_seed: [u8; 32],
    tls_seed: [u8; 32],
    certificate: Vec<u8>,
    nc: [u8; 32],
    psk: String,
    paired: bool,
}
impl Record {
    fn identity(&self) -> Identity {
        Identity::from_ed25519_seed(self.certificate.clone(), &Secret::new(self.tls_seed))
    }
    fn id(&self) -> String {
        hex(&self.pending.request.req_id)
    }
    fn configuration(
        &self,
        mode: &str,
        bridge: u16,
        quic: u16,
    ) -> Result<Secret<serde_json::Value>> {
        let invitation = self
            .invitation
            .as_ref()
            .ok_or_else(|| anyhow!("invitation required"))?;
        Ok(Secret::new(
            serde_json::json!({"cmd":"start","mode":mode,"node_private":format!("privkey:{}",hex(&self.node_seed)),"psk":self.psk,"address":invitation.tailcat_addr,"client_key":self.pending.request.ctrl_node_pub,"bridge_port":bridge,"quic_port":quic}),
        ))
    }
}
fn default_remember() -> bool {
    true
}
#[derive(Clone, Serialize, Deserialize)]
struct Database {
    record: Option<Record>,
    used: Vec<([u8; 16], u64)>,
    #[serde(default = "default_remember")]
    remember: bool,
}
impl Default for Database {
    fn default() -> Self {
        Self {
            record: None,
            used: Vec::new(),
            remember: true,
        }
    }
}

// Serialize read/modify/persist/cache commit as one operation. In particular,
// an old confirmation must never overwrite a later pairing removal.
fn change_database(
    cache: &Mutex<Option<Secret<Database>>>,
    edit: impl FnOnce(&mut Database) -> Result<()>,
    persist: impl FnOnce(&Secret<Vec<u8>>) -> Result<()>,
) -> Result<()> {
    let mut cache = cache.lock().unwrap();
    let mut database = cache
        .as_ref()
        .ok_or_else(|| anyhow!("pairing store not loaded"))?
        .expose()
        .clone();
    edit(&mut database)?;
    let mut persisted = database.clone();
    if !persisted.remember {
        persisted.record = None;
    }
    let bytes =
        Secret::new(serde_json::to_vec(&persisted).map_err(|_| anyhow!("pairing record failed"))?);
    persist(&bytes)?;
    *cache = Some(Secret::new(database));
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    pub token: u64,
    pub kind: String,
    pub name: String,
    pub summary: String,
    pub sas: Option<String>,
    pub expires: u64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub counters: std::collections::BTreeMap<String, u64>,
    pub remember: bool,
    pub role: Option<String>,
    pub paired: bool,
    pub has_invitation: bool,
    pub listening: bool,
    pub active: bool,
    pub path: String,
    pub prompt: Option<Prompt>,
    pub error: Option<String>,
}
struct Decision {
    allowed: bool,
    input: bool,
}
struct PendingPrompt {
    view: Prompt,
    answer: oneshot::Sender<Decision>,
}

pub struct Service {
    counters: Mutex<Option<helper::Counters>>,
    generation: AtomicU64,
    actions: tokio::sync::Mutex<()>,
    database: Mutex<Option<Secret<Database>>>,
    prompt: Mutex<Option<PendingPrompt>>,
    pub listening: AtomicBool,
    active: AtomicBool,
    phase: Mutex<String>,
    error: Mutex<Option<String>>,
    host_task: Mutex<Option<OwnedTask<()>>>,
    controller_tasks: Mutex<Vec<OwnedTask<()>>>,
    stop: watch::Sender<ByeReason>,
    controller: ControllerService,
}
impl Service {
    pub fn new() -> Arc<Self> {
        crate::host::clock::init();
        let (stop, _) = watch::channel(ByeReason::Kicked);
        Arc::new(Self {
            counters: Mutex::default(),
            generation: AtomicU64::new(0),
            actions: tokio::sync::Mutex::new(()),
            database: Mutex::default(),
            prompt: Mutex::default(),
            listening: AtomicBool::new(false),
            active: AtomicBool::new(false),
            phase: Mutex::new("none".into()),
            error: Mutex::default(),
            host_task: Mutex::default(),
            controller_tasks: Mutex::default(),
            stop,
            controller: ControllerService::start_direct(
                tauri::async_runtime::handle().inner().clone(),
            ),
        })
    }
    fn load(&self) -> Result<Secret<Database>> {
        let mut cache = self.database.lock().unwrap();
        if let Some(data) = &*cache {
            return Ok(data.clone());
        }
        let database = match keychain::read()? {
            Some(bytes) => serde_json::from_slice(bytes.expose())
                .map_err(|_| anyhow!("invalid Keychain pairing record"))?,
            None => Database::default(),
        };
        *cache = Some(Secret::new(database));
        Ok(cache.as_ref().unwrap().clone())
    }
    fn edit(&self, edit: impl FnOnce(&mut Database) -> Result<()>) -> Result<()> {
        self.load()?;
        change_database(&self.database, edit, keychain::write)
    }
    fn record(&self) -> Result<Secret<Record>> {
        self.load()?
            .expose()
            .record
            .clone()
            .map(Secret::new)
            .ok_or_else(|| anyhow!("pair a device first"))
    }
    fn notify(&self, app: &AppHandle) {
        let _ = app.emit("direct-status", ());
        crate::tray::refresh(app, &app.state::<crate::AppState>().hub.borrow());
    }
    fn phase(&self, app: &AppHandle, phase: &str) {
        *self.phase.lock().unwrap() = phase.into();
        self.notify(app)
    }
    pub fn view(&self) -> View {
        let loaded = self.load();
        let record = loaded
            .as_ref()
            .ok()
            .and_then(|d| d.expose().record.as_ref());
        View {
            counters: self
                .counters
                .lock()
                .unwrap()
                .as_ref()
                .map(|c| c.lock().unwrap().clone())
                .unwrap_or_default(),
            remember: loaded.as_ref().is_ok_and(|d| d.expose().remember),
            role: record.map(|r| r.role.clone()),
            paired: record.is_some_and(|r| r.paired),
            has_invitation: record.is_some_and(|r| r.invitation.is_some()),
            listening: self.listening.load(Ordering::Acquire),
            active: self.active.load(Ordering::Acquire),
            path: self.phase.lock().unwrap().clone(),
            prompt: self.prompt.lock().unwrap().as_ref().map(|p| p.view.clone()),
            error: loaded
                .err()
                .map(|_| "Keychain unavailable".into())
                .or_else(|| self.error.lock().unwrap().clone()),
        }
    }
    pub fn decide(&self, token: u64, allowed: bool, input: bool) -> Result<()> {
        let mut prompt = self.prompt.lock().unwrap();
        ensure!(
            prompt
                .as_ref()
                .is_some_and(|p| p.view.token == token && p.view.expires > now()),
            "request expired"
        );
        if let Some(prompt) = prompt.take() {
            let _ = prompt.answer.send(Decision {
                allowed,
                input: allowed && input,
            });
        }
        Ok(())
    }
    async fn ask(
        &self,
        app: &AppHandle,
        kind: &str,
        name: String,
        summary: String,
        sas: Option<String>,
    ) -> Result<Decision> {
        let (tx, rx) = oneshot::channel();
        let token = u64::from(rand::random::<u32>());
        {
            let mut pending = self.prompt.lock().unwrap();
            ensure!(pending.is_none(), "another request is pending");
            *pending = Some(PendingPrompt {
                view: Prompt {
                    token,
                    kind: kind.into(),
                    name,
                    summary,
                    sas,
                    expires: now() + 30,
                },
                answer: tx,
            });
        }
        crate::open_console(app);
        self.notify(app);
        let decision = tokio::time::timeout(Duration::from_secs(30), rx)
            .await
            .ok()
            .and_then(|r| r.ok());
        {
            let mut prompt = self.prompt.lock().unwrap();
            if prompt.as_ref().is_some_and(|p| p.view.token == token) {
                prompt.take();
            }
        }
        self.notify(app);
        decision
            .filter(|d| d.allowed)
            .ok_or_else(|| anyhow!("request declined or timed out"))
    }
    async fn prepare(
        mode: &str,
        node_seed: [u8; 32],
        tls_seed: [u8; 32],
        psk: &str,
    ) -> Result<helper::Reply> {
        let mut helper = Helper::spawn(&program()?)?;
        let mut config = serde_json::json!({"cmd":"prepare","mode":mode,"node_private":format!("privkey:{}",hex(&node_seed)),"tls_seed":tls_seed});
        if mode == "host" {
            config["psk"] = psk.into()
        }
        helper.send(&Secret::new(config))?;
        helper.prepared().await
    }
    pub async fn export_request(&self, app: &AppHandle) -> Result<()> {
        let generation = self.generation.load(Ordering::Acquire);
        ensure!(
            self.load()?.expose().record.is_none(),
            "forget the current pair before pairing again"
        );
        let node_seed = rand::random();
        let tls_seed = rand::random();
        let nc: [u8; 32] = rand::random();
        let prepared = Self::prepare("controller", node_seed, tls_seed, "").await?;
        let certificate = prepared.certificate;
        let request = Request {
            v: 1,
            ctrl_node_pub: prepared.public,
            ctrl_tls_fp: pairing::hash(&certificate),
            commitment: pairing::hash(&nc),
            req_id: rand::random(),
            expires: now() + pairing::TTL,
            name: "AirHand controller".into(),
        };
        request.validate(now())?;
        let bytes = serde_json::to_vec(&request)?;
        let Some(path) = files::choose(app, true).await? else {
            return Ok(());
        };
        files::write(&path, &bytes)?;
        ensure!(
            self.generation.load(Ordering::Acquire) == generation,
            "pairing cancelled"
        );
        self.edit(|database| {
            ensure!(
                self.generation.load(Ordering::Acquire) == generation,
                "pairing cancelled"
            );
            ensure!(database.record.is_none(), "pairing changed");
            database.record = Some(Record {
                role: "controller".into(),
                pending: PendingRequest {
                    request,
                    consumed: false,
                },
                request: bytes,
                invitation: None,
                invitation_bytes: Vec::new(),
                node_seed,
                tls_seed,
                certificate,
                nc,
                psk: String::new(),
                paired: false,
            });
            Ok(())
        })?;
        files::share(app, path).await?;
        self.notify(app);
        Ok(())
    }
    pub async fn import_request(&self, app: &AppHandle) -> Result<()> {
        let generation = self.generation.load(Ordering::Acquire);
        ensure!(
            self.load()?.expose().record.is_none(),
            "forget the current pair before pairing again"
        );
        let Some(path) = files::choose(app, false).await? else {
            return Ok(());
        };
        let bytes = files::read(&path)?;
        let request: Request =
            serde_json::from_slice(&bytes).map_err(|_| anyhow!("invalid request file"))?;
        request.validate(now())?;
        ensure!(
            !self
                .load()?
                .expose()
                .used
                .iter()
                .any(|(id, _)| *id == request.req_id),
            "request already used"
        );
        self.ask(
            app,
            "pair",
            request.name.clone(),
            format!(
                "Untrusted device label. Identity {}",
                hex(&request.ctrl_tls_fp[..6])
            ),
            None,
        )
        .await?;
        request.validate(now())?;
        let node_seed = rand::random();
        let tls_seed = rand::random();
        let psk = format!("psk:{}", hex(&rand::random::<[u8; 32]>()));
        let prepared = Self::prepare("host", node_seed, tls_seed, &psk).await?;
        let certificate = prepared.certificate;
        let invitation = Invitation {
            v: 1,
            tailcat_addr: prepared.address,
            host_tls_fp: pairing::hash(&certificate),
            nh: rand::random(),
            request_hash: pairing::hash(&bytes),
            expires: request.expires,
        };
        invitation.validate(&request, &bytes, now())?;
        let invite_bytes = serde_json::to_vec(&invitation)?;
        let Some(path) = files::choose(app, true).await? else {
            return Ok(());
        };
        ensure!(
            self.generation.load(Ordering::Acquire) == generation,
            "pairing cancelled"
        );
        self.edit(|database| {
            ensure!(
                self.generation.load(Ordering::Acquire) == generation,
                "pairing cancelled"
            );
            ensure!(database.record.is_none(), "pairing changed");
            database.used.retain(|(_, expires)| *expires > now());
            database.used.push((request.req_id, request.expires));
            database.record = Some(Record {
                role: "host".into(),
                pending: PendingRequest {
                    request,
                    consumed: true,
                },
                request: bytes,
                invitation: Some(invitation),
                invitation_bytes: invite_bytes.clone(),
                node_seed,
                tls_seed,
                certificate,
                nc: [0; 32],
                psk,
                paired: false,
            });
            Ok(())
        })?;
        files::write(&path, &invite_bytes)?;
        files::share(app, path).await?;
        self.notify(app);
        Ok(())
    }
    pub async fn import_invitation(&self, app: &AppHandle) -> Result<()> {
        let Some(path) = files::choose(app, false).await? else {
            return Ok(());
        };
        let bytes = Secret::new(files::read(&path)?);
        let invitation: Invitation = serde_json::from_slice(bytes.expose())
            .map_err(|_| anyhow!("invalid invitation file"))?;
        self.edit(|database| {
            let record = database
                .record
                .as_mut()
                .ok_or_else(|| anyhow!("no pending request"))?;
            ensure!(record.role == "controller", "no pending controller request");
            record
                .pending
                .consume(&invitation, &record.request, now())?;
            record.invitation = Some(invitation);
            record.invitation_bytes = bytes.expose().clone();
            Ok(())
        })?;
        std::fs::remove_file(path)
            .map_err(|_| anyhow!("invitation imported; remove its secret file using Finder"))?;
        self.notify(app);
        Ok(())
    }
    pub fn disconnect(&self, app: &AppHandle) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.stop.send_replace(ByeReason::Kicked);
        self.prompt.lock().unwrap().take();
        self.controller_tasks.lock().unwrap().clear();
        self.phase(app, "lost");
    }
    pub fn stop(&self, app: &AppHandle) {
        self.listening.store(false, Ordering::Release);
        self.disconnect(app);
        self.host_task.lock().unwrap().take();
        self.active.store(false, Ordering::Release);
        self.notify(app);
    }
    pub fn forget(&self, app: &AppHandle) -> Result<()> {
        self.stop(app);
        self.edit(|database| {
            database.record = None;
            Ok(())
        })?;
        self.phase(app, "none");
        Ok(())
    }
    pub fn allow(self: &Arc<Self>, app: &AppHandle, on: bool) -> Result<()> {
        self.stop(app);
        if !on {
            return Ok(());
        }
        let record = self.record()?;
        ensure!(
            record.expose().role == "host" && record.expose().invitation.is_some(),
            "import a pairing request first"
        );
        self.listening.store(true, Ordering::Release);
        let service = self.clone();
        let app = app.clone();
        let task = OwnedTask::spawn(async move {
            service.listen(app).await;
        });
        *self.host_task.lock().unwrap() = Some(task);
        Ok(())
    }
}

struct Active<'a>(&'a AtomicBool);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
struct CloseConnection(quinn::Connection);
impl Drop for CloseConnection {
    fn drop(&mut self) {
        self.0.close(0u32.into(), b"direct session ended");
    }
}
fn loopback_port() -> Result<u16> {
    Ok(std::net::UdpSocket::bind("127.0.0.1:0")
        .map_err(|_| anyhow!("loopback port unavailable"))?
        .local_addr()?
        .port())
}
fn watchdog(
    conn: quinn::Connection,
    mut helper_path: watch::Receiver<bool>,
) -> (watch::Receiver<bool>, OwnedTask<()>) {
    let (tx, rx) = watch::channel(true);
    let task = OwnedTask::spawn(async move {
        let mut probe = airhand_link::direct::probe::ProbeSource::start(conn.clone());
        let mut last = std::time::Instant::now();
        let mut previous = last;
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tokio::select! {biased;
             _=conn.closed()=>break,
             _=tick.tick()=>{let now=std::time::Instant::now();if now.duration_since(previous)>Duration::from_millis(1500)||last.elapsed()>Duration::from_millis(1100){break};previous=now;},
             changed=helper_path.changed()=>{if changed.is_err()||!*helper_path.borrow(){break}},
             progress=probe.progress.recv()=>{if progress.is_none(){break};last=std::time::Instant::now()},
            }
        }
        tx.send_replace(false);
        conn.close(0u32.into(), b"direct path lost");
    });
    (rx, task)
}
impl Service {
    async fn listen(self: Arc<Self>, app: AppHandle) {
        while self.listening.load(Ordering::Acquire) {
            self.phase(&app, "bootstrap");
            let result = self.host_once(&app).await;
            self.prompt.lock().unwrap().take();
            self.active.store(false, Ordering::Release);
            if result.is_err() {
                *self.error.lock().unwrap() = Some(
                    "Direct listener ended; verify pairing, permissions and the signed sidecar"
                        .into(),
                );
                self.listening.store(false, Ordering::Release);
            }
            self.phase(&app, "lost");
            if self.record().is_err() {
                self.listening.store(false, Ordering::Release);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        self.notify(&app);
    }
    async fn host_once(self: &Arc<Self>, app: &AppHandle) -> Result<()> {
        let record = self.record()?;
        let record = record.expose();
        ensure!(record.role == "host", "host pair required");
        let endpoint = quinn::Endpoint::server(
            tls::server_config(&record.identity(), record.pending.request.ctrl_tls_fp)?,
            "127.0.0.1:0".parse().unwrap(),
        )
        .map_err(|_| anyhow!("direct endpoint unavailable"))?;
        let bridge_port = loopback_port()?;
        let tuple = std::net::SocketAddr::from(([127, 0, 0, 1], bridge_port));
        let mut helper = Helper::spawn(&program()?)?;
        *self.counters.lock().unwrap() = Some(helper.counters.clone());
        helper.send(&record.configuration(
            "host",
            bridge_port,
            endpoint.local_addr()?.port(),
        )?)?;
        let mut path = helper.path.clone();
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {biased;
             changed=path.changed()=>{if changed.is_err(){return Err(anyhow!("helper ended"))}
                if *path.borrow(){self.phase(app,"direct")}else if self.active.load(Ordering::Acquire){return Err(anyhow!("direct path lost"))}},
             Some(_)=tasks.join_next()=>return Ok(()),
             incoming=endpoint.accept()=>{
              let incoming=incoming.ok_or_else(||anyhow!("direct listener ended"))?;
              if incoming.remote_address()!=tuple || self.active.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){incoming.refuse();continue}
              let service=self.clone();let app=app.clone();let record=Secret::new(record.clone());let path=path.clone();
              tasks.spawn(async move{let _active=Active(&service.active);let result=async{
               let conn=tokio::time::timeout(Duration::from_secs(8),incoming).await.map_err(|_|anyhow!("direct authentication timed out"))?.map_err(|_|anyhow!("direct authentication failed"))?;
               let _close=CloseConnection(conn.clone());ensure!(*path.borrow(),"direct path required");
               let(path,_watchdog)=watchdog(conn.clone(),path);
               service.host_session(&app,record.expose(),conn,path).await
              }.await;if result.is_err(){service.phase(&app,"lost");}});
             }
            }
        }
    }
    async fn host_session(
        &self,
        app: &AppHandle,
        record: &Record,
        conn: quinn::Connection,
        path: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut stop = self.stop.subscribe();
        stop.borrow_and_update();
        let mut session = transport::accept_session(&conn).await?;
        let hello = tokio::time::timeout(
            Duration::from_secs(10),
            read_msg::<_, DirectHello>(&mut session.streams.control.1, 1024, &mut Vec::new()),
        )
        .await
        .map_err(|_| anyhow!("direct reveal timed out"))??
        .ok_or_else(|| anyhow!("direct reveal missing"))?;
        ensure!(hello.v == pairing::VERSION, "direct protocol mismatch");
        record
            .pending
            .request
            .verify_reveal(&Secret::new(hello.nc))?;
        if !record.paired {
            let invitation = record
                .invitation
                .as_ref()
                .ok_or_else(|| anyhow!("invitation missing"))?;
            invitation.validate(&record.pending.request, &record.request, now())?;
            let sas = pairing::sas(
                &Secret::new(hello.nc),
                &invitation.nh,
                &record.request,
                &record.invitation_bytes,
            );
            if self
                .ask(
                    app,
                    "sas",
                    record.pending.request.name.clone(),
                    "Compare this code on both Macs. A mismatch must be rejected.".into(),
                    Some(sas),
                )
                .await
                .is_err()
            {
                self.edit(|database| {
                    if database
                        .record
                        .as_ref()
                        .is_some_and(|current| current.id() == record.id())
                    {
                        database.record = None;
                    }
                    Ok(())
                })?;
                return Err(anyhow!("SAS confirmation declined"));
            }
            invitation.validate(&record.pending.request, &record.request, now())?;
            ensure!(!stop.has_changed().unwrap_or(true), "session revoked");
            self.edit(|database| {
                let current = database
                    .record
                    .as_mut()
                    .ok_or_else(|| anyhow!("pair removed"))?;
                ensure!(current.id() == record.id(), "pair changed");
                current.paired = true;
                current.nc = hello.nc;
                Ok(())
            })?;
        }
        let decision=self.ask(app,"session",record.pending.request.name.clone(),"Allow this session? Screen access and optional keyboard/mouse control end when disconnected.".into(),None).await?;
        ensure!(
            *path.borrow() && !stop.has_changed().unwrap_or(true),
            "direct path lost or session revoked"
        );
        write_msg(
            &mut session.streams.control.0,
            &DirectReady {
                v: pairing::VERSION,
                input: decision.input,
            },
        )
        .await?;
        let id = session.session_id;
        let io = HostSession::from_accepted(session, 2, crate::host::audio::QUEUE)?;
        self.phase(app, "direct");
        crate::host::session::run_direct(conn, id, stop, io, decision.input, path).await
    }
    pub async fn connect(
        self: &Arc<Self>,
        app: &AppHandle,
        settings: QualitySettings,
        ui: Arc<dyn SessionUi>,
        frontend: Frontend,
    ) -> Result<SessionInfo> {
        let mut stop = self.stop.subscribe();
        stop.borrow_and_update();
        tokio::select! {biased;_=stop.changed()=>Err(anyhow!("session revoked")),result=self.connect_once(app,settings,ui,frontend)=>result}
    }
    async fn connect_once(
        self: &Arc<Self>,
        app: &AppHandle,
        settings: QualitySettings,
        ui: Arc<dyn SessionUi>,
        frontend: Frontend,
    ) -> Result<SessionInfo> {
        let mut stop = self.stop.subscribe();
        stop.borrow_and_update();
        ensure!(
            self.active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "another session is in progress"
        );
        let active = Active(&self.active);
        let record = self.record()?;
        let record = record.expose();
        ensure!(
            record.role == "controller" && record.pending.consumed,
            "import the invitation first"
        );
        let invitation = record
            .invitation
            .as_ref()
            .ok_or_else(|| anyhow!("invitation required"))?;
        if !record.paired {
            invitation.validate(&record.pending.request, &record.request, now())?
        }
        self.phase(app, "bootstrap");
        let mut helper = Helper::spawn(&program()?)?;
        *self.counters.lock().unwrap() = Some(helper.counters.clone());
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
            .map_err(|_| anyhow!("direct endpoint unavailable"))?;
        endpoint.set_default_client_config(tls::client_config(
            &record.identity(),
            invitation.host_tls_fp,
        )?);
        let bridge_port = loopback_port()?;
        helper.send(&record.configuration(
            "controller",
            bridge_port,
            endpoint.local_addr()?.port(),
        )?)?;
        helper.wait_direct().await?;
        let conn = tokio::time::timeout(
            Duration::from_secs(10),
            endpoint.connect(
                std::net::SocketAddr::from(([127, 0, 0, 1], bridge_port)),
                "airhand",
            )?,
        )
        .await
        .map_err(|_| anyhow!("direct QUIC timed out"))?
        .map_err(|_| anyhow!("direct authentication failed"))?;
        let close = CloseConnection(conn.clone());
        let (path, watchdog) = watchdog(conn.clone(), helper.path.clone());
        let sas = (!record.paired).then(|| {
            pairing::sas(
                &Secret::new(record.nc),
                &invitation.nh,
                &record.request,
                &record.invitation_bytes,
            )
        });
        if let Some(sas) = &sas {
            let _ = app.emit("direct-sas", sas);
        }
        let info = self
            .controller
            .connect_direct(
                &conn,
                DirectSession {
                    path: path.clone(),
                    nonce: Secret::new(record.nc),
                    sas,
                },
                settings,
                ui,
                frontend,
            )
            .await
            .map_err(|_| anyhow!("direct session declined or unavailable"))?;
        self.edit(|database| {
            let current = database
                .record
                .as_mut()
                .ok_or_else(|| anyhow!("pair removed"))?;
            ensure!(current.id() == record.id(), "pair changed");
            current.paired = true;
            Ok(())
        })?;
        self.phase(app, "direct");
        // The session owner outlives this command and owns all process/network guards.
        ensure!(!stop.has_changed().unwrap_or(true), "session revoked");
        std::mem::forget(active);
        let service = self.clone();
        let app = app.clone();
        let task = OwnedTask::spawn(async move {
            let _active = Active(&service.active);
            let _resources = (helper, endpoint, watchdog, close);
            tokio::select! {_=conn.closed()=>{},_=stop.changed()=>{conn.close(0u32.into(),b"local revocation");}}
            service.phase(&app, "lost");
        });
        let mut tasks = self.controller_tasks.lock().unwrap();
        tasks.retain(|t| !t.is_finished());
        tasks.push(task);
        Ok(info)
    }
    pub fn command(
        &self,
        command: airhand_link::controller::Command,
    ) -> std::result::Result<(), airhand_link::Problem> {
        self.controller.command(command)
    }
}

#[tauri::command]
pub fn direct_status(state: tauri::State<'_, crate::AppState>) -> View {
    state.direct.view()
}
#[tauri::command]
pub fn direct_decide(
    state: tauri::State<'_, crate::AppState>,
    token: u64,
    allowed: bool,
    input: bool,
) -> std::result::Result<(), String> {
    state
        .direct
        .decide(token, allowed, input)
        .map_err(|_| "Request expired".into())
}
#[tauri::command]
pub async fn direct_action(
    app: AppHandle,
    state: tauri::State<'_, crate::AppState>,
    action: String,
) -> std::result::Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err("Tailcat desktop MVP requires macOS".into());
    }
    let service = state.direct.clone();
    let _permit = if matches!(
        action.as_str(),
        "request" | "importRequest" | "importInvitation" | "remember"
    ) {
        Some(service.actions.lock().await)
    } else {
        None
    };
    let result = match action.as_str() {
        "remember" => service.edit(|database| {
            database.remember = !database.remember;
            Ok(())
        }),
        "request" => service.export_request(&app).await,
        "importRequest" => service.import_request(&app).await,
        "importInvitation" => service.import_invitation(&app).await,
        "allow" => service.allow(&app, true),
        "stop" => {
            service.stop(&app);
            Ok(())
        }
        "disconnect" => {
            service.disconnect(&app);
            Ok(())
        }
        "forget" => service.forget(&app),
        _ => Err(anyhow!("unknown pairing action")),
    };
    if let Err(error) = result {
        *service.error.lock().unwrap() = Some(error.to_string());
        service.notify(&app);
        return Err(error.to_string());
    }
    *service.error.lock().unwrap() = None;
    service.notify(&app);
    Ok(())
}

#[cfg(test)]
mod store_tests {
    use super::*;
    #[test]
    fn concurrent_store_changes_are_serialized_and_failed_writes_do_not_commit() {
        let cache = Arc::new(Mutex::new(Some(Secret::new(Database::default()))));
        let persisted = Arc::new(Mutex::new(Vec::new()));
        std::thread::scope(|scope| {
            for index in 1..=8 {
                let cache = cache.clone();
                let persisted = persisted.clone();
                scope.spawn(move || {
                    change_database(
                        &cache,
                        |database| {
                            database.used.push(([index; 16], 600));
                            Ok(())
                        },
                        |bytes| {
                            *persisted.lock().unwrap() = bytes.expose().clone();
                            Ok(())
                        },
                    )
                    .unwrap()
                });
            }
        });
        let stored: Database = serde_json::from_slice(&persisted.lock().unwrap()).unwrap();
        assert_eq!(stored.used.len(), 8);
        assert_eq!(
            cache.lock().unwrap().as_ref().unwrap().expose().used.len(),
            8
        );
        assert!(
            change_database(
                &cache,
                |database| {
                    database.used.clear();
                    Ok(())
                },
                |_| Err(anyhow!("test persistence failure"))
            )
            .is_err()
        );
        assert_eq!(
            cache.lock().unwrap().as_ref().unwrap().expose().used.len(),
            8
        );
    }
}
