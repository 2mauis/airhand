//! WASAPI 环回采集：录下默认输出设备正在播放的声音（即"系统声音"，不包括麦克风）。
//!
//! 优先让系统把混音格式自动转换为 48 kHz 立体声 float（Windows 10 起支持）；
//! 不支持时按设备混音格式采集，自己做声道下混与线性重采样。

use anyhow::{Context, Result, bail};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::core::GUID;

use std::time::{Duration, Instant};

use airhand_proto::media::AUDIO_SAMPLE_RATE;

/// 共享模式缓冲 200 ms：采集线程偶尔被耽搁也不会丢数据
const BUFFER_HNS: i64 = 2_000_000;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_IEEE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const SPEAKER_FRONT_LEFT: u32 = 0x1;
const SPEAKER_FRONT_RIGHT: u32 = 0x2;

pub struct Loopback {
    enumerator: IMMDeviceEnumerator,
    device_id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    conv: Converter,
}

impl Loopback {
    pub fn open() -> Result<Self> {
        Self::open_with(true)
    }

    /// `auto_convert=false` 时直接按设备混音格式采集（测试兜底路径用）。
    fn open_with(auto_convert: bool) -> Result<Self> {
        unsafe {
            // 每个线程都需要初始化 COM（重复调用是安全的）
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .context("can't create MMDeviceEnumerator")?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .context("no default audio output device")?;
            let device_id = device_id(&device)?;

            let want = float_stereo(AUDIO_SAMPLE_RATE);
            let want_ptr = (&raw const want).cast::<WAVEFORMATEX>();
            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK;
            let auto = flags
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let init = if auto_convert {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    auto,
                    BUFFER_HNS,
                    0,
                    want_ptr,
                    None,
                )
            } else {
                Err(windows::core::Error::empty())
            };
            let (client, conv) = match init {
                Ok(()) => (client, Converter::new(Format::native())),
                Err(e) => {
                    tracing::debug!(
                        "system format conversion unavailable ({e}), using the device mix format"
                    );
                    // 初始化失败后这个 IAudioClient 不能再用，重新激活一个
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    let mix = client.GetMixFormat()?;
                    let fmt = Format::parse(mix);
                    let init = client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        flags,
                        BUFFER_HNS,
                        0,
                        mix,
                        None,
                    );
                    CoTaskMemFree(Some(mix as *const _));
                    let fmt = fmt?;
                    init.context("can't initialize audio capture")?;
                    (client, Converter::new(fmt))
                }
            };
            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            tracing::info!(
                rate = conv.fmt.rate,
                channels = conv.fmt.channels,
                kind = ?conv.fmt.kind,
                "audio capture opened"
            );
            Ok(Self {
                enumerator,
                device_id,
                client,
                capture,
                conv,
            })
        }
    }

    /// 读出当前已采集的全部数据（48 kHz 立体声交错 float），追加到 `out`。没有数据时立即返回。
    ///
    /// 返回追加的第一个采样的采集时间（系统给出的 QPC 时间戳；不可用时为 `None`）。
    pub fn read(&mut self, out: &mut Vec<f32>) -> Result<Option<Instant>> {
        let base = out.len();
        let mut first: Option<Instant> = None;
        unsafe {
            loop {
                let n = self.capture.GetNextPacketSize()?;
                if n == 0 {
                    return Ok(first);
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                // 用这一包的时间戳倒推本次读取的起点（前面的包可能没有可用的时间戳）
                if first.is_none() && flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 == 0 {
                    let before = Duration::from_secs_f64(
                        (out.len() - base) as f64 / 2.0 / AUDIO_SAMPLE_RATE as f64,
                    );
                    first =
                        crate::host::clock::from_qpc_100ns(qpc).and_then(|t| t.checked_sub(before));
                }
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                let bytes = if silent {
                    None
                } else {
                    Some(std::slice::from_raw_parts(
                        data,
                        frames as usize * self.conv.fmt.block_align,
                    ))
                };
                self.conv.push(bytes, frames as usize, out);
                self.capture.ReleaseBuffer(frames)?;
            }
        }
    }

    /// 默认输出设备是否已经换了（或者没了）。
    pub fn default_changed(&self) -> bool {
        unsafe {
            match self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) {
                Ok(d) => device_id(&d).map_or(true, |id| id != self.device_id),
                Err(_) => true,
            }
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

fn device_id(device: &IMMDevice) -> Result<String> {
    unsafe {
        let p = device.GetId()?;
        let id = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        Ok(id)
    }
}

fn float_stereo(rate: u32) -> WAVEFORMATEXTENSIBLE {
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE,
            nChannels: 2,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: 32,
        },
        dwChannelMask: SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT,
        SubFormat: SUBTYPE_IEEE_FLOAT,
    }
}

// ───────────────────────────── 格式转换 ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum SampleKind {
    F32,
    I16,
    I24,
    I32,
}

#[derive(Debug, Clone, Copy)]
struct Format {
    kind: SampleKind,
    channels: usize,
    rate: u32,
    block_align: usize,
}

impl Format {
    fn native() -> Self {
        Self {
            kind: SampleKind::F32,
            channels: 2,
            rate: AUDIO_SAMPLE_RATE,
            block_align: 8,
        }
    }

    unsafe fn parse(p: *const WAVEFORMATEX) -> Result<Self> {
        let f = unsafe { std::ptr::read_unaligned(p) };
        let bits = f.wBitsPerSample;
        let float = match f.wFormatTag {
            WAVE_FORMAT_EXTENSIBLE if f.cbSize >= 22 => {
                let x = unsafe { std::ptr::read_unaligned(p as *const WAVEFORMATEXTENSIBLE) };
                let sub = x.SubFormat;
                if sub == SUBTYPE_IEEE_FLOAT {
                    true
                } else if sub == SUBTYPE_PCM {
                    false
                } else {
                    bail!("unsupported mix format {sub:?}");
                }
            }
            WAVE_FORMAT_IEEE_FLOAT => true,
            1 => false,
            tag => bail!("unsupported mix format 0x{tag:x}"),
        };
        let kind = match (float, bits) {
            (true, 32) => SampleKind::F32,
            (false, 16) => SampleKind::I16,
            (false, 24) => SampleKind::I24,
            (false, 32) => SampleKind::I32,
            _ => bail!("unsupported sample format ({bits} bits, float={float})"),
        };
        let channels = f.nChannels as usize;
        let block_align = f.nBlockAlign as usize;
        if channels == 0 || f.nSamplesPerSec == 0 || block_align < channels * bits as usize / 8 {
            bail!("invalid mix format");
        }
        Ok(Self {
            kind,
            channels,
            rate: f.nSamplesPerSec,
            block_align,
        })
    }
}

/// 任意 PCM → 48 kHz 立体声 float。
struct Converter {
    fmt: Format,
    /// 输入采样率 / 48000
    step: f64,
    /// 下一个输出采样在输入时间轴上的位置（0 = 上一块的最后一帧）
    pos: f64,
    prev: [f32; 2],
    tmp: Vec<[f32; 2]>,
}

impl Converter {
    fn new(fmt: Format) -> Self {
        Self {
            fmt,
            step: fmt.rate as f64 / AUDIO_SAMPLE_RATE as f64,
            pos: 1.0,
            prev: [0.0; 2],
            tmp: Vec::new(),
        }
    }

    fn sample(&self, b: &[u8]) -> f32 {
        match self.fmt.kind {
            SampleKind::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            SampleKind::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            SampleKind::I24 => {
                (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
            }
            SampleKind::I32 => {
                i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0
            }
        }
    }

    /// 一帧多声道 → 立体声（按 WAVE 声道顺序：FL FR FC LFE BL BR SL SR）。
    fn stereo(&self, frame: &[u8]) -> [f32; 2] {
        let size = match self.fmt.kind {
            SampleKind::I16 => 2,
            SampleKind::I24 => 3,
            _ => 4,
        };
        let ch = |i: usize| self.sample(&frame[i * size..]);
        match self.fmt.channels {
            1 => {
                let m = ch(0);
                [m, m]
            }
            2 => [ch(0), ch(1)],
            n => {
                const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
                let (mut l, mut r, mut norm) = (ch(0), ch(1), 1.0);
                if n >= 3 {
                    let c = ch(2) * K;
                    l += c;
                    r += c;
                    norm += K;
                }
                if n >= 6 {
                    l += ch(4) * K;
                    r += ch(5) * K;
                    norm += K;
                }
                if n >= 8 {
                    l += ch(6) * K;
                    r += ch(7) * K;
                    norm += K;
                }
                [l / norm, r / norm]
            }
        }
    }

    /// `data` 为 `None` 表示这一段是静音。
    fn push(&mut self, data: Option<&[u8]>, frames: usize, out: &mut Vec<f32>) {
        if frames == 0 {
            return;
        }
        let native = self.fmt.kind == SampleKind::F32
            && self.fmt.channels == 2
            && self.fmt.rate == AUDIO_SAMPLE_RATE;
        if native {
            match data {
                Some(b) => out.extend(b.as_chunks::<4>().0.iter().map(|s| f32::from_le_bytes(*s))),
                None => out.resize(out.len() + frames * 2, 0.0),
            }
            return;
        }
        let mut tmp = std::mem::take(&mut self.tmp);
        tmp.clear();
        match data {
            Some(b) => tmp.extend(
                b.chunks_exact(self.fmt.block_align)
                    .take(frames)
                    .map(|f| self.stereo(f)),
            ),
            None => tmp.resize(frames, [0.0; 2]),
        }
        if self.fmt.rate == AUDIO_SAMPLE_RATE {
            out.extend(tmp.iter().flatten());
        } else {
            // 线性插值重采样：位置 0 是上一块的最后一帧，1..=n 是本块
            let n = tmp.len();
            let at = |i: usize| if i == 0 { self.prev } else { tmp[i - 1] };
            loop {
                let i = self.pos as usize;
                if i >= n {
                    break;
                }
                let f = (self.pos - i as f64) as f32;
                let (a, b) = (at(i), at(i + 1));
                out.push(a[0] + (b[0] - a[0]) * f);
                out.push(a[1] + (b[1] - a[1]) * f);
                self.pos += self.step;
            }
            self.pos -= n as f64;
            self.prev = tmp[n - 1];
        }
        self.tmp = tmp;
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
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    let en: IMMDeviceEnumerator =
                        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
                    let device = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
                    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
                    let fmt = float_stereo(AUDIO_SAMPLE_RATE);
                    let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
                    let fmt = (&raw const fmt).cast::<WAVEFORMATEX>();
                    client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, fmt, None)?;
                    let size = client.GetBufferSize()?;
                    let render: IAudioRenderClient = client.GetService()?;
                    client.Start()?;
                    let _ = ready_tx.send(Ok(()));
                    let mut phase = 0f64;
                    let dphase = 2.0 * std::f64::consts::PI * freq / AUDIO_SAMPLE_RATE as f64;
                    while !s.load(Ordering::Acquire) {
                        let avail = size - client.GetCurrentPadding()?;
                        if avail > 0 {
                            let p = render.GetBuffer(avail)? as *mut f32;
                            let buf = std::slice::from_raw_parts_mut(p, avail as usize * 2);
                            for f in buf.as_chunks_mut::<2>().0 {
                                let v = phase.sin() as f32 * amp;
                                phase += dphase;
                                f[0] = v;
                                f[1] = v;
                            }
                            render.ReleaseBuffer(avail, 0)?;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let _ = client.Stop();
                    Ok(())
                }
            };
            if let Err(e) = run() {
                let _ = ready_tx.send(Err(e));
            }
        });
        ready_rx.recv().context("the playback thread exited")??;
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

    fn pcm16(frames: &[[i16; 2]]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|f| f.iter().flat_map(|s| s.to_le_bytes()))
            .collect()
    }

    #[test]
    fn convert_i16_44k_to_48k() {
        let fmt = Format {
            kind: SampleKind::I16,
            channels: 2,
            rate: 44_100,
            block_align: 4,
        };
        let mut c = Converter::new(fmt);
        let mut out = Vec::new();
        // 1 秒直流信号，分 100 块送入
        let block = pcm16(&[[16384, -16384]; 441]);
        for _ in 0..100 {
            c.push(Some(&block), 441, &mut out);
        }
        let frames = out.len() / 2;
        assert!((47_990..=48_000).contains(&frames), "输出帧数 {frames}");
        assert!(
            out.as_chunks::<2>()
                .0
                .iter()
                .all(|f| (f[0] - 0.5).abs() < 1e-4 && (f[1] + 0.5).abs() < 1e-4)
        );
    }

    #[test]
    fn convert_surround_downmix() {
        // 5.1 float：只有中置声道有声音 → 左右各一半左右
        let fmt = Format {
            kind: SampleKind::F32,
            channels: 6,
            rate: 48_000,
            block_align: 24,
        };
        let mut c = Converter::new(fmt);
        let frame: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let mut out = Vec::new();
        c.push(Some(&frame), 1, &mut out);
        assert_eq!(out.len(), 2);
        assert!(
            (out[0] - out[1]).abs() < 1e-6 && out[0] > 0.2 && out[0] <= 1.0,
            "{out:?}"
        );
        // 静音段
        c.push(None, 10, &mut out);
        assert_eq!(out.len(), 22);
    }

    /// 需要声音输出设备。会以 -60 dBFS（几乎听不见）播放 1.5 秒 440 Hz。
    /// `cargo test --lib loopback_captures_tone -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn loopback_captures_tone() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("debug")
            .try_init();
        let _tone = TonePlayer::start(440.0, 0.001).expect("播放测试音");
        // 系统格式转换 + 自己转换设备混音格式（兜底路径）都要能用
        for auto_convert in [true, false] {
            println!("auto_convert = {auto_convert}");
            capture_tone(Loopback::open_with(auto_convert).expect("打开环回采集"));
        }
    }

    fn capture_tone(mut cap: Loopback) {
        let mut pcm = Vec::new();
        let started = std::time::Instant::now();
        // 时间戳检查：每次读到的数据，结束时刻应当接近"现在"（环回采集只有一个设备周期的延迟）
        let (mut stamps, mut worst) = (0, 0f64);
        while started.elapsed() < std::time::Duration::from_millis(1500) {
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
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        println!("带时间戳的读取 {stamps} 次，数据结束时刻与当前时间最多相差 {worst:.1} ms");
        assert!(stamps > 50, "系统没有提供采集时间戳");
        assert!(worst < 60.0, "采集时间戳偏差过大: {worst:.1} ms");
        let secs = pcm.len() as f64 / 2.0 / AUDIO_SAMPLE_RATE as f64;
        assert!(secs > 1.0, "采集到的数据太少: {secs:.2} 秒");
        // 跳过开头 300 ms（播放刚启动）。环回采集的是音量调节之后的声音，绝对电平取决于系统音量，
        // 所以检查 440 Hz 分量相对其它频率是否突出（本机若在放别的声音也不影响判断）
        let left: Vec<f32> = pcm[pcm.len() / 5..].iter().step_by(2).copied().collect();
        let at = |f: f64| goertzel(&left, f);
        let (tone, other) = (at(440.0), (at(300.0) + at(620.0) + at(1000.0)) / 3.0);
        println!("采集 {secs:.2} 秒，440 Hz 幅度 {tone:.2e}，其它频率 {other:.2e}");
        assert!(tone > 1e-6 && tone > other * 10.0, "没有采集到测试音");
    }
}
