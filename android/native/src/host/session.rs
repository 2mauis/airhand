//! 被控端的单个远控会话（对应桌面版的 `client/src-tauri/src/host/session.rs`）。
//!
//! 手机默认要先请坐在它面前的人同意（见 `airhand_proto::session` 的模块说明）：
//!
//! ```text
//! 等控制端的 Hello（最多 2 秒）─┬─ 本次共享里同意过这台控制端 ──────────────┐
//!                              └─ AwaitingApproval → 问本机用户（最多 60 秒）─┼─ 同意 → Hello、画面……
//!                                                                          └─ 拒绝 / 没人理 → Bye
//! ```
//!
//! 同意之前只回应 Ping、记下画质和声音设置，不发画面、不执行任何输入。

use std::sync::Arc;
use std::time::{Duration, Instant};

use airhand_link::tasks::OwnedTask;
use airhand_link::transport::{SessionStreams, open_session, write_lane};
use airhand_proto::framing::{self, read_msg};
use airhand_proto::session::{
    ByeReason, ControllerMsg, HostHello, HostMsg, Notice, Platform, QualitySettings,
};
use airhand_proto::{MAX_SESSION_MSG, Role};
use anyhow::{Result, anyhow};
use bytes::Bytes;
use quinn::Connection;
use serde_json::json;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{info, warn};

use super::{Shared, audio, video};

/// 控制端每秒都会发 Ping，这么久没有任何消息说明对方已失联
const PEER_TIMEOUT: Duration = Duration::from_secs(30);
/// 等控制端报上自己的标识（它是会话流上的第一条消息，通常马上就到）
const HELLO_WAIT: Duration = Duration::from_secs(2);
/// 本机用户这么久没有回应连接请求就当作拒绝
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

/// 本机用户同意之前会话所处的阶段
enum Gate {
    /// 等控制端的 Hello，好知道是不是同意过的设备
    WaitHello,
    /// 正在问本机用户
    Asking(oneshot::Receiver<bool>),
    /// 已同意（或者不需要同意）
    Open,
}

async fn decided(gate: &mut Gate) -> bool {
    match gate {
        Gate::Asking(rx) => rx.await.unwrap_or(false),
        _ => std::future::pending().await,
    }
}

async fn until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}

/// The parent JoinSet may cancel us at any await (network loss/new code/stop).
/// Cleanup must therefore be owned, not just statements after the main loop.
struct SessionGuard {
    shared: Arc<Shared>,
    id: u64,
}
impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.shared.take_pending(self.id);
        self.shared.video.remove(self.id);
        self.shared.audio.unsubscribe(self.id);
        self.shared
            .event(json!({ "type": "input", "events": ["ReleaseAll"] }));
    }
}

pub async fn run(
    conn: Connection,
    session_id: u64,
    mut kick: watch::Receiver<ByeReason>,
    shared: Arc<Shared>,
) -> Result<()> {
    let guard = SessionGuard {
        shared: shared.clone(),
        id: session_id,
    };
    let SessionStreams {
        control: (send, mut recv),
        video: (video_send, _video_recv),
        audio: (audio_send, _audio_recv),
    } = open_session(&conn, session_id, Role::Host).await?;

    // Independent bounded queues and streams, shared with the desktop transport.
    let (ctl_tx, ctl_rx) = mpsc::channel::<Bytes>(32);
    let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(audio::QUEUE);
    let (video_tx, video_rx) = mpsc::channel::<Bytes>(video::QUEUE);
    let mut video_tx = Some(video_tx);
    let mut bcast = shared.broadcast.subscribe();
    let ctl = |m: &HostMsg| -> Result<()> {
        ctl_tx
            .try_send(Bytes::from(framing::encode(m)?))
            .map_err(|_| anyhow!("the session is closed"))
    };

    let mut writer = OwnedTask(tokio::spawn(async move {
        tokio::try_join!(
            write_lane(send, ctl_rx),
            write_lane(video_send, video_rx),
            write_lane(audio_send, audio_rx)
        )
    }));

    // 读任务：read_msg 不是取消安全的，单独放在一个任务里
    let (in_tx, mut in_rx) = mpsc::channel::<ControllerMsg>(64);
    let mut reader = OwnedTask(tokio::spawn(async move {
        let mut buf = Vec::with_capacity(1024);
        loop {
            match read_msg::<_, ControllerMsg>(&mut recv, MAX_SESSION_MSG, &mut buf).await {
                Ok(Some(m)) => {
                    if in_tx.send(m).await.is_err() {
                        return Ok(());
                    }
                }
                Ok(None) => return Ok(()),
                Err(e) => return Err(anyhow!(e)),
            }
        }
    }));

    // 同意之前收到的设置，同意之后再生效
    let mut settings: Option<QualitySettings> = None;
    let mut want_audio = false;
    let mut device: Option<[u8; 16]> = None;
    let mut gate = Gate::WaitHello;
    let mut deadline = Some(tokio::time::Instant::now() + HELLO_WAIT);
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut last_msg = Instant::now();

    // 同意之后（或者不需要同意时）：开始正常的会话
    let mut open = |gate: &mut Gate,
                    deadline: &mut Option<tokio::time::Instant>,
                    settings: Option<QualitySettings>,
                    want_audio: bool|
     -> Result<()> {
        *gate = Gate::Open;
        *deadline = None;
        shared.take_pending(session_id);
        shared.update_controller(session_id, |c| {
            c.pending = false;
            c.active = true;
        });
        let info = shared.info.lock().unwrap();
        ctl(&HostMsg::Hello(HostHello {
            hostname: info.hostname.clone(),
            os: info.os.clone(),
            version: info.version.clone(),
            displays: info.displays.clone(),
            current_display: 0,
            codecs: info.codecs.clone(),
            platform: Platform::Android,
            touch: info.touch,
            actions: info.actions.clone(),
        }))?;
        if !info.input {
            ctl(&HostMsg::Notice(Notice::MissingPermissions {
                screen: false,
                input: true,
            }))?;
        }
        if let Some(e) = &info.encoder {
            ctl(&HostMsg::Encoder(e.clone()))?;
        }
        if let Some(detail) = &info.capture_failure {
            ctl(&HostMsg::Notice(Notice::CaptureFailed {
                detail: detail.clone(),
            }))?;
        }
        drop(info);
        if let Some(s) = settings {
            shared.video.set_settings(s);
        }
        if let Some(tx) = video_tx.take() {
            shared.video.add(session_id, tx);
        }
        if want_audio {
            shared.audio.subscribe(session_id, audio_tx.clone());
        }
        info!(session_id, "session started");
        Ok(())
    };

    if !shared.approval_required() {
        open(&mut gate, &mut deadline, None, false)?;
    }

    // 问本机用户：告诉控制端在等，等结果
    let ask = |gate: &mut Gate, deadline: &mut Option<tokio::time::Instant>| -> Result<()> {
        let (tx, rx) = oneshot::channel();
        shared.add_pending(session_id, tx);
        shared.update_controller(session_id, |c| c.pending = true);
        *gate = Gate::Asking(rx);
        *deadline = Some(tokio::time::Instant::now() + APPROVAL_TIMEOUT);
        info!(session_id, "asking the local user to allow the connection");
        ctl(&HostMsg::AwaitingApproval)
    };

    let result: Result<()> = async {
        loop {
            let is_open = matches!(gate, Gate::Open);
            tokio::select! {
                msg = in_rx.recv() => {
                    let Some(msg) = msg else { break };
                    last_msg = Instant::now();
                    match msg {
                        ControllerMsg::Hello(h) => {
                            device = Some(h.device);
                            if matches!(gate, Gate::WaitHello) {
                                if shared.is_trusted(&h.device) {
                                    info!(session_id, "this controller was allowed before during this sharing");
                                    open(&mut gate, &mut deadline, settings.take(), want_audio)?;
                                } else {
                                    ask(&mut gate, &mut deadline)?;
                                }
                            }
                        }
                        ControllerMsg::Ping { t } => ctl(&HostMsg::Pong { t })?,
                        ControllerMsg::Settings(s) if !is_open => settings = Some(s),
                        ControllerMsg::Audio { enabled } if !is_open => want_audio = enabled,
                        // 同意之前不执行任何操作、不发画面
                        _ if !is_open => {}
                        ControllerMsg::Input(events) => {
                            shared.event(json!({ "type": "input", "events": events }));
                        }
                        ControllerMsg::Ack { seq } => shared.video.ack(session_id, seq),
                        ControllerMsg::Settings(s) => shared.video.set_settings(s),
                        ControllerMsg::RequestKeyframe => shared.video.request_keyframe(),
                        // 手机只有一块屏幕
                        ControllerMsg::SelectDisplay { .. } => {}
                        ControllerMsg::Clipboard { text } => {
                            shared.event(json!({ "type": "clipboard", "text": text }));
                        }
                        ControllerMsg::Audio { enabled } => {
                            if enabled {
                                shared.audio.subscribe(session_id, audio_tx.clone());
                            } else {
                                shared.audio.unsubscribe(session_id);
                            }
                        }
                    }
                }
                allow = decided(&mut gate) => {
                    if allow {
                        info!(session_id, "the local user allowed the connection");
                        if let Some(d) = device {
                            shared.trust(d);
                        }
                        open(&mut gate, &mut deadline, settings.take(), want_audio)?;
                    } else {
                        info!(session_id, "the local user declined the connection");
                        ctl(&HostMsg::Bye(ByeReason::Declined))?;
                        break;
                    }
                }
                _ = until(deadline) => match gate {
                    // 控制端没报上标识（不应该发生）：当作没见过的设备
                    Gate::WaitHello => ask(&mut gate, &mut deadline)?,
                    _ => {
                        info!(session_id, "nobody answered the connection request");
                        ctl(&HostMsg::Bye(ByeReason::Unanswered))?;
                        break;
                    }
                },
                _ = kick.changed() => {
                    let reason = *kick.borrow_and_update();
                    info!(session_id, ?reason, "the local user disconnected the controller");
                    ctl(&HostMsg::Bye(reason))?;
                    break;
                }
                b = bcast.recv() => match b {
                    // 同意之前不转发（Hello 里会带上那时的状态）
                    Ok(b) => if is_open {
                        ctl_tx.try_send(b).map_err(|_| anyhow!("session control queue overflow"))?;
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(session_id, n, "missed broadcast messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                r = &mut writer.0 => return Err(anyhow!("session writer ended: {r:?}")),
                _ = watchdog.tick() => {
                    if last_msg.elapsed() > PEER_TIMEOUT {
                        anyhow::bail!("the controller stopped responding");
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    drop(guard);
    drop(video_tx);
    drop(audio_tx);
    drop(ctl_tx);
    reader.abort();
    // 等写任务把已排队的数据（例如 Bye）发完
    if !writer.is_finished() {
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut writer.0).await;
    }
    let result = match result {
        Ok(()) => match (&mut reader.0).await {
            Ok(Err(e)) => Err(e),
            _ => Ok(()),
        },
        Err(e) => Err(e),
    };
    match &result {
        Ok(()) => info!(session_id, "session ended"),
        Err(e) => warn!(session_id, "session failed: {e:#}"),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{Host, HostConfig};
    #[tokio::test]
    async fn cancellation_releases_subscriptions_and_pending_approval() {
        let cfg: HostConfig = serde_json::from_value(json!({
            "server": "127.0.0.1:1", "fingerprint": "", "name": "test", "os": "test",
            "version": "test", "reclaim": null, "displays": [], "codecs": ["H264"],
            "input": false, "approval": true
        }))
        .unwrap();
        let host = Host::start(cfg).unwrap();
        let sh = host.shared.clone();
        let (video_tx, _video_rx) = mpsc::channel(video::QUEUE);
        let (audio_tx, _audio_rx) = mpsc::channel(audio::QUEUE);
        let (approval_tx, approval_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = SessionGuard {
                shared: sh.clone(),
                id: 7,
            };
            sh.video.add(7, video_tx);
            sh.audio.subscribe(7, audio_tx);
            sh.add_pending(7, approval_tx);
            for i in 0..3 {
                sh.video.push(
                    &[1],
                    i == 0,
                    false,
                    0,
                    airhand_proto::session::Codec::AV1,
                    8,
                    8,
                );
            }
            ready_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready_rx.await.unwrap();
        assert!(!host.shared.video.wait_window(Duration::ZERO));
        task.abort();
        let _ = task.await;
        assert!(approval_rx.await.is_err());
        assert!(host.shared.video.wait_window(Duration::ZERO));
        let mut video_off = false;
        let mut audio_off = false;
        while let Some(event) = host.poll_event(Duration::ZERO) {
            let v: serde_json::Value = serde_json::from_str(&event).unwrap();
            if v["active"] == false {
                video_off |= v["type"] == "video";
                audio_off |= v["type"] == "audio";
            }
        }
        assert!(video_off && audio_off);
    }
}
