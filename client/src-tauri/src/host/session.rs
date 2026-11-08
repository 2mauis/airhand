//! 被控端的单个远控会话。

use airhand_link::flow::Flow;
use airhand_link::tasks::OwnedTask;
use airhand_link::transport::{SessionStreams, open_session, write_lane};
use std::sync::Arc;
use std::time::{Duration, Instant};

use airhand_proto::framing::{self, read_msg};
use airhand_proto::session::{
    ByeReason, ControllerMsg, DisplayInfo, HostHello, HostMsg, Notice, Platform, QualitySettings,
};
use airhand_proto::{MAX_SESSION_MSG, Role};
use anyhow::{Result, anyhow};
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

/// 告诉控制端的平台（控制端据此选快捷键、提示的说法）
#[cfg(windows)]
const PLATFORM: Platform = Platform::Windows;
#[cfg(target_os = "macos")]
const PLATFORM: Platform = Platform::Mac;
#[cfg(not(any(windows, target_os = "macos")))]
const PLATFORM: Platform = Platform::Linux;
/// 检查系统权限是否有变化的间隔
const PERMISSION_CHECK: Duration = Duration::from_secs(2);
/// 检查显示器有没有变化（插拔、旋转、改分辨率 / 缩放 / 排列）的间隔
const DISPLAY_CHECK: Duration = Duration::from_secs(2);

/// 显示器列表变了之后，正在看的显示器在新列表里的序号：按名字找（插拔别的显示器时序号可能变），
/// 找不到（被拔掉了）就改看主显示器（新列表的第一个）。
fn follow_display(old: &[DisplayInfo], current: u8, new: &[DisplayInfo]) -> u8 {
    old.get(current as usize)
        .and_then(|d| new.iter().position(|n| n.name == d.name))
        .unwrap_or(0) as u8
}

/// 等到声音采集的状态有变化；没有订阅声音时一直等下去
async fn audio_failure(sound: &mut Option<AudioSubscription>) -> Option<String> {
    match sound {
        Some(s) => s.failure_changed().await,
        None => std::future::pending().await,
    }
}

/// `(缺少屏幕录制, 缺少辅助功能)` → 提示（两项都为 false 表示已经全部授予）
fn permission_notice((screen, input): (bool, bool)) -> Notice {
    Notice::MissingPermissions { screen, input }
}

pub async fn run(
    conn: Connection,
    session_id: u64,
    mut kick: watch::Receiver<ByeReason>,
) -> Result<()> {
    let SessionStreams {
        control: (send, mut recv),
        video: (video_send, _video_recv),
        audio: (audio_send, _audio_recv),
    } = open_session(&conn, session_id, Role::Host).await?;

    let (mut displays, codecs, notice) = tokio::task::spawn_blocking(|| {
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

    // Independent bounded queues AND QUIC streams: video loss cannot hold up Pong/audio.
    let (out_tx, out_rx) = mpsc::channel::<Bytes>(2);
    let (ctl_tx, ctl_rx) = mpsc::channel::<Bytes>(32);
    let (audio_tx, audio_rx) = mpsc::channel::<Bytes>(audio::QUEUE);
    let ctl = |m: &HostMsg| -> Result<()> {
        ctl_tx
            .try_send(Bytes::from(framing::encode(m)?))
            .map_err(|_| anyhow!("the session is closed"))
    };
    ctl(&HostMsg::Hello(HostHello {
        hostname,
        os,
        version: config::VERSION.to_string(),
        displays: displays.clone(),
        current_display: current,
        codecs,
        platform: PLATFORM,
        // 桌面被控端只有鼠标键盘：不接收触摸，也没有手机那样的系统操作（快捷键由控制端发按键）
        touch: false,
        actions: Vec::new(),
    }))?;
    if displays.is_empty() {
        ctl(&HostMsg::Notice(Notice::NoDisplays))?;
    }
    if let Some(notice) = notice {
        ctl(&HostMsg::Notice(notice))?;
    }
    // 缺少系统权限（macOS）：告诉控制端为什么没有画面 / 不能操作；之后权限有变化时再发
    let mut missing = super::missing_permissions();
    if missing != (false, false) {
        ctl(&HostMsg::Notice(permission_notice(missing)))?;
    }

    let mut writer = OwnedTask(tokio::spawn(async move {
        tokio::try_join!(
            write_lane(send, ctl_rx),
            write_lane(video_send, out_rx),
            write_lane(audio_send, audio_rx)
        )
    }));

    // 读任务：read_msg 不是取消安全的，单独放在一个任务里，通过通道交给主循环
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

    let mut settings = QualitySettings::default();
    let flow = Arc::new(Flow::default());
    let mut pipeline = (!displays.is_empty()).then(|| {
        Pipeline::start(
            current,
            settings,
            out_tx.clone(),
            ctl_tx.clone(),
            flow.clone(),
        )
    });
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
    let mut permission_tick = tokio::time::interval(PERMISSION_CHECK);
    permission_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut display_tick = tokio::time::interval(DISPLAY_CHECK);
    display_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
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
                        ControllerMsg::Ack { seq } => flow.ack(seq),
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
                                if let Some(p) = pipeline.take() { p.stop().await; }
                                pipeline = Some(Pipeline::start(current, settings, out_tx.clone(), ctl_tx.clone(), flow.clone()));
                                ctl(&HostMsg::Displays { displays: displays.clone(), current })?;
                            }
                        }
                        ControllerMsg::Ping { t } => ctl(&HostMsg::Pong { t })?,
                        ControllerMsg::Clipboard { text } => clip.apply(text),
                        // 桌面被控端不请本机用户同意，用不到控制端的标识
                        ControllerMsg::Hello(_) => {}
                        ControllerMsg::Audio { enabled } => {
                            if !enabled {
                                sound = None;
                            } else if sound.is_none() {
                                let s = audio::subscribe(audio_tx.clone());
                                // 别的会话已经在采集、并且正打不开：马上告诉这个控制端
                                if let Some(detail) = s.failure() {
                                    ctl(&HostMsg::Notice(Notice::AudioFailed { detail }))?;
                                }
                                sound = Some(s);
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
                r = &mut writer.0 => {
                    return Err(anyhow!("session writer ended: {r:?}"));
                }
                _ = watchdog.tick() => {
                    if last_msg.elapsed() > PEER_TIMEOUT {
                        anyhow::bail!("the controller stopped responding");
                    }
                }
                // 声音采集打不开 / 恢复了（只在控制端开着声音时）
                failure = audio_failure(&mut sound) => {
                    ctl(&HostMsg::Notice(match failure {
                        Some(detail) => Notice::AudioFailed { detail },
                        None => Notice::AudioRecovered,
                    }))?;
                }
                // 例如本机用户在会话中途打开了"辅助功能"：更新（两项都授予时撤下）控制端的提示
                _ = permission_tick.tick() => {
                    let now = super::missing_permissions();
                    if now != missing {
                        info!(session_id, screen = !now.0, input = !now.1, "permissions changed");
                        missing = now;
                        ctl(&HostMsg::Notice(permission_notice(now)))?;
                    }
                }
                // 显示器变了：鼠标坐标是按这里的列表换算的，旧列表会让点击落到别的位置
                // （例如把显示器转成竖屏后宽高对调）。更新列表并告诉控制端；采集线程自己会重新打开。
                // 正在看的显示器换了序号就跟着它走，被拔掉了就改看主显示器
                _ = display_tick.tick() => {
                    let now = tokio::task::spawn_blocking(capture::displays).await.unwrap_or_default();
                    // 列不出来（例如切换显示模式的瞬间）时保留原来的列表
                    if !now.is_empty() && now != displays {
                        let next = follow_display(&displays, current, &now);
                        info!(session_id, ?now, current = next, "displays changed");
                        displays = now;
                        if next != current || pipeline.is_none() {
                            current = next;
                            injector.release_all();
                            if let Some(p) = pipeline.take() { p.stop().await; }
                            pipeline = Some(Pipeline::start(current, settings, out_tx.clone(), ctl_tx.clone(), flow.clone()));
                        }
                        ctl(&HostMsg::Displays { displays: displays.clone(), current })?;
                    }
                }
            }
        }
        Ok(())
    }
    .await;

    injector.release_all();
    if let Some(p) = pipeline {
        p.stop().await;
    }
    drop(sound);
    drop(audio_tx);
    drop(out_tx);
    drop(ctl_tx);
    reader.abort();
    // 等待写任务把已排队的数据（例如 Bye）发完
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

    fn d(index: u8, name: &str, x: i32, w: u32, h: u32) -> DisplayInfo {
        DisplayInfo {
            index,
            name: name.into(),
            x,
            y: 0,
            width: w,
            height: h,
            primary: index == 0,
        }
    }

    #[test]
    fn follows_the_current_display() {
        let old = [
            d(0, "DISPLAY1", 0, 1920, 1080),
            d(1, "DISPLAY2", 1920, 1920, 1080),
        ];
        // 转成竖屏：序号不变
        let rotated = [
            d(0, "DISPLAY1", 0, 1920, 1080),
            d(1, "DISPLAY2", 1920, 1080, 1920),
        ];
        assert_eq!(follow_display(&old, 1, &rotated), 1);
        // 在左边插了一个显示器，正在看的那个排到了后面
        let plugged = [
            d(0, "DISPLAY1", 0, 1920, 1080),
            d(1, "DISPLAY3", 1920, 1280, 1024),
            d(2, "DISPLAY2", 3200, 1920, 1080),
        ];
        assert_eq!(follow_display(&old, 1, &plugged), 2);
        // 正在看的被拔掉了：改看主显示器
        assert_eq!(follow_display(&old, 1, &old[..1]), 0);
        // 序号本来就不对
        assert_eq!(follow_display(&old, 7, &old), 0);
    }
}
