//! Independent control, video and audio QUIC streams for one session.
//! Each stream has its own flow control and loss recovery. Connection congestion
//! is still shared; this does not promise unlimited audio bandwidth on a full link.
use airhand_proto::{MAX_SESSION_MSG, Role, SessionHeader, SessionLane};
use airhand_proto::{framing::read_msg, session::HostMsg};
use anyhow::{Context, Result};
use quinn::{Connection, RecvStream, SendStream};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

pub type Stream = (SendStream, RecvStream);

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

/// Cancellation-safe receive interface. Each framing reader lives for the whole
/// stream; select!/timeout only cancel channel receives, never a partial frame.
/// Separate bounded mailboxes prevent a video flood from hiding a Pong or Bye.
pub struct HostReceiver {
    control: mpsc::Receiver<Result<HostMsg>>,
    video: mpsc::Receiver<Result<HostMsg>>,
    audio: mpsc::Receiver<Result<HostMsg>>,
    video_open: bool,
    audio_open: bool,
    _tasks: JoinSet<()>,
    // Keep the unused reverse media halves alive for the lifetime of the session.
    _reverse: (SendStream, SendStream),
}
impl HostReceiver {
    pub fn new(control: RecvStream, video: Stream, audio: Stream) -> Self {
        let mut tasks = JoinSet::new();
        fn reader(
            tasks: &mut JoinSet<()>,
            mut recv: RecvStream,
            capacity: usize,
            max: usize,
        ) -> mpsc::Receiver<Result<HostMsg>> {
            let (tx, rx) = mpsc::channel(capacity);
            tasks.spawn(async move {
                let mut buf = Vec::new();
                loop {
                    let result = match read_msg(&mut recv, max, &mut buf).await {
                        Ok(Some(msg)) => Ok(msg),
                        Ok(None) => break,
                        Err(e) => Err(e.into()),
                    };
                    let end = result.is_err();
                    if tx.send(result).await.is_err() || end {
                        break;
                    }
                }
            });
            rx
        }
        Self {
            control: reader(&mut tasks, control, 16, 2 * 1024 * 1024),
            video: reader(&mut tasks, video.1, 2, MAX_SESSION_MSG),
            audio: reader(&mut tasks, audio.1, 16, 64 * 1024),
            video_open: true,
            audio_open: true,
            _tasks: tasks,
            _reverse: (video.0, audio.0),
        }
    }
    pub async fn recv(&mut self) -> Result<HostMsg> {
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
