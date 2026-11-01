//! 被控端的单个远控会话。

use std::time::{Duration, Instant};

use airhand_proto::framing::{self, read_msg};
use airhand_proto::session::{
    ByeReason, ControllerMsg, DisplayInfo, HostHello, HostMsg, Notice, QualitySettings,
};
use airhand_proto::{MAX_SESSION_MSG, Role, SessionHeader};
use anyhow::{Context, Result, anyhow};
use bytes::Bytes;
use quinn::Connection;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

use super::audio::{self, AudioSubscription};
use super::capture;
use super::input::Injector;
use super::pipeline::Pipeline;
use crate::clipboard::{self, ClipSync};
use crate::config;

/// 控制端每秒都会发 Ping，这么久没有任何消息说明对方已失联
const PEER_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn run(
    conn: Connection,
    session_id: u64,
    mut kick: watch::Receiver<ByeReason>,
) -> Result<()> {
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .context("can't open the session stream")?;
    send.write_all(
        &SessionHeader {
            session_id,
            role: Role::Host,
        }
        .encode(),
    )
    .await?;

    let (displays, codecs, notice) = tokio::task::spawn_blocking(|| {
        (
            capture::displays(),
            super::encoder::available_codecs(),
            capture::environment_notice(),
        )
    })
    .await
    .unwrap_or_default();
    let mut current: u8 = 0;
    let (hostname, os) = crate::sysinfo::host_identity();
    info!(session_id, displays = displays.len(), "session started");

    // 视频走有界队列（背压），控制类小消息走优先队列，不会排在视频帧后面；
    // 声音包很小、对卡顿敏感，排在控制消息之后、视频之前，队列满了直接丢
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(2);
    let (ctl_tx, mut ctl_rx) = mpsc::unbounded_channel::<Bytes>();
    let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(audio::QUEUE);
    let ctl = |m: &HostMsg| -> Result<()> {
        ctl_tx
            .send(Bytes::from(framing::encode(m)?))
            .map_err(|_| anyhow!("the session is closed"))
    };
    ctl(&HostMsg::Hello(HostHello {
        hostname,
        os,
        version: config::VERSION.to_string(),
        displays: displays.clone(),
        current_display: current,
        codecs,
    }))?;
    if displays.is_empty() {
        ctl(&HostMsg::Notice(Notice::NoDisplays))?;
    }
    if let Some(notice) = notice {
        ctl(&HostMsg::Notice(notice))?;
    }

    // 写任务：所有发往控制端的数据都经过这里（Bytes 零拷贝交给 QUIC）
    let writer = tokio::spawn(async move {
        loop {
            let buf = tokio::select! {
                biased;
                Some(b) = ctl_rx.recv() => b,
                Some(b) = audio_rx.recv() => b,
                Some(b) = out_rx.recv() => b,
                else => break,
            };
            if send.write_chunk(buf).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });

    // 读任务：read_msg 不是取消安全的，单独放在一个任务里，通过通道交给主循环
    let (in_tx, mut in_rx) = mpsc::channel::<ControllerMsg>(64);
    let reader = tokio::spawn(async move {
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
    });

    let mut settings = QualitySettings::default();
    let mut pipeline =
        (!displays.is_empty()).then(|| Pipeline::start(current, settings, out_tx.clone()));
    let mut injector = Injector::new();
    let mut sound: Option<AudioSubscription> = None;
    let fallback = DisplayInfo {
        index: 0,
        name: String::new(),
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        primary: true,
    };
    let mut clip = ClipSync::new();
    let mut clip_tick = tokio::time::interval(clipboard::POLL);
    clip_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut watchdog = tokio::time::interval(Duration::from_secs(5));
    let mut last_msg = Instant::now();

    let result: Result<()> = async {
        loop {
            tokio::select! {
                msg = in_rx.recv() => {
                    let Some(msg) = msg else { break };
                    last_msg = Instant::now();
                    match msg {
                        ControllerMsg::Input(events) => {
                            let d = displays.get(current as usize).unwrap_or(&fallback);
                            injector.handle(&events, d);
                        }
                        ControllerMsg::Ack { seq } => {
                            if let Some(p) = &pipeline {
                                p.ack(seq);
                            }
                        }
                        ControllerMsg::Settings(s) => {
                            settings = s.sanitized();
                            if let Some(p) = &pipeline {
                                p.update_settings(settings);
                            }
                        }
                        ControllerMsg::RequestKeyframe => {
                            if let Some(p) = &pipeline {
                                p.request_keyframe();
                            }
                        }
                        ControllerMsg::SelectDisplay { index } => {
                            if (index as usize) < displays.len() && index != current {
                                current = index;
                                injector.release_all();
                                // 先停旧管线再启动新管线（新管线从关键帧开始）
                                drop(pipeline.take());
                                pipeline = Some(Pipeline::start(current, settings, out_tx.clone()));
                                ctl(&HostMsg::Displays { displays: displays.clone(), current })?;
                            }
                        }
                        ControllerMsg::Ping { t } => ctl(&HostMsg::Pong { t })?,
                        ControllerMsg::Clipboard { text } => clip.apply(text),
                        ControllerMsg::Audio { enabled } => {
                            if !enabled {
                                sound = None;
                            } else if sound.is_none() {
                                sound = Some(audio::subscribe(audio_tx.clone()));
                            }
                        }
                    }
                }
                _ = clip_tick.tick() => {
                    if let Some(text) = clip.poll().await {
                        ctl(&HostMsg::Clipboard { text })?;
                    }
                }
                _ = kick.changed() => {
                    let reason = *kick.borrow_and_update();
                    info!(session_id, ?reason, "the local user disconnected the controller");
                    ctl(&HostMsg::Bye(reason))?;
                    break;
                }
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

    injector.release_all();
    drop(pipeline);
    drop(sound);
    drop(audio_tx);
    drop(out_tx);
    drop(ctl_tx);
    reader.abort();
    // 等待写任务把已排队的数据（例如 Bye）发完
    let _ = tokio::time::timeout(Duration::from_secs(1), writer).await;
    let result = match result {
        Ok(()) => match reader.await {
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
