//! Linux 被控端的 X11 公共部分：连接、显示器枚举、Wayland 检测。
//!
//! 用 x11rb 的纯 Rust 连接（不依赖 libX11/libxcb）。采集线程、每个会话的输入注入、
//! 剪贴板监听各用一条独立连接，互不阻塞。

use std::sync::OnceLock;

use airhand_proto::session::{DisplayInfo, Notice};
use anyhow::{Context, Result, anyhow, bail};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, Screen, Window};
use x11rb::rust_connection::RustConnection;

pub struct X11 {
    pub conn: RustConnection,
    screen: usize,
}

impl X11 {
    pub fn connect() -> Result<Self> {
        match x11rb::connect(None) {
            Ok((conn, screen)) => Ok(Self { conn, screen }),
            Err(e) => {
                let why = if std::env::var_os("DISPLAY").is_none() {
                    "DISPLAY is not set".to_string()
                } else {
                    e.to_string()
                };
                let hint = if is_wayland_session() {
                    "; this is a Wayland session, log in with an X11 (Xorg) session instead"
                } else {
                    ""
                };
                bail!("can't connect to the X11 display server ({why}){hint}")
            }
        }
    }

    pub fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen]
    }

    pub fn root(&self) -> Window {
        self.screen().root
    }

    pub fn has_extension(&self, name: &'static str) -> bool {
        matches!(self.conn.extension_information(name), Ok(Some(_)))
    }

    /// 等待服务器处理完之前的所有请求（顺便刷新发送缓冲）。
    pub fn sync(&self) -> Result<()> {
        self.conn.get_input_focus()?.reply()?;
        Ok(())
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
}

/// 被控端运行环境的提示（发给控制端，由控制端按自己的语言显示）：Wayland 桌面下只能看到/操作 X11 程序。
pub fn environment_notice() -> Option<Notice> {
    static WAYLAND: OnceLock<bool> = OnceLock::new();
    let wayland = *WAYLAND.get_or_init(|| {
        // XWayland 会注册 XWAYLAND 扩展：这时 X11 只能看到 X11 程序的窗口，原生 Wayland 窗口是黑的
        let xwayland = X11::connect().is_ok_and(|x| x.has_extension("XWAYLAND"));
        xwayland || is_wayland_session()
    });
    wayland.then_some(Notice::Wayland)
}

/// 一个显示器在根窗口坐标系中的位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub name: String,
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
    pub primary: bool,
}

/// 列出所有显示器：主显示器排第一，其余按坐标从左到右。没有 RandR 1.5 时把整个屏幕当作一个显示器。
pub fn monitors(x: &X11) -> Result<Vec<Monitor>> {
    let (sw, sh) = (x.screen().width_in_pixels, x.screen().height_in_pixels);
    let mut list = Vec::new();
    if x.has_extension(randr::X11_EXTENSION_NAME)
        && let Ok(v) = x.conn.randr_query_version(1, 5)?.reply()
        && (v.major_version, v.minor_version) >= (1, 5)
    {
        for m in x.conn.randr_get_monitors(x.root(), true)?.reply()?.monitors {
            // 超出根窗口的部分无法采集，裁掉
            let (mx, my) = (m.x.max(0), m.y.max(0));
            let w = (m.width as i32).min(sw as i32 - mx as i32);
            let h = (m.height as i32).min(sh as i32 - my as i32);
            if w <= 0 || h <= 0 {
                continue;
            }
            let name = match x.conn.get_atom_name(m.name)?.reply() {
                Ok(r) => String::from_utf8_lossy(&r.name).into_owned(),
                Err(_) => String::new(),
            };
            list.push(Monitor {
                name,
                x: mx,
                y: my,
                width: w as u16,
                height: h as u16,
                primary: m.primary,
            });
        }
    }
    if list.is_empty() {
        list.push(Monitor {
            name: "X11".into(),
            x: 0,
            y: 0,
            width: sw,
            height: sh,
            primary: true,
        });
    }
    list.sort_by_key(|m| (!m.primary, m.x, m.y));
    if !list.iter().any(|m| m.primary) {
        list[0].primary = true;
    }
    Ok(list)
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    let x = X11::connect()?;
    Ok(monitors(&x)?
        .into_iter()
        .enumerate()
        .map(|(i, m)| DisplayInfo {
            index: i as u8,
            name: m.name,
            x: m.x as i32,
            y: m.y as i32,
            width: m.width as u32,
            height: m.height as u32,
            primary: m.primary,
        })
        .collect())
}

/// 第 `index` 个显示器（与 [`displays`] 的顺序一致）。
pub fn monitor(x: &X11, index: u8) -> Result<Monitor> {
    monitors(x)?
        .into_iter()
        .nth(index as usize)
        .ok_or_else(|| anyhow!("display {index} doesn't exist"))
}

/// 等待连接上有数据可读，最多 `timeout`。
pub fn wait_readable(x: &X11, timeout: std::time::Duration) -> Result<()> {
    use std::os::fd::AsRawFd;
    x.conn.flush().context("lost connection to X11")?;
    let mut fds = libc::pollfd {
        fd: x.conn.stream().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    // EINTR 等错误按"超时"处理，调用方会重新检查
    unsafe { libc::poll(&mut fds, 1, ms) };
    Ok(())
}
