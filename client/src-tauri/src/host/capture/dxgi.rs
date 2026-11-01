//! Windows DXGI Desktop Duplication 采集。
//!
//! * 画面由 GPU 合成后直接复制到 staging 纹理，只在桌面有变化时才产生新帧；
//! * 优先使用 `DuplicateOutput1`（支持 HDR 桌面自动转换为 BGRA8）。

use std::sync::Arc;
use std::time::Duration;

use airhand_proto::session::{CursorShape, DisplayInfo};
use anyhow::{Context, Result, anyhow, bail};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO,
    DXGI_OUTDUPL_POINTER_SHAPE_INFO, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
    DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_OUTPUT_DESC, IDXGIAdapter1, IDXGIFactory1,
    IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
use windows::core::Interface;

use super::{CaptureUpdate, Capturer, FramePool};

struct Output {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput1,
    desc: DXGI_OUTPUT_DESC,
}

fn outputs() -> Result<Vec<Output>> {
    let mut list = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().context("CreateDXGIFactory1")?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            ai += 1;
            let mut oi = 0;
            while let Ok(output) = adapter.EnumOutputs(oi) {
                oi += 1;
                let Ok(desc) = output.GetDesc() else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let Ok(output) = output.cast::<IDXGIOutput1>() else {
                    continue;
                };
                list.push(Output {
                    adapter: adapter.clone(),
                    output,
                    desc,
                });
            }
        }
    }
    // 主显示器排第一，其余按桌面坐标从左到右
    list.sort_by_key(|o| {
        let r = o.desc.DesktopCoordinates;
        (!is_primary(&o.desc), r.left, r.top)
    });
    Ok(list)
}

fn is_primary(desc: &DXGI_OUTPUT_DESC) -> bool {
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        if GetMonitorInfoW(desc.Monitor, &mut mi).as_bool() {
            return mi.dwFlags & 1 != 0; // MONITORINFOF_PRIMARY
        }
    }
    let r = desc.DesktopCoordinates;
    r.left == 0 && r.top == 0
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    Ok(outputs()?
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let r = o.desc.DesktopCoordinates;
            let raw = String::from_utf16_lossy(&o.desc.DeviceName);
            let name = raw
                .trim_end_matches('\0')
                .trim_start_matches(r"\\.\")
                .to_string();
            DisplayInfo {
                index: i as u8,
                name,
                x: r.left,
                y: r.top,
                width: (r.right - r.left) as u32,
                height: (r.bottom - r.top) as u32,
                primary: is_primary(&o.desc),
            }
        })
        .collect())
}

pub struct DxgiCapturer {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    first: bool,
    shape_buf: Vec<u8>,
}

// D3D11 设备只在采集线程内使用
unsafe impl Send for DxgiCapturer {}

impl DxgiCapturer {
    pub fn open(index: u8) -> Result<Self> {
        let outs = outputs()?;
        let o = outs
            .get(index as usize)
            .ok_or_else(|| anyhow!("display {index} doesn't exist"))?;
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            let levels: [D3D_FEATURE_LEVEL; 4] = [
                D3D_FEATURE_LEVEL_11_1,
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_10_0,
            ];
            D3D11CreateDevice(
                &o.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .context("D3D11CreateDevice")?;
            let device = device.ok_or_else(|| anyhow!("D3D11 device is null"))?;
            let ctx = ctx.ok_or_else(|| anyhow!("D3D11 context is null"))?;

            let dup = match o.output.cast::<IDXGIOutput5>() {
                Ok(o5) => match o5.DuplicateOutput1(&device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM]) {
                    Ok(d) => d,
                    Err(_) => o
                        .output
                        .DuplicateOutput(&device)
                        .context("DuplicateOutput")?,
                },
                Err(_) => o
                    .output
                    .DuplicateOutput(&device)
                    .context("DuplicateOutput")?,
            };
            let desc = dup.GetDesc();
            if desc.ModeDesc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
                let format = desc.ModeDesc.Format;
                bail!("unsupported desktop pixel format: {format:?}");
            }
            Ok(Self {
                device,
                ctx,
                dup,
                staging: None,
                first: true,
                shape_buf: Vec::new(),
            })
        }
    }

    fn staging(&mut self, w: u32, h: u32) -> Result<ID3D11Texture2D> {
        if let Some((t, sw, sh)) = &self.staging
            && *sw == w
            && *sh == h
        {
            return Ok(t.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut tex = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut tex)) }
            .context("CreateTexture2D")?;
        let tex = tex.ok_or_else(|| anyhow!("staging texture is null"))?;
        self.staging = Some((tex.clone(), w, h));
        Ok(tex)
    }
}

impl Capturer for DxgiCapturer {
    fn next(&mut self, timeout: Duration, pool: &mut FramePool) -> Result<CaptureUpdate> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        if let Err(e) = unsafe {
            self.dup
                .AcquireNextFrame(timeout.as_millis() as u32, &mut info, &mut res)
        } {
            if e.code() == DXGI_ERROR_WAIT_TIMEOUT {
                return Ok(CaptureUpdate::default());
            }
            return Err(anyhow!("AcquireNextFrame: {e}"));
        }
        let mut update = CaptureUpdate::default();

        // 光标：位置由控制端本地光标负责，这里只同步可见性与形状
        if info.LastMouseUpdateTime != 0 {
            update.cursor_visible = Some(info.PointerPosition.Visible.as_bool());
        }
        if info.PointerShapeBufferSize > 0 {
            update.cursor_shape = self.pointer_shape(info.PointerShapeBufferSize);
        }

        // 仅鼠标移动（画面未变）时 LastPresentTime 为 0
        let copied = if info.LastPresentTime == 0 && !self.first {
            Ok(None)
        } else {
            (|| -> Result<Option<(ID3D11Texture2D, u32, u32)>> {
                let tex: ID3D11Texture2D = res
                    .take()
                    .ok_or_else(|| anyhow!("desktop resource is null"))?
                    .cast()?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                unsafe { tex.GetDesc(&mut desc) };
                let staging = self.staging(desc.Width, desc.Height)?;
                unsafe { self.ctx.CopyResource(&staging, &tex) };
                Ok(Some((staging, desc.Width, desc.Height)))
            })()
        };
        drop(res);
        // 尽早释放，让 DWM 继续合成
        let _ = unsafe { self.dup.ReleaseFrame() };
        let Some((staging, w, h)) = copied? else {
            return Ok(update);
        };
        self.first = false;

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.ctx
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .context("Map")?;
        let mut frame = pool.checkout(w, h);
        {
            let f = Arc::get_mut(&mut frame).expect("unique frame");
            // 采集时间用画面真正呈现的时刻（LastPresentTime），比"复制完成的时刻"更准，
            // 声音包也用同一个时钟（QPC），音画同步就靠这两个时间戳对齐
            if let Some(t) = crate::host::clock::from_qpc(info.LastPresentTime) {
                f.captured = t;
            }
            let row = w as usize * 4;
            let pitch = mapped.RowPitch as usize;
            let src = mapped.pData as *const u8;
            unsafe {
                if pitch == row {
                    std::ptr::copy_nonoverlapping(src, f.data.as_mut_ptr(), row * h as usize);
                } else {
                    for y in 0..h as usize {
                        std::ptr::copy_nonoverlapping(
                            src.add(y * pitch),
                            f.data.as_mut_ptr().add(y * row),
                            row,
                        );
                    }
                }
            }
        }
        unsafe { self.ctx.Unmap(&staging, 0) };
        pool.keep(&frame);
        update.frame = Some(frame);
        Ok(update)
    }
}

impl DxgiCapturer {
    fn pointer_shape(&mut self, size: u32) -> Option<CursorShape> {
        self.shape_buf.resize(size as usize, 0);
        let mut required = 0u32;
        let mut si = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
        unsafe {
            self.dup
                .GetFramePointerShape(
                    size,
                    self.shape_buf.as_mut_ptr().cast(),
                    &mut required,
                    &mut si,
                )
                .ok()?;
        }
        convert_pointer(&self.shape_buf, &si)
    }
}

/// DXGI 光标形状 → RGBA。
fn convert_pointer(buf: &[u8], si: &DXGI_OUTDUPL_POINTER_SHAPE_INFO) -> Option<CursorShape> {
    let (w, pitch) = (si.Width as usize, si.Pitch as usize);
    let ty = si.Type as i32;
    let h = if ty == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 {
        si.Height as usize / 2
    } else {
        si.Height as usize
    };
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let mut rgba = vec![0u8; w * h * 4];
    if ty == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 {
        // AND 掩码在上半部分，XOR 掩码在下半部分，各 1bpp
        if buf.len() < pitch * h * 2 {
            return None;
        }
        let bit = |row: usize, x: usize| (buf[row * pitch + x / 8] >> (7 - (x % 8))) & 1;
        let mut invert = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let (and, xor) = (bit(y, x), bit(y + h, x));
                let o = (y * w + x) * 4;
                match (and, xor) {
                    (0, 0) => rgba[o..o + 4].copy_from_slice(&[0, 0, 0, 255]),
                    (0, _) => rgba[o..o + 4].copy_from_slice(&[255, 255, 255, 255]),
                    (_, 0) => {}
                    _ => invert[y * w + x] = true,
                }
            }
        }
        outline_inverted(&mut rgba, &invert, w, h);
    } else {
        if buf.len() < pitch * h {
            return None;
        }
        let masked = ty == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0;
        let mut invert = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = y * pitch + x * 4;
                let (b, g, r, a) = (buf[i], buf[i + 1], buf[i + 2], buf[i + 3]);
                let o = (y * w + x) * 4;
                if masked {
                    // alpha=0：直接显示颜色；alpha=0xFF：与屏幕异或（颜色为 0 则透明）
                    if a == 0 {
                        rgba[o..o + 4].copy_from_slice(&[r, g, b, 255]);
                    } else if r | g | b != 0 {
                        invert[y * w + x] = true;
                    }
                } else {
                    rgba[o..o + 4].copy_from_slice(&[r, g, b, a]);
                }
            }
        }
        if masked {
            outline_inverted(&mut rgba, &invert, w, h);
        }
    }
    Some(CursorShape {
        width: w as u16,
        height: h as u16,
        hot_x: si.HotSpot.x.clamp(0, w as i32 - 1) as u16,
        hot_y: si.HotSpot.y.clamp(0, h as i32 - 1) as u16,
        rgba,
    })
}

/// "反色"像素无法用普通图片表示：画成黑色并加白色描边，在任何背景上都清晰可见。
fn outline_inverted(rgba: &mut [u8], invert: &[bool], w: usize, h: usize) {
    for y in 0..h {
        for x in 0..w {
            if !invert[y * w + x] {
                continue;
            }
            let o = (y * w + x) * 4;
            rgba[o..o + 4].copy_from_slice(&[0, 0, 0, 255]);
            for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let n = ny as usize * w + nx as usize;
                if !invert[n] && rgba[n * 4 + 3] == 0 {
                    rgba[n * 4..n * 4 + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }
}
