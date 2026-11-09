//! 一个会话的三条 QUIC 流（控制、画面、声音，见 `airhand_proto` 的模块说明）。
//!
//! 每条流有自己的流量控制和丢包重传，画面堵住不影响声音和控制消息（整条连接的拥塞控制还是共用的，
//! 网络满了声音也会慢）。
//!
//! * 双方：[`open_session`] 打开三条流；
//! * 被控端：[`write_lane`] 把排好队的数据写进一条流（见 [`crate::host::HostSession`]）；
//! * 控制端：[`HostReceiver`] 从三条流读被控端发来的东西。
use airhand_proto::framing::{read_msg, read_packet};
use airhand_proto::media::{self, Kind};
use airhand_proto::session::HostMsg;
use airhand_proto::{MAX_CONTROL_MSG, Role, SessionHeader, SessionLane};
use anyhow::{Context, Result, anyhow};
use quinn::{Connection, RecvStream, SendStream};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

pub type Stream = (SendStream, RecvStream);

/// 把 `rx` 里排好队的数据（已经带长度前缀）依次写进一条流，`rx` 关闭后正常结束这条流
pub async fn write_lane(mut send: SendStream, mut rx: mpsc::Receiver<bytes::Bytes>) -> Result<()> {
    while let Some(bytes) = rx.recv().await {
        tokio::time::timeout(Duration::from_secs(10), send.write_chunk(bytes))
            .await
            .context("session writer stalled")??;
    }
    send.finish()?;
    Ok(())
}

pub struct SessionStreams {
    pub control: Stream,
    pub video: Stream,
    pub audio: Stream,
}

pub async fn open_session(
    conn: &Connection,
    session_id: u64,
    role: Role,
) -> Result<SessionStreams> {
    async fn lane(
        conn: &Connection,
        session_id: u64,
        role: Role,
        lane: SessionLane,
    ) -> Result<Stream> {
        let (mut send, recv) = conn.open_bi().await?;
        send.set_priority(match lane {
            SessionLane::Control => 10,
            SessionLane::Audio => 5,
            SessionLane::Video => 0,
        })?;
        send.write_all(
            &SessionHeader {
                session_id,
                role,
                lane,
            }
            .encode(),
        )
        .await?;
        Ok((send, recv))
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let control = lane(conn, session_id, role, SessionLane::Control).await?;
        let video = lane(conn, session_id, role, SessionLane::Video).await?;
        let audio = lane(conn, session_id, role, SessionLane::Audio).await?;
        Ok(SessionStreams {
            control,
            video,
            audio,
        })
    })
    .await
    .context("timed out opening session streams")?
}

/// 被控端发来的一样东西
#[derive(Debug)]
pub enum FromHost {
    /// 控制流上的消息
    Control(HostMsg),
    /// 画面包（[`media`] 格式：头 + 码流），原样交给界面
    Video(Vec<u8>),
    /// 声音包（[`media`] 格式：头 + Opus 包），原样交给界面
    Audio(Vec<u8>),
}

/// 控制端读被控端的三条流。
///
/// 每条流一个读任务，读任务活到流结束：`select!` / 超时只会取消通道上的接收，不会切断半帧。
/// 三个有界的信箱互相独立，画面再多也挡不住 Pong 和 Bye。
pub struct HostReceiver {
    control: mpsc::Receiver<Result<FromHost>>,
    video: mpsc::Receiver<Result<FromHost>>,
    audio: mpsc::Receiver<Result<FromHost>>,
    video_open: bool,
    audio_open: bool,
    _tasks: JoinSet<()>,
    // 画面流和声音流反方向的一半用不到，但要留到会话结束
    _reverse: (SendStream, SendStream),
}

impl HostReceiver {
    pub fn new(control: RecvStream, video: Stream, audio: Stream) -> Self {
        let mut tasks = JoinSet::new();
        Self {
            control: control_reader(&mut tasks, control),
            video: media_reader(&mut tasks, video.1, Kind::Video, 2),
            audio: media_reader(&mut tasks, audio.1, Kind::Audio, 16),
            video_open: true,
            audio_open: true,
            _tasks: tasks,
            _reverse: (video.0, audio.0),
        }
    }

    /// 下一样东西。控制消息优先；控制流结束了就是会话结束了（画面流、声音流先结束不算）
    pub async fn recv(&mut self) -> Result<FromHost> {
        loop {
            tokio::select! {
                biased;
                m = self.control.recv() => return m.context("control stream ended")?,
                m = self.audio.recv(), if self.audio_open => match m {
                    Some(m) => return m,
                    None => self.audio_open = false,
                },
                m = self.video.recv(), if self.video_open => match m {
                    Some(m) => return m,
                    None => self.video_open = false,
                },
            }
        }
    }
}

/// 把读到的东西放进信箱。返回 false 表示这条流读完了（结束、出错，或者没人要了）
async fn forward(tx: &mpsc::Sender<Result<FromHost>>, r: Result<Option<FromHost>>) -> bool {
    let r = match r {
        Ok(Some(m)) => Ok(m),
        Ok(None) => return false,
        Err(e) => Err(e),
    };
    let more = r.is_ok();
    tx.send(r).await.is_ok() && more
}

fn control_reader(
    tasks: &mut JoinSet<()>,
    mut recv: RecvStream,
) -> mpsc::Receiver<Result<FromHost>> {
    let (tx, rx) = mpsc::channel(16);
    tasks.spawn(async move {
        let mut buf = Vec::new();
        loop {
            let r = read_msg::<_, HostMsg>(&mut recv, MAX_CONTROL_MSG, &mut buf)
                .await
                .map(|m| m.map(FromHost::Control))
                .map_err(Into::into);
            if !forward(&tx, r).await {
                break;
            }
        }
    });
    rx
}

fn media_reader(
    tasks: &mut JoinSet<()>,
    mut recv: RecvStream,
    kind: Kind,
    capacity: usize,
) -> mpsc::Receiver<Result<FromHost>> {
    let (tx, rx) = mpsc::channel(capacity);
    let max = match kind {
        Kind::Audio => media::MAX_AUDIO_PACKET,
        _ => media::MAX_VIDEO_PACKET,
    };
    tasks.spawn(async move {
        loop {
            let r = match read_packet(&mut recv, max).await {
                Ok(Some(p)) if Kind::of(&p) == Some(kind) => Ok(Some(match kind {
                    Kind::Audio => FromHost::Audio(p),
                    _ => FromHost::Video(p),
                })),
                Ok(Some(_)) => Err(anyhow!("unexpected packet on the {kind:?} stream")),
                Ok(None) => Ok(None),
                Err(e) => Err(e.into()),
            };
            if !forward(&tx, r).await {
                break;
            }
        }
    });
    rx
}
