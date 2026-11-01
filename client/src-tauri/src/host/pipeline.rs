//! 编码管线（每个会话一个线程）：取最新帧 → 节流 → 在途帧控制 → 编码 → 发送。
//!
//! 低延迟的关键：
//! * 永远只编码**最新**的一帧，旧帧直接丢弃，不排队；
//! * 在途（已发送未确认）帧数量受限，网络变慢时编码器等待而不是把数据堆进缓冲区；
//! * 自适应码率根据阻塞情况与 RTT 膨胀程度下调/恢复码率。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use airhand_proto::framing;
use airhand_proto::session::{CursorState, HostMsg, Notice, QualitySettings, VideoFrame};
use bytes::Bytes;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::capture::{self, Frame};
use super::encoder::Encoder;

pub struct Pipeline {
    ctrl: Arc<Ctrl>,
}

struct Ctrl {
    settings: Mutex<QualitySettings>,
    settings_ver: AtomicU64,
    force_key: AtomicBool,
    stop: AtomicBool,
    flow: Flow,
}

impl Pipeline {
    pub fn start(display: u8, settings: QualitySettings, out: mpsc::Sender<Bytes>) -> Self {
        let ctrl = Arc::new(Ctrl {
            settings: Mutex::new(settings),
            settings_ver: AtomicU64::new(0),
            force_key: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            flow: Flow::new(),
        });
        let c = ctrl.clone();
        // 不保留 JoinHandle：停止时线程最多再阻塞几十毫秒，不值得在异步运行时里 join
        std::thread::Builder::new()
            .name("airhand-encoder".into())
            .spawn(move || run(c, display, out))
            .expect("spawn encoder");
        Self { ctrl }
    }

    pub fn update_settings(&self, s: QualitySettings) {
        *self.ctrl.settings.lock().unwrap() = s;
        self.ctrl.settings_ver.fetch_add(1, Ordering::Release);
    }

    pub fn request_keyframe(&self) {
        self.ctrl.force_key.store(true, Ordering::Release);
    }

    pub fn ack(&self, seq: u32) {
        self.ctrl.flow.on_ack(seq);
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.ctrl.stop.store(true, Ordering::Release);
        self.ctrl.flow.cv.notify_all();
    }
}

// ───────────────────────────── 在途帧控制 ─────────────────────────────

struct Flow {
    st: Mutex<FlowState>,
    cv: Condvar,
}

struct FlowState {
    sent: u32,
    acked: u32,
    pending: VecDeque<(u32, Instant)>,
    srtt_ms: f64,
    /// 近 10 秒的最小 RTT（基础时延）
    min_rtt: (f64, Instant),
}

impl Flow {
    fn new() -> Self {
        Self {
            st: Mutex::new(FlowState {
                sent: 0,
                acked: 0,
                pending: VecDeque::new(),
                srtt_ms: 0.0,
                min_rtt: (f64::MAX, Instant::now()),
            }),
            cv: Condvar::new(),
        }
    }

    fn on_sent(&self, seq: u32) {
        let mut st = self.st.lock().unwrap();
        st.sent = seq;
        st.pending.push_back((seq, Instant::now()));
        if st.pending.len() > 256 {
            st.pending.pop_front();
        }
    }

    fn on_ack(&self, seq: u32) {
        let mut st = self.st.lock().unwrap();
        if seq.wrapping_sub(st.acked) as i32 <= 0 {
            return;
        }
        st.acked = seq;
        let now = Instant::now();
        while let Some(&(s, t)) = st.pending.front() {
            if s.wrapping_sub(seq) as i32 > 0 {
                break;
            }
            st.pending.pop_front();
            if s == seq {
                let rtt = now.duration_since(t).as_secs_f64() * 1000.0;
                st.srtt_ms = if st.srtt_ms == 0.0 {
                    rtt
                } else {
                    st.srtt_ms * 0.875 + rtt * 0.125
                };
                if rtt < st.min_rtt.0 || now.duration_since(st.min_rtt.1) > Duration::from_secs(10)
                {
                    st.min_rtt = (rtt, now);
                }
            }
        }
        self.cv.notify_all();
    }

    fn inflight(st: &FlowState) -> u32 {
        st.sent.wrapping_sub(st.acked)
    }

    /// 等待在途帧数低于窗口。返回 false 表示超时。
    fn wait_window(&self, fps: u8, stop: &AtomicBool, timeout: Duration) -> bool {
        let st = self.st.lock().unwrap();
        let window = Self::window(&st, fps);
        let (st, _) = self
            .cv
            .wait_timeout_while(st, timeout, |s| {
                Self::inflight(s) >= window && !stop.load(Ordering::Acquire)
            })
            .unwrap();
        Self::inflight(&st) < window
    }

    /// 窗口 = 基础时延内可发出的帧数 + 2，限制在 [2, 8]。
    fn window(st: &FlowState, fps: u8) -> u32 {
        if st.min_rtt.0 == f64::MAX {
            return 3;
        }
        let frames = (st.min_rtt.0 / 1000.0 * fps as f64).ceil() as u32 + 2;
        frames.clamp(2, 8)
    }

    fn rtt(&self) -> (f64, f64) {
        let st = self.st.lock().unwrap();
        (
            st.srtt_ms,
            if st.min_rtt.0 == f64::MAX {
                0.0
            } else {
                st.min_rtt.0
            },
        )
    }
}

// ───────────────────────────── 自适应码率 ─────────────────────────────

struct Abr {
    target: u32,
    current: u32,
    blocked: u32,
    window_start: Instant,
    calm_since: Instant,
}

impl Abr {
    fn new(target: u32) -> Self {
        Self {
            target,
            current: target,
            blocked: 0,
            window_start: Instant::now(),
            calm_since: Instant::now(),
        }
    }

    fn reset(&mut self, target: u32) {
        *self = Self::new(target);
    }

    fn floor(&self) -> u32 {
        (self.target / 6).max(300).min(self.target)
    }

    fn tick(&mut self, srtt: f64, min_rtt: f64) {
        let now = Instant::now();
        if now.duration_since(self.window_start) < Duration::from_secs(1) {
            return;
        }
        let congested = self.blocked >= 3 || (min_rtt > 0.0 && srtt > min_rtt * 2.0 + 60.0);
        if congested {
            let next = ((self.current as f64) * 0.75) as u32;
            self.current = next.max(self.floor());
            self.calm_since = now;
            debug!(
                kbps = self.current,
                srtt,
                min_rtt,
                blocked = self.blocked,
                "network congested, lowering the bitrate"
            );
        } else if now.duration_since(self.calm_since) > Duration::from_secs(3)
            && self.current < self.target
        {
            self.current = ((self.current as f64 * 1.12) as u32 + 100).min(self.target);
            self.calm_since = now - Duration::from_secs(2);
        }
        self.blocked = 0;
        self.window_start = now;
    }
}

// ───────────────────────────── 编码线程 ─────────────────────────────

fn run(ctrl: Arc<Ctrl>, screen: u8, out: mpsc::Sender<Bytes>) {
    capture::raise_priority();
    let mut settings = *ctrl.settings.lock().unwrap();
    let mut ver = ctrl.settings_ver.load(Ordering::Acquire);
    let sub = capture::subscribe(screen, settings.fps);
    let slot = sub.slot();
    let mut enc: Option<Encoder> = None;
    // 硬件编码器连续失败时改用软件编码
    let mut hw_failures = 0u32;
    let (mut frame_seq, mut cursor_seq, mut shape_seq) = (0u64, 0u64, 0u64);
    let mut last_frame: Option<Arc<Frame>> = None;
    let mut seq = 0u32;
    let mut next_due = Instant::now();
    let mut abr = Abr::new(settings.bitrate_kbps);
    let mut reported_error: Option<String> = None;
    info!(screen, ?settings, "encoding pipeline started");

    let send = |msg: &HostMsg, cap: usize| -> bool {
        match framing::encode_with_capacity(msg, cap) {
            Ok(buf) => out.blocking_send(Bytes::from(buf)).is_ok(),
            Err(e) => {
                warn!("can't encode message: {e}");
                true
            }
        }
    };

    while !ctrl.stop.load(Ordering::Acquire) {
        let v = ctrl.settings_ver.load(Ordering::Acquire);
        if v != ver {
            ver = v;
            settings = *ctrl.settings.lock().unwrap();
            sub.set_fps(settings.fps);
            abr.reset(settings.bitrate_kbps);
            if let Some(e) = &mut enc {
                e.set_bitrate(settings.bitrate_kbps);
            }
            info!(?settings, "quality settings updated");
        }

        let (fs, cs) = slot.wait_change(frame_seq, cursor_seq, Duration::from_millis(40));

        // 光标变化：只发送可见性 +（如有变化）形状
        if cs > cursor_seq {
            cursor_seq = cs;
            let c = slot.cursor();
            let shape = if c.shape_seq > shape_seq {
                c.shape.as_deref().cloned()
            } else {
                None
            };
            shape_seq = c.shape_seq;
            let cap = shape.as_ref().map_or(16, |s| s.rgba.len() + 16);
            if !send(
                &HostMsg::Cursor(CursorState {
                    visible: c.visible,
                    shape,
                }),
                cap,
            ) {
                break;
            }
        }

        let force = ctrl.force_key.load(Ordering::Acquire);
        if fs <= frame_seq {
            if let Some(err) = slot.error()
                && reported_error.as_ref() != Some(&err)
            {
                if !send(
                    &HostMsg::Notice(Notice::CaptureFailed {
                        detail: err.clone(),
                    }),
                    256,
                ) {
                    break;
                }
                reported_error = Some(err);
            }
            // 画面静止但控制端需要关键帧（例如解码器重置）：重新编码上一帧
            if !(force && last_frame.is_some()) {
                if settings.adaptive {
                    let (srtt, min_rtt) = ctrl.flow.rtt();
                    abr.tick(srtt, min_rtt);
                }
                continue;
            }
        } else if reported_error.take().is_some() {
            // 报告过采集失败，现在又有新画面了：让控制端撤下提示
            if !send(&HostMsg::Notice(Notice::CaptureRecovered), 16) {
                break;
            }
        }

        // 帧率节流
        let now = Instant::now();
        if now < next_due {
            std::thread::sleep(next_due - now);
        }
        // 在途帧控制：网络跟不上时在这里等待（期间新的画面会覆盖旧画面，不会排队）
        while !ctrl
            .flow
            .wait_window(settings.fps, &ctrl.stop, Duration::from_millis(50))
        {
            abr.blocked += 1;
            if ctrl.stop.load(Ordering::Acquire) {
                return;
            }
        }
        // 取等待期间到达的最新一帧
        let frame = match slot.try_newer(frame_seq) {
            Some((s, f)) => {
                frame_seq = s;
                f
            }
            None => match &last_frame {
                Some(f) => f.clone(),
                None => continue,
            },
        };
        let started = Instant::now();
        next_due = started + Duration::from_secs_f64(1.0 / settings.fps.max(1) as f64);

        let mut force = ctrl.force_key.swap(false, Ordering::AcqRel);
        if !enc
            .as_ref()
            .is_some_and(|e| e.compatible(frame.width, frame.height, &settings))
        {
            enc = None;
            match Encoder::new(
                frame.width,
                frame.height,
                &settings,
                abr.current,
                hw_failures < 3,
            ) {
                Ok(e) => {
                    if !send(&HostMsg::Encoder(e.info().clone()), 128) {
                        break;
                    }
                    enc = Some(e);
                    force = true;
                }
                Err(e) => {
                    warn!("can't create the encoder: {e:#}");
                    ctrl.force_key.store(true, Ordering::Release);
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
            }
        }
        let e = enc.as_mut().unwrap();
        if settings.adaptive {
            let (srtt, min_rtt) = ctrl.flow.rtt();
            abr.tick(srtt, min_rtt);
            e.set_bitrate(abr.current);
        }

        match e.encode(&frame, force) {
            Ok(Some(ef)) => {
                seq = seq.wrapping_add(1);
                let info = e.info();
                let len = ef.data.len();
                let msg = HostMsg::Video(VideoFrame {
                    seq,
                    key: ef.key,
                    codec: info.codec,
                    width: info.width as u16,
                    height: info.height as u16,
                    capture_us: super::clock::us(frame.captured),
                    data: ef.data,
                });
                ctrl.flow.on_sent(seq);
                if !send(&msg, len) {
                    break;
                }
            }
            Ok(None) => {
                if force {
                    ctrl.force_key.store(true, Ordering::Release);
                }
            }
            Err(err) => {
                warn!("encoding failed, recreating the encoder: {err:#}");
                if e.info().hardware {
                    hw_failures += 1;
                }
                enc = None;
                ctrl.force_key.store(true, Ordering::Release);
            }
        }
        last_frame = Some(frame);
    }
    info!(screen, "encoding pipeline stopped");
}
