//! 视频编码。
//!
//! * **硬件编码**（优先）：Windows Media Foundation 硬件 MFT（NVENC / Quick Sync / AMF），
//!   支持 H.264 / H.265 / AV1，几乎不占用 CPU；
//! * **软件编码**（兜底）：OpenH264 屏幕内容模式。
//!
//! 共同的预处理：BGRA →（SIMD 缩放）→（SIMD 色彩转换）YUV 4:2:0。
//! 被控端只在画面变化时才编码，静止画面完全不占带宽。

#[cfg(windows)]
mod mf;
mod software;

use std::sync::OnceLock;

use airhand_proto::session::{Codec, EncoderInfo, QualitySettings};
use anyhow::{Context, Result, anyhow};
use fast_image_resize as fir;
use tracing::info;
#[cfg(windows)]
use tracing::warn;

use super::capture::Frame;

/// 硬件编码器普遍支持的最大分辨率（同时也是 OpenH264 Level 5.2 的上限）
const MAX_LONG: u32 = 3840;
const MAX_SHORT: u32 = 2160;

pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub key: bool,
}

pub trait VideoEncoder {
    fn info(&self) -> &EncoderInfo;
    /// 运行时调整码率（不重建编码器、不产生关键帧）。
    fn set_bitrate(&mut self, kbps: u32);
    /// 编码一帧 BGRA（尺寸已经等于输出尺寸，stride = width * 4）。
    fn encode(&mut self, bgra: &[u8], force_key: bool) -> Result<Option<EncodedFrame>>;
}

/// 根据短边上限计算输出分辨率（偶数、不超过编码器上限）。`max_short = 0` 表示不限制。
pub fn target_size(src_w: u32, src_h: u32, max_short: u16) -> (u32, u32) {
    let (mut w, mut h) = (src_w as f64, src_h as f64);
    let short = w.min(h);
    if max_short > 0 && short > max_short as f64 {
        let f = max_short as f64 / short;
        w *= f;
        h *= f;
    }
    let (max_w, max_h) = if w >= h {
        (MAX_LONG, MAX_SHORT)
    } else {
        (MAX_SHORT, MAX_LONG)
    };
    let f = (max_w as f64 / w).min(max_h as f64 / h).min(1.0);
    w *= f;
    h *= f;
    let even = |v: f64| ((v.round() as u32) & !1).max(64);
    (even(w), even(h))
}

/// 被控端支持的编码格式（H.264 永远可用，其余取决于是否有硬件编码器）。
pub fn available_codecs() -> Vec<Codec> {
    static CACHE: OnceLock<Vec<Codec>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            #[allow(unused_mut)]
            let mut list = vec![Codec::H264];
            #[cfg(windows)]
            for c in [Codec::H265, Codec::AV1] {
                if mf::has_hardware(c) {
                    list.push(c);
                }
            }
            info!(?list, "available codecs");
            list
        })
        .clone()
}

/// 缩放 + 编码。持有可复用的缩放缓冲区。
pub struct Encoder {
    inner: Box<dyn VideoEncoder>,
    resizer: fir::Resizer,
    scaled: Vec<u8>,
    src_w: u32,
    src_h: u32,
    fps: u8,
    max_short: u16,
    codec: Codec,
}

impl Encoder {
    /// 创建编码器：按"请求的格式硬件 → H.264 硬件 → OpenH264"顺序尝试。
    pub fn new(
        src_w: u32,
        src_h: u32,
        s: &QualitySettings,
        kbps: u32,
        allow_hw: bool,
    ) -> Result<Self> {
        let (w, h) = target_size(src_w, src_h, s.max_short);
        #[allow(unused_mut)]
        let mut inner: Option<Box<dyn VideoEncoder>> = None;

        #[cfg(windows)]
        if allow_hw {
            let mut order = vec![s.codec];
            if s.codec != Codec::H264 {
                order.push(Codec::H264);
            }
            for codec in order {
                if codec != Codec::H264 && !available_codecs().contains(&codec) {
                    continue;
                }
                match mf::MfEncoder::new(codec, w, h, s.fps, kbps) {
                    Ok(e) => {
                        inner = Some(Box::new(e));
                        break;
                    }
                    Err(e) => warn!(?codec, "hardware encoder unavailable: {e:#}"),
                }
            }
        }
        #[cfg(not(windows))]
        let _ = allow_hw;

        let inner = match inner {
            Some(e) => e,
            None => {
                Box::new(software::OpenH264Encoder::new(w, h, s.fps, kbps).context("OpenH264")?)
            }
        };
        let i = inner.info();
        info!(encoder = %i.name, hw = i.hardware, codec = ?i.codec, w, h, kbps, "encoder created");
        Ok(Self {
            inner,
            resizer: fir::Resizer::new(),
            scaled: Vec::new(),
            src_w,
            src_h,
            fps: s.fps,
            max_short: s.max_short,
            codec: s.codec,
        })
    }

    /// 源尺寸或需要重建编码器的参数发生变化时返回 false。
    pub fn compatible(&self, src_w: u32, src_h: u32, s: &QualitySettings) -> bool {
        self.src_w == src_w
            && self.src_h == src_h
            && self.fps == s.fps
            && self.max_short == s.max_short
            && self.codec == s.codec
    }

    pub fn info(&self) -> &EncoderInfo {
        self.inner.info()
    }

    pub fn set_bitrate(&mut self, kbps: u32) {
        self.inner.set_bitrate(kbps);
    }

    pub fn encode(&mut self, frame: &Frame, force_key: bool) -> Result<Option<EncodedFrame>> {
        let (w, h) = (self.inner.info().width, self.inner.info().height);
        // 尺寸一致（或仅因奇数尺寸多出 1 像素）时直接使用原图，不缩放
        let crop_only =
            frame.width >= w && frame.height >= h && frame.width - w <= 1 && frame.height - h <= 1;
        let bgra: &[u8] = if crop_only && frame.width == w {
            &frame.data[..(w * h * 4) as usize]
        } else if crop_only {
            // 宽度为奇数：逐行裁掉最后一列
            self.scaled.resize((w * h * 4) as usize, 0);
            let (src_row, dst_row) = (frame.width as usize * 4, w as usize * 4);
            for y in 0..h as usize {
                self.scaled[y * dst_row..(y + 1) * dst_row]
                    .copy_from_slice(&frame.data[y * src_row..y * src_row + dst_row]);
            }
            &self.scaled
        } else {
            let src = fir::images::ImageRef::new(
                frame.width,
                frame.height,
                &frame.data,
                fir::PixelType::U8x4,
            )
            .map_err(|e| anyhow!("invalid source image: {e}"))?;
            self.scaled.resize((w * h * 4) as usize, 0);
            let mut dst =
                fir::images::Image::from_slice_u8(w, h, &mut self.scaled, fir::PixelType::U8x4)
                    .map_err(|e| anyhow!("invalid target image: {e}"))?;
            // 缩小时使用双线性卷积（对文字边缘比最近邻清晰得多，且 SIMD 下 4K→1080p 只需数毫秒）
            let opts = fir::ResizeOptions::new()
                .resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Bilinear))
                .use_alpha(false);
            self.resizer
                .resize(&src, &mut dst, &opts)
                .context("scaling failed")?;
            &self.scaled
        };
        self.inner.encode(bgra, force_key)
    }
}

/// 在 Annex-B 码流中查找 NAL 类型（用于判断是否包含参数集 / 关键帧）。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn annexb_nal_types(data: &[u8], hevc: bool) -> impl Iterator<Item = u8> + '_ {
    let mut i = 0usize;
    std::iter::from_fn(move || {
        while i + 3 < data.len() {
            if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
                let b = data[i + 3];
                i += 3;
                return Some(if hevc { (b >> 1) & 0x3f } else { b & 0x1f });
            }
            i += 1;
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(target_size(1920, 1080, 0), (1920, 1080));
        assert_eq!(target_size(1921, 1081, 0), (1920, 1080));
        assert_eq!(target_size(5120, 2880, 0), (3840, 2160));
        assert_eq!(target_size(1440, 2560, 0), (1440, 2560));
        assert_eq!(target_size(3840, 2160, 1080), (1920, 1080));
        assert_eq!(target_size(2560, 1600, 1080), (1728, 1080));
        assert_eq!(target_size(1280, 720, 1080), (1280, 720));
        assert_eq!(target_size(1080, 1920, 720), (720, 1280));
    }

    #[test]
    fn nal_scan() {
        let d = [0, 0, 0, 1, 0x67, 9, 0, 0, 1, 0x68, 0, 0, 0, 1, 0x65, 1];
        assert_eq!(
            annexb_nal_types(&d, false).collect::<Vec<_>>(),
            vec![7, 8, 5]
        );
    }

    fn synthetic(w: u32, h: u32) -> Frame {
        let mut data = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                data[i] = (x % 256) as u8;
                data[i + 1] = (y % 256) as u8;
                data[i + 2] = ((x / 8 + y / 8) % 2 * 255) as u8;
                data[i + 3] = 255;
            }
        }
        Frame {
            width: w,
            height: h,
            data,
            captured: std::time::Instant::now(),
        }
    }

    fn check(codec: Codec, allow_hw: bool) -> Option<String> {
        let frame = synthetic(1281, 721);
        let s = QualitySettings {
            codec,
            max_short: 0,
            ..Default::default()
        };
        let mut enc = Encoder::new(frame.width, frame.height, &s, 3000, allow_hw).unwrap();
        if enc.info().codec != codec {
            return None;
        }
        assert_eq!((enc.info().width, enc.info().height), (1280, 720));
        let mut got_key = false;
        for i in 0..10 {
            if let Some(f) = enc.encode(&frame, i == 0).unwrap() {
                got_key |= f.key;
                assert!(!f.data.is_empty());
            }
        }
        enc.set_bitrate(1000);
        let _ = enc.encode(&frame, true).unwrap();
        assert!(got_key, "{codec:?} 没有产生关键帧");
        Some(enc.info().name.clone())
    }

    #[test]
    fn software_h264() {
        assert_eq!(check(Codec::H264, false).as_deref(), Some("OpenH264"));
    }

    #[cfg(windows)]
    #[test]
    fn hardware_all() {
        for c in available_codecs() {
            match check(c, true) {
                Some(name) => println!("{c:?}: {name}"),
                None => println!("{c:?}: 回退到其他编码器"),
            }
        }
    }
}
