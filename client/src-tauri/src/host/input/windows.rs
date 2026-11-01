//! Windows `SendInput` 注入。

use std::collections::HashSet;

use airhand_proto::session::{DisplayInfo, InputEvent, MouseButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use super::keymap::{self, Key};

#[derive(Default)]
pub struct WinInjector {
    keys: HashSet<Key>,
    buttons: HashSet<MouseButton>,
    batch: Vec<INPUT>,
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keybd(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(k: Key, down: bool) -> INPUT {
    let up = if down {
        KEYBD_EVENT_FLAGS(0)
    } else {
        KEYEVENTF_KEYUP
    };
    match k {
        Key::Scan(sc, ext) => {
            let ext = if ext {
                KEYEVENTF_EXTENDEDKEY
            } else {
                KEYBD_EVENT_FLAGS(0)
            };
            keybd(0, sc, KEYEVENTF_SCANCODE | ext | up)
        }
        Key::Vk(vk) => keybd(vk, 0, up),
    }
}

fn button_flags(b: MouseButton, down: bool) -> (MOUSE_EVENT_FLAGS, u32) {
    match (b, down) {
        (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, 1),
        (MouseButton::Back, false) => (MOUSEEVENTF_XUP, 1),
        (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, 2),
        (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, 2),
    }
}

/// 把显示器内的归一化坐标换算为 SendInput 使用的虚拟桌面绝对坐标（0..=65535）。
fn to_absolute(d: &DisplayInfo, x: f32, y: f32) -> (i32, i32) {
    let (vx, vy, vw, vh) = crate::host::dpi_aware(|| unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
        )
    });
    let px = d.x as f64 + x.clamp(0.0, 1.0) as f64 * (d.width.max(1) - 1) as f64;
    let py = d.y as f64 + y.clamp(0.0, 1.0) as f64 * (d.height.max(1) - 1) as f64;
    let nx = ((px - vx as f64) * 65535.0 / (vw - 1) as f64).round() as i32;
    let ny = ((py - vy as f64) * 65535.0 / (vh - 1) as f64).round() as i32;
    (nx.clamp(0, 65535), ny.clamp(0, 65535))
}

impl WinInjector {
    pub fn handle(&mut self, events: &[InputEvent], display: &DisplayInfo) {
        self.batch.clear();
        for ev in events {
            match ev {
                InputEvent::MouseMove { x, y } => {
                    let (nx, ny) = to_absolute(display, *x, *y);
                    self.batch.push(mouse(
                        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        nx,
                        ny,
                        0,
                    ));
                }
                InputEvent::MouseButton { button, down } => {
                    if *down {
                        self.buttons.insert(*button);
                    } else {
                        self.buttons.remove(button);
                    }
                    let (f, data) = button_flags(*button, *down);
                    self.batch.push(mouse(f, 0, 0, data));
                }
                InputEvent::Wheel { dx, dy } => {
                    if *dy != 0 {
                        self.batch
                            .push(mouse(MOUSEEVENTF_WHEEL, 0, 0, *dy as i32 as u32));
                    }
                    if *dx != 0 {
                        self.batch
                            .push(mouse(MOUSEEVENTF_HWHEEL, 0, 0, *dx as i32 as u32));
                    }
                }
                InputEvent::Key { code, down } => {
                    let Some(k) = keymap::lookup(code) else {
                        continue;
                    };
                    if *down {
                        self.keys.insert(k);
                    } else if !self.keys.remove(&k) {
                        // 没按下过的键不发送抬起，避免干扰本地用户
                        continue;
                    }
                    self.batch.push(key_input(k, *down));
                }
                InputEvent::Text { text } => {
                    for unit in text.encode_utf16() {
                        self.batch.push(keybd(0, unit, KEYEVENTF_UNICODE));
                        self.batch
                            .push(keybd(0, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                    }
                }
                InputEvent::ReleaseAll => self.push_release_all(),
            }
        }
        self.flush();
    }

    fn push_release_all(&mut self) {
        for k in std::mem::take(&mut self.keys) {
            self.batch.push(key_input(k, false));
        }
        for b in std::mem::take(&mut self.buttons) {
            let (f, data) = button_flags(b, false);
            self.batch.push(mouse(f, 0, 0, data));
        }
    }

    pub fn release_all(&mut self) {
        self.batch.clear();
        self.push_release_all();
        self.flush();
    }

    fn flush(&mut self) {
        if self.batch.is_empty() {
            return;
        }
        crate::host::dpi_aware(|| unsafe {
            SendInput(&self.batch, std::mem::size_of::<INPUT>() as i32);
        });
        self.batch.clear();
    }
}
