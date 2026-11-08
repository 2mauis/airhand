//! 控制端：在 [`airhand_link::controller::ControllerService`] 外面包一层给 Kotlin 用的接口。
//!
//! * 服务事件（与服务器的连接状态、等待的被控端上线了）：JSON 字符串，[`Ctl::poll_event`] 取；
//! * 会话数据：每个会话一个队列，[`Ctl::poll_session`] 取。画面 / 光标 / 声音是 `airhand_link` 定义的二进制包
//!   （第一个字节是类型 1 / 2 / 3，见 [`airhand_link::controller::HEADER_LEN`]），会话事件是 JSON（第一个字节是 `{`）；
//! * 命令：JSON（[`Cmd`]），[`Ctl::call`] 执行。画面的确认走单独的 [`Ctl::ack`]（每帧一次）。
//!
//! 和网页控制台一样用 [`Frontend::Browser`]：画面在 Kotlin 那边确认；默认送进解码器后确认，
//! 音画同步时进入有界等待队列后确认，避免主动等待被当成网络拥塞。剪贴板也由 Kotlin 自己处理。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_link::controller::{
    ControllerService, Frontend, HostOnline, SessionEvent, SessionUi, UiClosed,
};
use airhand_link::net::Report;
use airhand_link::{LinkConfig, Problem, ServerState};
use airhand_proto::session::{InputEvent, QualitySettings};
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::queue::Queue;
use crate::rt::rt;
use crate::{hex, unhex};

/// 连接服务器用的配置（Kotlin 传来的 JSON）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkSettings {
    pub server: String,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub os: String,
}

impl LinkSettings {
    pub fn link(&self) -> Result<LinkConfig> {
        LinkConfig::new(
            self.server.trim(),
            &self.fingerprint,
            self.name.clone(),
            self.os.clone(),
        )
    }
}

/// Kotlin 发来的命令
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Cmd {
    /// 连接被控端（阻塞到有结果）。`resume`：之前连上这台时得到的重连凭证（32 位十六进制）
    Connect {
        code: String,
        resume: Option<String>,
        settings: QualitySettings,
    },
    Input {
        sid: u32,
        events: Vec<InputEvent>,
    },
    Settings {
        sid: u32,
        settings: QualitySettings,
    },
    Keyframe {
        sid: u32,
    },
    Display {
        sid: u32,
        index: u8,
    },
    Audio {
        sid: u32,
        enabled: bool,
    },
    Clipboard {
        sid: u32,
        text: String,
    },
    Close {
        sid: u32,
    },
}

/// 会话队列：画面一直有在途帧控制，正常情况下远远到不了上限
const SESSION_QUEUE: usize = 1024;

pub struct Ctl {
    svc: ControllerService,
    events: Arc<Queue<String>>,
    sessions: Mutex<HashMap<u32, Arc<Queue<Vec<u8>>>>>,
}

/// 会话的数据交给 Kotlin：放进队列
struct QueueUi(Arc<Queue<Vec<u8>>>);

impl SessionUi for QueueUi {
    fn packet(&self, packet: Vec<u8>) -> Result<(), UiClosed> {
        if self.0.push(packet) {
            Ok(())
        } else {
            Err(UiClosed)
        }
    }

    fn event(&self, event: SessionEvent) -> Result<(), UiClosed> {
        let json = serde_json::to_vec(&event).map_err(|_| UiClosed)?;
        if self.0.push(json) {
            Ok(())
        } else {
            Err(UiClosed)
        }
    }
}

impl Ctl {
    pub fn start(cfg: &LinkSettings) -> Result<Arc<Ctl>> {
        let link = Arc::new(cfg.link()?);
        let events = Arc::new(Queue::new(256));
        let ev = events.clone();
        let report: Report = Arc::new(move |s: ServerState| {
            ev.push(json!({ "type": "server", "server": s }).to_string());
        });
        let ev = events.clone();
        let host_online: HostOnline = Arc::new(move |code: u32| {
            ev.push(json!({ "type": "hostOnline", "code": code }).to_string());
        });
        let svc = ControllerService::start(link, rt().handle().clone(), report, host_online);
        Ok(Arc::new(Ctl {
            svc,
            events,
            sessions: Mutex::default(),
        }))
    }

    pub fn poll_event(&self, timeout: Duration) -> Option<String> {
        self.events.pop(timeout)
    }

    pub fn poll_session(&self, sid: u32, timeout: Duration) -> Option<Vec<u8>> {
        let q = self.sessions.lock().unwrap().get(&sid).cloned()?;
        q.pop(timeout)
    }

    pub fn ack(&self, sid: u32, seq: u32) {
        let _ = self.svc.ack(sid, seq);
    }

    /// 执行命令，返回 JSON：`{"ok": ...}` 或 `{"err": Problem}`
    pub fn call(&self, json: &str) -> String {
        let cmd: Cmd = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => return err(Problem::failed(format!("bad command: {e}"))),
        };
        let r = match cmd {
            Cmd::Connect {
                code,
                resume,
                settings,
            } => return self.connect(&code, resume.as_deref(), settings),
            Cmd::Input { sid, events } => self.svc.input(sid, events),
            Cmd::Settings { sid, settings } => self.svc.settings(sid, settings),
            Cmd::Keyframe { sid } => self.svc.keyframe(sid),
            Cmd::Display { sid, index } => self.svc.select_display(sid, index),
            Cmd::Audio { sid, enabled } => self.svc.set_audio(sid, enabled),
            Cmd::Clipboard { sid, text } => self.svc.send_clipboard(sid, text),
            Cmd::Close { sid } => {
                self.svc.disconnect(sid);
                if let Some(q) = self.sessions.lock().unwrap().remove(&sid) {
                    q.close();
                }
                Ok(())
            }
        };
        match r {
            Ok(()) => json!({ "ok": null }).to_string(),
            Err(p) => err(p),
        }
    }

    fn connect(&self, code: &str, resume: Option<&str>, settings: QualitySettings) -> String {
        let Some(code) = airhand_proto::parse_code(code) else {
            return err(Problem::InvalidCode);
        };
        let resume = resume.and_then(unhex::<16>);
        let queue = Arc::new(Queue::new(SESSION_QUEUE));
        let ui = Arc::new(QueueUi(queue.clone()));
        let result = rt().block_on(
            self.svc
                .connect(code, resume, settings, ui, Frontend::Browser),
        );
        match result {
            Ok(info) => {
                self.sessions.lock().unwrap().insert(info.sid, queue);
                json!({
                    "ok": {
                        "sid": info.sid,
                        "code": info.code,
                        "hostName": info.host_name,
                        "hostOs": info.host_os,
                        "resume": hex(&info.resume),
                    }
                })
                .to_string()
            }
            Err(p) => err(p),
        }
    }
}

impl Drop for Ctl {
    fn drop(&mut self) {
        self.events.close();
        for (_, q) in self.sessions.lock().unwrap().drain() {
            q.close();
        }
    }
}

fn err(p: Problem) -> String {
    json!({ "err": p }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_parse() {
        let c: Cmd = serde_json::from_str(
            r#"{"op":"input","sid":3,"events":[{"MouseMove":{"x":0.5,"y":0.25}},{"MouseButton":{"button":"Left","down":true}},{"Wheel":{"dx":0,"dy":-120}},{"Key":{"code":"KeyA","down":false}},{"Text":{"text":"你好"}},"ReleaseAll"]}"#,
        )
        .unwrap();
        let Cmd::Input { sid, events } = c else {
            panic!()
        };
        assert_eq!(sid, 3);
        assert_eq!(events.len(), 6);
        assert_eq!(events[0], InputEvent::MouseMove { x: 0.5, y: 0.25 });
        assert_eq!(events[5], InputEvent::ReleaseAll);

        let c: Cmd = serde_json::from_str(
            r#"{"op":"connect","code":"123 456","resume":null,"settings":{"fps":30,"bitrate_kbps":4000,"max_short":1080,"adaptive":true,"codec":"H265"}}"#,
        )
        .unwrap();
        assert!(matches!(c, Cmd::Connect { .. }));
    }

    #[test]
    fn bad_config_is_an_error() {
        let cfg = LinkSettings {
            server: "127.0.0.1:4433".into(),
            fingerprint: "zz".into(),
            name: String::new(),
            os: String::new(),
        };
        assert!(cfg.link().is_err());
    }

    #[test]
    fn invalid_code() {
        let ctl = Ctl::start(&LinkSettings {
            server: "127.0.0.1:1".into(),
            fingerprint: String::new(),
            name: "test".into(),
            os: "test".into(),
        })
        .unwrap();
        let r: serde_json::Value = serde_json::from_str(&ctl.call(
            r#"{"op":"connect","code":"12","resume":null,"settings":{"fps":30,"bitrate_kbps":4000,"max_short":1080,"adaptive":true,"codec":"H264"}}"#,
        ))
        .unwrap();
        assert_eq!(r, json!({ "err": { "kind": "invalidCode" } }));
        // 第一个事件是"正在连接服务器"
        let e: serde_json::Value =
            serde_json::from_str(&ctl.poll_event(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(
            e,
            json!({ "type": "server", "server": { "state": "connecting" } })
        );
    }
}
