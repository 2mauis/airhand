//! 端到端测试：真实服务器 + 真实被控端（屏幕采集 + 编码）+ 协议级控制端。
//!
//! 先在另一个终端启动服务器（不固定证书）：
//! ```text
//! AIRHAND_CERT=target/e2e/cert.pem AIRHAND_KEY=target/e2e/key.pem cargo run -p airhand-server
//! ```
//! 然后运行：`cargo test --lib e2e -- --ignored --nocapture`

use std::time::{Duration, Instant};

use airhand_proto::framing::{read_msg, write_msg};
use airhand_proto::session::{
    ByeReason, Codec, ControllerMsg, HostMsg, InputEvent, QualitySettings,
};
use airhand_proto::signal::{ClientMsg, ConnectError, ServerMsg};
use airhand_proto::{MAX_SESSION_MSG, MAX_SIGNAL_MSG, Role, SessionHeader};

use airhand_link::ServerState;

use crate::host::HostService;
use crate::status;

async fn wait_code(hub: &status::StatusHub) -> u32 {
    let mut rx = hub.subscribe();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        {
            let st = rx.borrow_and_update();
            if let (ServerState::Connected, Some(code)) = (&st.server, st.code) {
                return code;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        assert!(!left.is_zero(), "被控端未能注册");
        let _ = tokio::time::timeout(left, rx.changed()).await;
    }
}

struct Report {
    hello: bool,
    encoders: Vec<(Codec, String, bool)>,
    frames: u32,
    keyframes: u32,
    bytes: usize,
    pongs: u32,
    cursor: u32,
    decoded: u32,
    /// 收到的声音包
    audio: u32,
    audio_starts: u32,
    /// 解码后的声音 RMS
    audio_rms: f64,
    /// 关闭声音之后仍然收到的包
    audio_late: u32,
    /// 收到时距采集时间多久（被控端时钟，毫秒）：最大值
    video_age_ms: f64,
    audio_age_ms: f64,
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_roundtrip() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,quinn=warn")
        .try_init();
    let hub = status::new_hub();
    let _host = HostService::start(hub.clone());
    let code = wait_code(&hub).await;
    println!("被控端控制码: {code}");

    // 控制端：连接服务器
    let mut sig = airhand_link::net::connect_signal(&crate::config::link(), Role::Controller)
        .await
        .expect("控制端连接服务器");
    let mut buf = Vec::new();

    // 错误控制码
    let wrong = if code == 999_999 { 100_000 } else { code + 1 };
    write_msg(
        &mut sig.send,
        &ClientMsg::Connect {
            req_id: 1,
            code: wrong,
        },
    )
    .await
    .unwrap();
    let r: ServerMsg = read_msg(&mut sig.recv, MAX_SIGNAL_MSG, &mut buf)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            r,
            ServerMsg::ConnectResult {
                req_id: 1,
                result: Err(ConnectError::NotFound)
            }
        ),
        "{r:?}"
    );

    // 正确控制码
    write_msg(&mut sig.send, &ClientMsg::Connect { req_id: 2, code })
        .await
        .unwrap();
    let r: ServerMsg = read_msg(&mut sig.recv, MAX_SIGNAL_MSG, &mut buf)
        .await
        .unwrap()
        .unwrap();
    let ServerMsg::ConnectResult {
        req_id: 2,
        result: Ok(grant),
    } = r
    else {
        panic!("{r:?}")
    };
    println!("会话已授权: {} ({})", grant.host_name, grant.host_os);

    let (mut send, mut recv) = sig.conn.open_bi().await.unwrap();
    send.write_all(
        &SessionHeader {
            session_id: grant.session_id,
            role: Role::Controller,
        }
        .encode(),
    )
    .await
    .unwrap();
    let settings = QualitySettings {
        codec: Codec::H264,
        fps: 30,
        ..Default::default()
    };
    write_msg(&mut send, &ControllerMsg::Settings(settings))
        .await
        .unwrap();
    write_msg(
        &mut send,
        &ControllerMsg::Input(vec![InputEvent::ReleaseAll]),
    )
    .await
    .unwrap();
    // 声音：在本机以 -60 dBFS（几乎听不见）播放测试音，被控端环回采集后应能收到并解码
    write_msg(&mut send, &ControllerMsg::Audio { enabled: true })
        .await
        .unwrap();
    let tone = match crate::host::audio::TonePlayer::start(440.0, 0.001) {
        Ok(t) => Some(t),
        Err(e) => {
            println!("无法播放测试音（没有声音输出设备？），跳过声音检查: {e:#}");
            None
        }
    };
    let mut audio_dec = opus::Decoder::new(48_000, opus::Channels::Stereo).unwrap();
    let mut pcm = vec![0f32; 5760 * 2];
    let (mut audio_energy, mut audio_samples) = (0f64, 0usize);
    let mut audio_off: Option<Instant> = None;

    let mut rep = Report {
        hello: false,
        encoders: vec![],
        frames: 0,
        keyframes: 0,
        bytes: 0,
        pongs: 0,
        cursor: 0,
        decoded: 0,
        audio: 0,
        audio_starts: 0,
        audio_rms: 0.0,
        audio_late: 0,
        video_age_ms: 0.0,
        audio_age_ms: 0.0,
    };
    // 被控端在同一进程里，可以直接读它的时钟：画面和声音的采集时间戳都应落在"刚刚"
    let age_ms = |capture_us: u64| {
        (crate::host::clock::us(Instant::now()) as f64 - capture_us as f64) / 1000.0
    };
    let mut decoder = openh264::decoder::Decoder::new().unwrap();
    let mut switched = false;
    // 剪贴板：控制端 → 被控端（写入本机剪贴板），然后本机修改 → 被控端应回传
    let original_clip = crate::clipboard::get_text().ok();
    let to_host = format!("airhand-e2e-to-host-{}", rand::random::<u32>());
    let from_host = format!("airhand-e2e-from-host-{}", rand::random::<u32>());
    write_msg(
        &mut send,
        &ControllerMsg::Clipboard {
            text: to_host.clone(),
        },
    )
    .await
    .unwrap();
    let mut clip_written = false;
    let mut clip_echo = 0;
    let mut clip_back = false;
    let started = Instant::now();
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut rbuf = Vec::new();

    while started.elapsed() < Duration::from_secs(8) {
        if last_ping.elapsed() > Duration::from_millis(500) {
            last_ping = Instant::now();
            write_msg(&mut send, &ControllerMsg::Ping { t: 1 })
                .await
                .unwrap();
        }
        if !clip_written && started.elapsed() > Duration::from_secs(1) {
            let now = crate::clipboard::get_text().unwrap_or_default();
            assert_eq!(now, to_host, "被控端没有写入剪贴板");
            clip_written = true;
            crate::clipboard::set_text(&from_host).unwrap();
        }
        // 3 秒后切换到 AV1/HEVC（若被控端支持），验证运行时切换编码器
        if !switched && started.elapsed() > Duration::from_secs(3) {
            switched = true;
            let s = QualitySettings {
                codec: Codec::AV1,
                fps: 60,
                bitrate_kbps: 8000,
                ..settings
            };
            write_msg(&mut send, &ControllerMsg::Settings(s))
                .await
                .unwrap();
        }
        // 6 秒后关闭声音：之后不应再收到声音包
        if audio_off.is_none() && started.elapsed() > Duration::from_secs(6) {
            audio_off = Some(Instant::now());
            write_msg(&mut send, &ControllerMsg::Audio { enabled: false })
                .await
                .unwrap();
        }
        let msg = match tokio::time::timeout(
            Duration::from_secs(1),
            read_msg::<_, HostMsg>(&mut recv, MAX_SESSION_MSG, &mut rbuf),
        )
        .await
        {
            Ok(m) => m.expect("读取会话消息").expect("会话被关闭"),
            Err(_) => continue, // 画面静止时不会有帧
        };
        match msg {
            HostMsg::Hello(h) => {
                rep.hello = true;
                println!(
                    "Hello: {} / {} / 显示器 {} / 编码 {:?}",
                    h.hostname,
                    h.os,
                    h.displays.len(),
                    h.codecs
                );
            }
            HostMsg::Encoder(e) => {
                println!(
                    "编码器: {} {:?} {}x{} hw={}",
                    e.name, e.codec, e.width, e.height, e.hardware
                );
                rep.encoders.push((e.codec, e.name, e.hardware));
            }
            HostMsg::Video(v) => {
                let age = age_ms(v.capture_us);
                assert!(age >= 0.0, "画面时间戳在未来: {age} ms");
                rep.video_age_ms = rep.video_age_ms.max(age);
                rep.frames += 1;
                rep.keyframes += v.key as u32;
                rep.bytes += v.data.len();
                if rep.frames == 1 {
                    assert!(v.key, "第一帧必须是关键帧");
                }
                if v.codec == Codec::H264 && matches!(decoder.decode(&v.data), Ok(Some(_))) {
                    rep.decoded += 1;
                }
                write_msg(&mut send, &ControllerMsg::Ack { seq: v.seq })
                    .await
                    .unwrap();
            }
            HostMsg::Pong { .. } => rep.pongs += 1,
            HostMsg::Cursor(_) => rep.cursor += 1,
            HostMsg::Clipboard { text } => {
                if text == to_host {
                    clip_echo += 1;
                } else if text == from_host {
                    clip_back = true;
                }
            }
            HostMsg::Bye(reason) => panic!("意外的 Bye: {reason:?}"),
            HostMsg::Displays { .. } => {}
            HostMsg::Notice(notice) => println!("提示: {notice:?}"),
            HostMsg::Audio(a) => {
                let age = age_ms(a.capture_us);
                assert!(age >= 0.0, "声音时间戳在未来: {age} ms");
                rep.audio_age_ms = rep.audio_age_ms.max(age);
                if audio_off.is_some_and(|t| t.elapsed() > Duration::from_millis(300)) {
                    rep.audio_late += 1;
                }
                rep.audio += 1;
                rep.audio_starts += a.start as u32;
                let n = audio_dec
                    .decode_float(&a.data, &mut pcm, false)
                    .expect("Opus 解码");
                // 跳过开头几包（播放刚启动、编码器收敛）
                if rep.audio > 10 {
                    audio_energy += pcm[..n * 2]
                        .iter()
                        .map(|s| (*s as f64).powi(2))
                        .sum::<f64>();
                    audio_samples += n * 2;
                }
            }
        }
    }
    let tone_ok = tone.is_some();
    drop(tone);
    rep.audio_rms = (audio_energy / audio_samples.max(1) as f64).sqrt();
    if let Some(t) = original_clip {
        let _ = crate::clipboard::set_text(&t);
    }
    assert!(clip_back, "本机剪贴板变化没有同步给控制端");
    assert_eq!(clip_echo, 0, "剪贴板出现回声");

    // 托盘"断开所有控制端"：控制端应收到 Bye
    _host.send(crate::host::HostCmd::KickAll);
    let bye = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match read_msg::<_, HostMsg>(&mut recv, MAX_SESSION_MSG, &mut rbuf).await {
                Ok(Some(HostMsg::Bye(reason))) => break Some(reason),
                Ok(Some(_)) => continue,
                _ => break None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    assert_eq!(
        bye,
        Some(ByeReason::Kicked),
        "没有收到\"断开所有控制端\"的 Bye"
    );
    let _ = send.finish();
    println!(
        "帧 {} (关键帧 {}) · {:.1} KB · H.264 解码成功 {} · pong {} · 光标 {} · 声音包 {} (start {}, RMS {:.6}, 关闭后 {})",
        rep.frames,
        rep.keyframes,
        rep.bytes as f64 / 1024.0,
        rep.decoded,
        rep.pongs,
        rep.cursor,
        rep.audio,
        rep.audio_starts,
        rep.audio_rms,
        rep.audio_late,
    );
    if tone_ok {
        // 测试音持续播放 6 秒，每包 20 ms：应收到约 300 包
        assert!(rep.audio >= 150, "声音包太少: {}", rep.audio);
        // 环回采集的是音量调节之后的声音，电平取决于系统音量（-60 dBFS 的测试音在 100% 音量时 RMS ≈ 7e-4）
        assert!(
            rep.audio_rms > 1e-5,
            "声音能量异常（系统音量是否为 0？）: {}",
            rep.audio_rms
        );
        assert_eq!(rep.audio_late, 0, "关闭声音后仍在发送");
        println!(
            "采集到收到：画面最多 {:.1} ms，声音最多 {:.1} ms",
            rep.video_age_ms, rep.audio_age_ms
        );
        assert!(
            rep.audio_age_ms < 500.0,
            "声音时间戳与画面不在同一个时钟上？{:.1} ms",
            rep.audio_age_ms
        );
    }
    assert!(rep.hello, "没有收到 Hello");
    assert!(rep.frames >= 2, "收到的视频帧太少");
    assert!(rep.keyframes >= 1);
    assert!(rep.pongs >= 3, "Ping/Pong 不工作");
    assert!(!rep.encoders.is_empty());
    assert!(rep.encoders.len() >= 2, "切换编码格式后应重建编码器");
    assert!(rep.decoded >= 1, "H.264 码流无法解码");
    // 画面静止时会重新编码上一帧（时间戳不变），所以只检查不离谱
    assert!(
        rep.video_age_ms < 10_000.0,
        "画面时间戳异常: {:.1} ms",
        rep.video_age_ms
    );

    // 会话结束后被控端应释放会话
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        hub.borrow().controllers.is_empty(),
        "会话没有被清理: {:?}",
        hub.borrow().controllers
    );
}

/// 服务器重启后，被控端应自动重连并取回原控制码。会自行启动/重启服务器进程。
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn e2e_reconnect_keeps_code() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("info,quinn=warn")
        .try_init();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let exe = root.join(format!(
        "target/debug/airhand-server{}",
        std::env::consts::EXE_SUFFIX
    ));
    let spawn = || {
        std::process::Command::new(&exe)
            .env("AIRHAND_LISTEN", "127.0.0.1:4433")
            .env("AIRHAND_CERT", root.join("target/e2e/cert.pem"))
            .env("AIRHAND_KEY", root.join("target/e2e/key.pem"))
            .env("RUST_LOG", "warn")
            .spawn()
            .expect("启动服务器（先 cargo build -p airhand-server）")
    };
    let mut server = spawn();
    let hub = status::new_hub();
    let _host = HostService::start(hub.clone());
    let code = wait_code(&hub).await;
    println!("初始控制码 {code}");

    server.kill().unwrap();
    let _ = server.wait();
    let t = Instant::now();
    let mut rx = hub.subscribe();
    while matches!(rx.borrow_and_update().server, ServerState::Connected) {
        let _ = tokio::time::timeout(Duration::from_secs(20), rx.changed()).await;
        assert!(t.elapsed() < Duration::from_secs(25), "没有检测到掉线");
    }
    println!(
        "检测到掉线，用时 {:.1}s: {:?}",
        t.elapsed().as_secs_f32(),
        hub.borrow().server
    );

    let mut server = spawn();
    let t = Instant::now();
    let code2 = wait_code(&hub).await;
    println!(
        "重连成功，用时 {:.1}s，控制码 {code2}",
        t.elapsed().as_secs_f32()
    );
    assert_eq!(code, code2, "重连后控制码应保持不变");

    // 托盘"更换控制码"
    _host.send(crate::host::HostCmd::NewCode);
    let t = Instant::now();
    let code3 = loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let st = hub.borrow().clone();
        if let (ServerState::Connected, Some(c)) = (&st.server, st.code)
            && c != code2
        {
            break c;
        }
        assert!(t.elapsed() < Duration::from_secs(5), "更换控制码超时");
    };
    println!(
        "已更换控制码 {code2} -> {code3}，用时 {:.1}s",
        t.elapsed().as_secs_f32()
    );
    server.kill().unwrap();
    let _ = server.wait();
}
