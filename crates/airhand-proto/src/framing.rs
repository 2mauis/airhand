//! 长度前缀帧：`u32 LE 长度 | postcard 负载`。

use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("encoding error: {0}")]
    Codec(#[from] postcard::Error),
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
}

/// 把消息编码为带长度前缀的字节串。
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, FrameError> {
    encode_with_capacity(msg, 32)
}

/// 同 [`encode`]，按预估大小一次分配好（视频帧这类大消息用，避免多次扩容）。
pub fn encode_with_capacity<T: Serialize>(msg: &T, cap: usize) -> Result<Vec<u8>, FrameError> {
    let mut buf = Vec::with_capacity(cap + 32);
    buf.extend_from_slice(&[0u8; 4]);
    let mut buf = postcard::to_extend(msg, buf)?;
    let len = buf.len() - 4;
    buf[..4].copy_from_slice(&(len as u32).to_le_bytes());
    Ok(buf)
}

pub async fn write_msg<W, T>(w: &mut W, msg: &T) -> Result<(), FrameError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let buf = encode(msg)?;
    w.write_all(&buf).await?;
    Ok(())
}

/// 读取一条消息。对端在消息边界处正常关闭时返回 `Ok(None)`。
///
/// `buf` 由调用方复用，避免每条消息都分配内存。
pub async fn read_msg<R, T>(
    r: &mut R,
    max: usize,
    buf: &mut Vec<u8>,
) -> Result<Option<T>, FrameError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut len = [0u8; 4];
    // 区分"在边界处干净关闭"与"半截消息"
    let mut got = 0;
    while got < 4 {
        let n = r.read(&mut len[got..]).await?;
        if n == 0 {
            if got == 0 {
                return Ok(None);
            }
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        got += n;
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > max {
        return Err(FrameError::TooLarge(len));
    }
    buf.clear();
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(Some(postcard::from_bytes(buf)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::*;
    use crate::signal::*;

    #[tokio::test]
    async fn roundtrip() {
        let msgs = vec![
            HostMsg::Video(VideoFrame {
                seq: 7,
                key: true,
                codec: Codec::H264,
                width: 1920,
                height: 1080,
                capture_us: 42,
                data: vec![0, 0, 0, 1, 0x67, 1, 2, 3],
            }),
            HostMsg::Pong { t: 99 },
            HostMsg::Notice(Notice::CaptureFailed {
                detail: "AcquireNextFrame: 0x887A0026".into(),
            }),
            HostMsg::Notice(Notice::MissingPermissions {
                screen: true,
                input: false,
            }),
            HostMsg::Notice(Notice::AudioFailed {
                detail: "no default audio output device".into(),
            }),
            HostMsg::Notice(Notice::AudioRecovered),
            HostMsg::Bye(ByeReason::CodeChanged),
            HostMsg::Audio(AudioFrame {
                seq: 3,
                start: true,
                capture_us: 1 << 40,
                data: vec![0xfc, 0xff, 0xfe],
            }),
        ];
        let mut wire = Vec::new();
        for m in &msgs {
            write_msg(&mut wire, m).await.unwrap();
        }
        let mut r = &wire[..];
        let mut buf = Vec::new();
        let a: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(matches!(a, HostMsg::Video(ref v) if v.seq == 7 && v.data.len() == 8));
        let b: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(matches!(b, HostMsg::Pong { t: 99 }));
        let n: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(
            matches!(n, HostMsg::Notice(Notice::CaptureFailed { ref detail }) if detail == "AcquireNextFrame: 0x887A0026")
        );
        let p: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(matches!(
            p,
            HostMsg::Notice(Notice::MissingPermissions {
                screen: true,
                input: false
            })
        ));
        let a: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(
            matches!(a, HostMsg::Notice(Notice::AudioFailed { ref detail }) if detail == "no default audio output device")
        );
        let a: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(matches!(a, HostMsg::Notice(Notice::AudioRecovered)));
        let bye: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(matches!(bye, HostMsg::Bye(ByeReason::CodeChanged)));
        let a: HostMsg = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap().unwrap();
        assert!(
            matches!(a, HostMsg::Audio(ref f) if f.seq == 3 && f.start && f.capture_us == 1 << 40 && f.data == [0xfc, 0xff, 0xfe])
        );
        let c: Option<HostMsg> = read_msg(&mut r, 1 << 20, &mut buf).await.unwrap();
        assert!(c.is_none());
    }

    #[tokio::test]
    async fn signal_roundtrip() {
        let m = ServerMsg::ConnectResult {
            req_id: 3,
            result: Err(ConnectError::Busy),
        };
        let wire = encode(&m).unwrap();
        let mut r = &wire[..];
        let mut buf = Vec::new();
        let back: ServerMsg = read_msg(&mut r, 1024, &mut buf).await.unwrap().unwrap();
        assert!(matches!(
            back,
            ServerMsg::ConnectResult {
                req_id: 3,
                result: Err(ConnectError::Busy)
            }
        ));

        for ip in ["10.0.0.7", "2001:db8::1"] {
            let peer = Peer {
                name: "PC".into(),
                ip: ip.parse().unwrap(),
            };
            let wire = encode(&ServerMsg::Incoming {
                session_id: 9,
                peer: peer.clone(),
            })
            .unwrap();
            let back: ServerMsg = read_msg(&mut &wire[..], 1024, &mut buf)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(back, ServerMsg::Incoming { session_id: 9, peer: ref p } if *p == peer)
            );
        }

        // 重连：带凭证的请求、带凭证的会话授权、"不在线"、"重新上线了"
        let token = [7u8; 16];
        let wire = encode(&ClientMsg::Connect {
            req_id: 4,
            code: 123_456,
            resume: Some(token),
        })
        .unwrap();
        let back: ClientMsg = read_msg(&mut &wire[..], 1024, &mut buf)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            back,
            ClientMsg::Connect { req_id: 4, code: 123_456, resume: Some(t) } if t == token
        ));
        for m in [
            ServerMsg::ConnectResult {
                req_id: 5,
                result: Ok(SessionGrant {
                    session_id: 1,
                    host_name: "PC".into(),
                    host_os: "Windows".into(),
                    resume: token,
                }),
            },
            ServerMsg::ConnectResult {
                req_id: 6,
                result: Err(ConnectError::Offline),
            },
            ServerMsg::HostOnline { code: 123_456 },
        ] {
            let wire = encode(&m).unwrap();
            let back: ServerMsg = read_msg(&mut &wire[..], 1024, &mut buf)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(format!("{back:?}"), format!("{m:?}"));
        }
    }

    /// 版本握手的消息永远不能变（见 signal 模块说明）。这个测试锁定它们的编码：
    /// 如果它失败了，说明改动会让其它版本的客户端 / 服务器连"版本不一致"都无法得知。
    #[test]
    fn version_handshake_wire_is_stable() {
        let hello = ClientMsg::Hello {
            version: 7,
            role: crate::Role::Controller,
            name: "A".into(),
            os: "B".into(),
        };
        // 变体 0 | version=7 | Role::Controller=1 | "A" | "B"
        assert_eq!(
            postcard::to_stdvec(&hello).unwrap(),
            [0, 7, 1, 1, b'A', 1, b'B']
        );
        let host = ClientMsg::Hello {
            version: 7,
            role: crate::Role::Host,
            name: String::new(),
            os: String::new(),
        };
        assert_eq!(postcard::to_stdvec(&host).unwrap(), [0, 7, 0, 0, 0]);
        assert_eq!(
            postcard::to_stdvec(&ServerMsg::Welcome { version: 7 }).unwrap(),
            [0, 7]
        );
        assert_eq!(
            postcard::to_stdvec(&ServerMsg::Incompatible { server_version: 7 }).unwrap(),
            [1, 7]
        );
        assert_eq!(crate::ALPN, b"airhand/1");
    }

    #[tokio::test]
    async fn too_large() {
        let wire = encode(&ControllerMsg::Input(vec![InputEvent::Text {
            text: "x".repeat(100),
        }]))
        .unwrap();
        let mut r = &wire[..];
        let mut buf = Vec::new();
        let res: Result<Option<ControllerMsg>, _> = read_msg(&mut r, 10, &mut buf).await;
        assert!(matches!(res, Err(FrameError::TooLarge(_))));
    }
}
