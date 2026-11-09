//! PulseAudio 环回采集：录下默认输出设备的"监视器"（monitor source），即系统正在播放的声音，
//! 不包括麦克风。PipeWire 桌面（pipewire-pulse）同样适用。
//!
//! 直接让服务器把格式转换为 48 kHz 立体声 float，不需要自己重采样。主循环由采集线程手动驱动，
//! 不额外开线程。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use airhand_proto::media::AUDIO_SAMPLE_RATE;
use anyhow::{Context as _, Result, anyhow, bail};
use libpulse_binding as pa;
use pa::callbacks::ListResult;
use pa::context::{Context, FlagSet as ContextFlags, State as ContextState};
use pa::def::BufferAttr;
use pa::mainloop::standard::{IterateResult, Mainloop};
use pa::operation::State as OpState;
use pa::proplist::{Proplist, properties};
use pa::sample::{Format, Spec};
use pa::stream::{FlagSet as StreamFlags, Latency, PeekResult, State as StreamState, Stream};

const SPEC: Spec = Spec {
    format: Format::F32le,
    rate: AUDIO_SAMPLE_RATE,
    channels: 2,
};
/// 一帧（立体声 float）的字节数
const FRAME_BYTES: usize = 8;
/// 每 10 ms 交付一次数据
const FRAGMENT: u32 = AUDIO_SAMPLE_RATE / 100 * FRAME_BYTES as u32;
const TIMEOUT: Duration = Duration::from_secs(3);

/// libpulse 的错误。它的说明文字是按本机语言翻译的（libpulse 用 gettext），所以发给控制端的说明里
/// 只带错误码（见 `capture::wire_detail`），日志里照常是完整的说明。
#[derive(Debug)]
pub struct PulseError(pub pa::error::PAErr);

impl PulseError {
    pub fn code(&self) -> i32 {
        self.0.0
    }
}

impl std::fmt::Display for PulseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (PulseAudio error {})", self.0, self.code())
    }
}

impl std::error::Error for PulseError {}

/// 与声音服务器的连接。
struct Client {
    // 按声明顺序释放：context 必须先于 mainloop
    context: Context,
    mainloop: Mainloop,
}

impl Client {
    fn connect(name: &str) -> Result<Self> {
        let mainloop =
            Mainloop::new().ok_or_else(|| anyhow!("can't create the PulseAudio main loop"))?;
        let mut props =
            Proplist::new().ok_or_else(|| anyhow!("can't create the PulseAudio property list"))?;
        let _ = props.set_str(properties::APPLICATION_NAME, "Airhand");
        let mut context = Context::new_with_proplist(&mainloop, name, &props)
            .ok_or_else(|| anyhow!("can't create the PulseAudio context"))?;
        const CONNECT: &str = "can't connect to the sound server (is PulseAudio/PipeWire running?)";
        context
            .connect(None, ContextFlags::NOAUTOSPAWN, None)
            .map_err(PulseError)
            .context(CONNECT)?;
        let mut me = Self { context, mainloop };
        me.wait(|c| match c.context.get_state() {
            ContextState::Ready => Some(Ok(())),
            ContextState::Failed | ContextState::Terminated => Some(Err(anyhow::Error::new(
                PulseError(c.context.errno()),
            )
            .context(CONNECT))),
            _ => None,
        })?;
        Ok(me)
    }

    /// 处理已到达的消息，不阻塞。
    fn pump(&mut self) -> Result<()> {
        loop {
            match self.mainloop.iterate(false) {
                IterateResult::Success(0) => return Ok(()),
                IterateResult::Success(_) => {}
                IterateResult::Quit(_) => bail!("the PulseAudio main loop quit"),
                IterateResult::Err(e) => {
                    return Err(anyhow::Error::new(PulseError(e)).context("PulseAudio main loop"));
                }
            }
        }
    }

    /// 驱动主循环直到 `done` 给出结果，最多等 [`TIMEOUT`]。
    fn wait<T>(&mut self, mut done: impl FnMut(&mut Self) -> Option<Result<T>>) -> Result<T> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            self.pump()?;
            if let Some(r) = done(self) {
                return r;
            }
            if Instant::now() > deadline {
                bail!("the sound server didn't respond in time");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn default_sink(&mut self) -> Result<String> {
        let out = Rc::new(RefCell::new(None));
        let o = out.clone();
        let op = self.context.introspect().get_server_info(move |i| {
            *o.borrow_mut() = i.default_sink_name.as_ref().map(|s| s.to_string());
        });
        self.wait(|_| (op.get_state() != OpState::Running).then_some(Ok(())))?;
        out.take()
            .ok_or_else(|| anyhow!("no default audio output device"))
    }

    fn monitor_of(&mut self, sink: &str) -> Result<String> {
        let out = Rc::new(RefCell::new(None));
        let o = out.clone();
        let op = self
            .context
            .introspect()
            .get_sink_info_by_name(sink, move |r| {
                if let ListResult::Item(i) = r {
                    *o.borrow_mut() = i.monitor_source_name.as_ref().map(|s| s.to_string());
                }
            });
        self.wait(|_| (op.get_state() != OpState::Running).then_some(Ok(())))?;
        out.take()
            .ok_or_else(|| anyhow!("audio output device {sink} has no monitor source"))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.context.disconnect();
    }
}

pub struct Loopback {
    // 按声明顺序释放：stream 必须先于 client
    stream: Stream,
    client: Client,
    sink: String,
}

impl Loopback {
    pub fn open() -> Result<Self> {
        let mut client = Client::connect("Airhand audio capture")?;
        let sink = client.default_sink()?;
        let monitor = client.monitor_of(&sink)?;
        let mut stream = Stream::new(&mut client.context, "Airhand audio capture", &SPEC, None)
            .ok_or_else(|| anyhow!("can't create the capture stream"))?;
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: FRAGMENT,
        };
        // DONT_MOVE：设备没了就让流失败（由上层重新打开默认设备），而不是被挪到麦克风之类的设备上
        let flags = StreamFlags::ADJUST_LATENCY
            | StreamFlags::AUTO_TIMING_UPDATE
            | StreamFlags::INTERPOLATE_TIMING
            | StreamFlags::DONT_MOVE;
        stream
            .connect_record(Some(&monitor), Some(&attr), flags)
            .map_err(PulseError)
            .context("can't open audio capture")?;
        let mut me = Self {
            stream,
            client,
            sink,
        };
        me.client.wait(|_| match me.stream.get_state() {
            StreamState::Ready => Some(Ok(())),
            StreamState::Failed | StreamState::Terminated => {
                Some(Err(anyhow!("can't open audio capture")))
            }
            _ => None,
        })?;
        tracing::info!(sink = %me.sink, %monitor, "audio capture opened");
        Ok(me)
    }

    /// 读出当前已采集的全部数据（48 kHz 立体声交错 float），追加到 `out`。没有数据时立即返回。
    ///
    /// 返回追加的第一个采样的采集时间（按流的延迟推算；服务器还没给出时序信息时为 `None`）。
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<Option<Instant>> {
        self.client.pump()?;
        if self.client.context.get_state() != ContextState::Ready {
            bail!("lost connection to the sound server");
        }
        if self.stream.get_state() != StreamState::Ready {
            bail!("the capture stream ended (output device removed?)");
        }
        // 录音流的延迟 = 读指针处的采样从被采集到现在过了多久
        let first = match self.stream.readable_size() {
            Some(n) if n > 0 => match self.stream.get_latency() {
                Ok(Latency::Positive(us)) => {
                    Instant::now().checked_sub(Duration::from_micros(us.0))
                }
                Ok(_) => Some(Instant::now()),
                Err(_) => None,
            },
            _ => None,
        };
        loop {
            match self
                .stream
                .peek()
                .map_err(PulseError)
                .context("reading audio failed")?
            {
                PeekResult::Empty => break,
                // 空洞（数据丢失）按静音补齐
                PeekResult::Hole(n) => out.resize(out.len() + n / 4, 0.0),
                PeekResult::Data(d) => {
                    out.extend(d.as_chunks::<4>().0.iter().map(|s| f32::from_le_bytes(*s)))
                }
            }
            self.stream
                .discard()
                .map_err(PulseError)
                .context("reading audio failed")?;
        }
        Ok(first)
    }

    /// 默认输出设备是否已经换了（或者没了）。
    pub fn default_changed(&mut self) -> bool {
        self.client.default_sink().map_or(true, |s| s != self.sink)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = self.stream.disconnect();
    }
}

// ───────────────────────────── 测试 ─────────────────────────────

/// 测试用：在默认输出设备上播放一个很轻的正弦波，Drop 时停止。
#[cfg(test)]
pub(crate) struct TonePlayer {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(test)]
impl TonePlayer {
    pub fn start(freq: f64, amp: f32) -> Result<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        let thread = std::thread::spawn(move || {
            let run = || -> Result<()> {
                let mut client = Client::connect("Airhand test tone")?;
                let mut stream = Stream::new(&mut client.context, "Airhand test tone", &SPEC, None)
                    .ok_or_else(|| anyhow!("can't create the playback stream"))?;
                // 50 ms 的播放缓冲
                let tlength = AUDIO_SAMPLE_RATE / 20 * FRAME_BYTES as u32;
                let attr = BufferAttr {
                    maxlength: u32::MAX,
                    tlength,
                    prebuf: u32::MAX,
                    minreq: u32::MAX,
                    fragsize: u32::MAX,
                };
                stream
                    .connect_playback(None, Some(&attr), StreamFlags::ADJUST_LATENCY, None, None)
                    .map_err(|e| anyhow!("can't open the playback stream: {e}"))?;
                client.wait(|_| match stream.get_state() {
                    StreamState::Ready => Some(Ok(())),
                    StreamState::Failed | StreamState::Terminated => {
                        Some(Err(anyhow!("can't open the playback stream")))
                    }
                    _ => None,
                })?;
                let _ = ready_tx.send(Ok(()));
                let mut phase = 0f64;
                let dphase = 2.0 * std::f64::consts::PI * freq / AUDIO_SAMPLE_RATE as f64;
                let mut buf = Vec::new();
                while !s.load(Ordering::Acquire) {
                    client.pump()?;
                    let frames = stream.writable_size().unwrap_or(0) / FRAME_BYTES;
                    if frames > 0 {
                        buf.clear();
                        for _ in 0..frames {
                            let v = (phase.sin() as f32 * amp).to_le_bytes();
                            phase += dphase;
                            buf.extend_from_slice(&v);
                            buf.extend_from_slice(&v);
                        }
                        stream
                            .write(&buf, None, 0, pa::stream::SeekMode::Relative)
                            .map_err(|e| anyhow!("{e}"))?;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = stream.disconnect();
                Ok(())
            };
            if let Err(e) = run() {
                let _ = ready_tx.send(Err(e));
            }
        });
        ready_rx
            .recv()
            .map_err(|_| anyhow!("the playback thread exited"))??;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

#[cfg(test)]
impl Drop for TonePlayer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单频幅度（Goertzel）
    fn goertzel(x: &[f32], freq: f64) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq / AUDIO_SAMPLE_RATE as f64;
        let (mut s1, mut s2) = (0f64, 0f64);
        for &v in x {
            let s = v as f64 + 2.0 * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        (s1 * s1 + s2 * s2 - 2.0 * w.cos() * s1 * s2)
            .max(0.0)
            .sqrt()
            * 2.0
            / x.len().max(1) as f64
    }

    /// 需要 PulseAudio/PipeWire 和一个输出设备。会以 -60 dBFS（几乎听不见）播放 1.5 秒 440 Hz。
    /// `cargo test --lib loopback_captures_tone -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn loopback_captures_tone() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();
        let _tone = TonePlayer::start(440.0, 0.001).expect("播放测试音");
        let mut cap = Loopback::open().expect("打开环回采集");
        assert!(!cap.default_changed());
        let mut pcm = Vec::new();
        let started = Instant::now();
        // 时间戳检查：每次读到的数据，结束时刻应当接近"现在"
        let (mut stamps, mut worst) = (0, 0f64);
        while started.elapsed() < Duration::from_millis(1500) {
            let before = pcm.len();
            if let Some(t) = cap.read(&mut pcm).unwrap() {
                let end = t + Duration::from_secs_f64(
                    (pcm.len() - before) as f64 / 2.0 / AUDIO_SAMPLE_RATE as f64,
                );
                let now = Instant::now();
                let off = now
                    .saturating_duration_since(end)
                    .max(end.saturating_duration_since(now));
                worst = worst.max(off.as_secs_f64() * 1000.0);
                stamps += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("带时间戳的读取 {stamps} 次，数据结束时刻与当前时间最多相差 {worst:.1} ms");
        assert!(stamps > 50, "服务器没有提供时序信息");
        assert!(worst < 60.0, "采集时间戳偏差过大: {worst:.1} ms");
        let secs = pcm.len() as f64 / 2.0 / AUDIO_SAMPLE_RATE as f64;
        assert!(secs > 1.0, "采集到的数据太少: {secs:.2} 秒");
        let left: Vec<f32> = pcm[pcm.len() / 5..].iter().step_by(2).copied().collect();
        let at = |f: f64| goertzel(&left, f);
        let (tone, other) = (at(440.0), (at(300.0) + at(620.0) + at(1000.0)) / 3.0);
        println!("采集 {secs:.2} 秒，440 Hz 幅度 {tone:.2e}，其它频率 {other:.2e}");
        assert!(tone > 1e-6 && tone > other * 10.0, "没有采集到测试音");
    }
}
