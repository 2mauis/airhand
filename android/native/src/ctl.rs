//! 控制端：在 [`airhand_link::controller::ControllerService`] 外面包一层给 Kotlin 用的接口。
//!
//! * 服务事件（与服务器的连接状态、等待的被控端上线了）：JSON 字符串，[`Ctl::poll_event`] 取：
//!   `{"type":"server","state":ServerState}`、`{"type":"hostOnline","code":123456}`（和网页控制台一样）；
//! * 会话数据：每个会话一个队列，[`Ctl::poll_session`] 取。画面 / 光标 / 声音是媒体包
//!   （`airhand_proto::media`，第一个字节是类型 1 / 2 / 3），会话事件是 JSON（第一个字节是 `{`）；
//! * 命令：JSON（[`Cmd`]：`connect` 加上和桌面版、网页版一样的 [`Command`]），[`Ctl::call`] 执行。
//!   画面的确认走单独的 [`Ctl::ack`]（每帧一次，不经过 JSON）。
//!
//! 和网页控制台一样用 [`Frontend::Browser`]：画面在 Kotlin 那边确认；默认送进解码器后确认，
//! 音画同步时进入有界等待队列后确认，避免主动等待被当成网络拥塞。剪贴板也由 Kotlin 自己处理。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use airhand_link::controller::{
    Command, ControllerService, Frontend, HostOnline, SessionEvent, SessionUi, UiClosed,
};
use airhand_link::net::Report;
use airhand_link::{LinkConfig, Problem, ServerState};
use airhand_proto::session::QualitySettings;
use airhand_proto::signal::ResumeToken;
use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use crate::queue::Queue;
use crate::rt::rt;

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
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Cmd {
    /// 连接被控端（阻塞到有结果），回复 `SessionInfo`。`resume`：之前连上这台时得到的重连凭证
    Connect {
        code: String,
        #[serde(default)]
        resume: Option<ResumeToken>,
        settings: QualitySettings,
    },
    /// 对会话的操作
    #[serde(untagged)]
    Session(Command),
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
            ev.push(json!({ "type": "server", "state": s }).to_string());
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
            } => return self.connect(&code, resume, settings),
            Cmd::Session(c) => {
                if let Command::Disconnect { sid } = c
                    && let Some(q) = self.sessions.lock().unwrap().remove(&sid)
                {
                    q.close();
                }
                self.svc.command(c)
            }
        };
        match r {
            Ok(()) => json!({ "ok": null }).to_string(),
            Err(p) => err(p),
        }
    }

    fn connect(
        &self,
        code: &str,
        resume: Option<ResumeToken>,
        settings: QualitySettings,
    ) -> String {
        let queue = Arc::new(Queue::new(SESSION_QUEUE));
        let ui = Arc::new(QueueUi(queue.clone()));
        let result = rt().block_on(
            self.svc
                .connect(code, resume, settings, ui, Frontend::Browser),
        );
        match result {
            Ok(info) => {
                self.sessions.lock().unwrap().insert(info.sid, queue);
                json!({ "ok": info }).to_string()
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
        // 和桌面版、网页版共用的命令样例（crates/airhand-link/tests/protocol.json）
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../crates/airhand-link/tests/protocol.json"
        ))
        .unwrap();
        for c in fixture["commands"].as_array().unwrap() {
            let parsed: Cmd =
                serde_json::from_value(c.clone()).unwrap_or_else(|e| panic!("{c}: {e}"));
            assert!(matches!(parsed, Cmd::Session(_)), "{c}");
        }

        let c: Cmd = serde_json::from_str(
            r#"{"cmd":"connect","code":"123 456","resume":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16],"settings":{"fps":30,"bitrate_kbps":4000,"max_short":1080,"adaptive":true,"codec":"H265"}}"#,
        )
        .unwrap();
        assert!(matches!(c, Cmd::Connect { resume: Some(r), .. } if r[15] == 16));
        let c: Cmd = serde_json::from_str(
            r#"{"cmd":"connect","code":"123456","settings":{"fps":30,"bitrate_kbps":4000,"max_short":1080,"adaptive":true,"codec":"H264"}}"#,
        )
        .unwrap();
        assert!(matches!(c, Cmd::Connect { resume: None, .. }));
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
            r#"{"cmd":"connect","code":"12","resume":null,"settings":{"fps":30,"bitrate_kbps":4000,"max_short":1080,"adaptive":true,"codec":"H264"}}"#,
        ))
        .unwrap();
        assert_eq!(r, json!({ "err": { "kind": "invalidCode" } }));
        // 没有这个会话
        let r: serde_json::Value =
            serde_json::from_str(&ctl.call(r#"{"cmd":"keyframe","sid":9}"#)).unwrap();
        assert_eq!(r, json!({ "err": { "kind": "noSession" } }));
        // 第一个事件是"正在连接服务器"
        let e: serde_json::Value =
            serde_json::from_str(&ctl.poll_event(Duration::from_secs(5)).unwrap()).unwrap();
        assert_eq!(
            e,
            json!({ "type": "server", "state": { "state": "connecting" } })
        );
    }
}
