//! tailcat-p2p 规格「先红验收测试清单」中被控端确认闸门的用例（`docs/specs/tailcat-p2p.md`）。
//! 每个用例注明清单编号。场景由 `direct/sim.rs` 驱动；直连实现接上之前是空操作驱动，用例因断言不满足而失败。

#[macro_use]
#[path = "support/mod.rs"]
mod support;
#[path = "direct/sim.rs"]
mod sim;

use std::time::Duration;

use airhand_link::Problem;
use sim::{
    Approve, CodeChoice, ControllerConfig, Event as E, HostConfig, Msg, Path, Scenario, Side, Trace,
};
use support::Expect;

const C0: u8 = 0;
const C1: u8 = 1;

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn connect(c: u8) -> E {
    E::Connect {
        controller: c,
        code: CodeChoice::Current,
    }
}

fn approve(c: u8) -> E {
    E::Approve {
        controller: c,
        direct: false,
    }
}

fn ctl(device: u8) -> ControllerConfig {
    ControllerConfig {
        device: [device; 16],
        ..ControllerConfig::default()
    }
}

fn hello_at(t: &Trace, c: u8) -> Option<Duration> {
    t.msgs(Side::Host, c)
        .find(|s| matches!(s.msg, Msg::Hello { .. }))
        .map(|s| s.at)
}

/// 清单 #67（I15）：ask 模式：确认之前只发 `AwaitingApproval`，不发 `HostMsg::Hello`。
#[test]
fn ask_mode_sends_awaiting_approval_and_no_hello() {
    let t = Scenario::new(HostConfig::default())
        .controller(ctl(1))
        .at(0, connect(C0))
        .at(5000, approve(C0))
        .run();
    let waiting = t
        .msgs(Side::Host, C0)
        .find(|s| s.msg == Msg::AwaitingApproval)
        .map(|s| s.at)
        .p2p_expect("确认之前没有发 AwaitingApproval");
    let hello = hello_at(&t, C0).p2p_expect("确认之后没有发 Hello（正向对照）");
    p2p_assert!(waiting < ms(5000), "AwaitingApproval 在 {waiting:?}");
    p2p_assert!(hello >= ms(5000), "确认之前（{hello:?}）就发了 Hello");
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .filter(|s| s.at < ms(5000))
            .all(|s| s.msg == Msg::AwaitingApproval),
        "确认之前发了 AwaitingApproval 以外的消息"
    );
}

/// 清单 #68（I15）：确认之前没有画面包与声音包。
#[test]
fn no_frames_or_audio_before_approval() {
    let t = Scenario::new(HostConfig::default())
        .controller(ctl(1))
        .screen(sim::Screen::Moving)
        .at(0, connect(C0))
        .at(5000, approve(C0))
        .run();
    p2p_assert!(
        t.frames.iter().any(|f| f.1 == C0 && f.0 >= ms(5000)),
        "确认之后没有画面（正向对照）"
    );
    p2p_assert!(!t.frames.iter().any(|f| f.0 < ms(5000)), "确认之前有画面包");
    p2p_assert!(!t.audio.iter().any(|a| a.0 < ms(5000)), "确认之前有声音包");
}

/// 清单 #91（I17）：超出 `max_pending_approvals` 的请求立即回 `Bye(Busy)`。
#[test]
fn pending_approvals_over_cap_get_busy() {
    let host = HostConfig {
        max_pending_approvals: 2,
        ..HostConfig::default()
    };
    let mut s = Scenario::new(host);
    for c in 0..4u8 {
        s = s.controller(ctl(c + 1)).at(100 * u64::from(c), connect(c));
    }
    let t = s.run_for(10).run();
    p2p_assert_eq!(t.approval_prompts.len(), 2, "应只弹出 2 个确认窗");
    for c in 2..4u8 {
        let busy = t
            .msgs(Side::Host, c)
            .find(|s| s.msg == Msg::Busy)
            .map(|s| s.at)
            .p2p_expect("超出上限的请求没有回 Bye(Busy)");
        p2p_assert!(
            busy <= ms(100 * u64::from(c) + 100),
            "Busy 不是立即回的（{busy:?}）"
        );
    }
}

/// 清单 #92（I17）：确认窗里的自报设备名限长并清理控制 / 双向字符，标注“自报”。
#[test]
fn approval_prompt_text_is_sanitized_and_length_limited() {
    let name = format!("ev\u{202e}il\u{0007}\n{}", "x".repeat(200));
    let t = Scenario::new(HostConfig::default())
        .controller(ControllerConfig { name, ..ctl(1) })
        .at(0, connect(C0))
        .run();
    let (_, text) = t.prompt_texts.first().p2p_expect("没有确认窗文字");
    p2p_assert!(text.contains("自报"), "确认窗没有标注“自报”：{text:?}");
    p2p_assert!(
        !text.chars().any(|c| c.is_control()
            || ('\u{202a}'..='\u{202e}').contains(&c)
            || ('\u{2066}'..='\u{2069}').contains(&c)),
        "确认窗里留有控制 / 双向字符：{text:?}"
    );
    p2p_assert!(
        text.chars().filter(|&c| c == 'x').count() <= 64,
        "设备名没有限长"
    );
}

/// 清单 #114（I21）：never 改回 ask 后在途会话继续到结束，新会话须确认。
#[test]
fn approve_never_to_ask_keeps_inflight_session_until_end() {
    let host = HostConfig {
        approve: Approve::Never,
        ..HostConfig::default()
    };
    let t = Scenario::new(host)
        .controller(ctl(1))
        .controller(ctl(2))
        .at(0, connect(C0))
        .at(5000, E::SetApprove(Approve::Ask))
        .at(8000, connect(C1))
        .run_for(20)
        .run();
    p2p_assert!(hello_at(&t, C0).is_some(), "never 下的会话没有开始");
    p2p_assert!(!t.session_ended(C0), "改回 ask 后在途会话被结束");
    p2p_assert!(
        t.approval_prompts.iter().any(|p| p.1 == C1),
        "改回 ask 后的新会话没有弹确认窗"
    );
    p2p_assert!(hello_at(&t, C1).is_none(), "新会话未经确认就开始了");
}

/// 清单 #118（握手第 4 步）：`[host] grant_memory_minutes` 默认 0（关闭），设为正数时才开启。
#[test]
fn grant_memory_minutes_defaults_off_and_is_configurable() {
    let reconnect = |host: HostConfig| {
        Scenario::new(host)
            .controller(ctl(1))
            .at(0, connect(C0))
            .at(1000, approve(C0))
            .at(5000, E::ControllerBye { controller: C0 })
            .at(8000, connect(C0))
            .run_for(15)
            .run()
    };
    let t = reconnect(HostConfig::default());
    p2p_assert_eq!(t.approval_prompts.len(), 2, "默认（0）时重连应再次确认");
    let t = reconnect(HostConfig {
        grant_memory_minutes: 10,
        ..HostConfig::default()
    });
    p2p_assert_eq!(
        t.approval_prompts.len(),
        1,
        "grant_memory_minutes = 10 时重连不应再确认"
    );
    p2p_assert!(hello_at(&t, C0).is_some());
}

/// 清单 #119（握手第 4 步、I7）：设备标识为每次安装随机生成的高熵值（≥ 128 bit），不在界面、日志、`Debug` 中显示。
#[test]
fn device_id_is_high_entropy_not_displayed_or_logged() {
    let install = || {
        Scenario::new(HostConfig::default())
            .controller(ControllerConfig::default())
            .at(0, connect(C0))
            .run()
    };
    let (a, b) = (install(), install());
    let id_a = a
        .device_ids
        .first()
        .map(|d| d.1.clone())
        .p2p_expect("被控端没有看到设备标识");
    let id_b = b
        .device_ids
        .first()
        .map(|d| d.1.clone())
        .p2p_expect("被控端没有看到设备标识");
    p2p_assert!(id_a.len() >= 16, "设备标识只有 {} 字节", id_a.len());
    p2p_assert!(id_a != id_b, "两次安装的设备标识相同");
    let hex: String = id_a.iter().map(|b| format!("{b:02x}")).collect();
    p2p_assert!(
        !a.logs
            .iter()
            .chain(a.ui_texts.iter())
            .any(|l| l.contains(&hex)),
        "设备标识出现在日志或界面里"
    );
}

/// 清单 #136（握手第 4 步、I15）：默认配置下同一设备在会话结束后重连仍需确认。
#[test]
fn same_device_reconnect_requires_reapproval_by_default() {
    let t = Scenario::new(HostConfig::default())
        .controller(ctl(7))
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(5000, E::ControllerBye { controller: C0 })
        .at(8000, connect(C0))
        .run_for(15)
        .run();
    p2p_assert!(hello_at(&t, C0).is_some(), "第一次会话没有开始");
    p2p_assert!(
        t.approval_prompts.iter().any(|p| p.0 >= ms(8000)),
        "同一设备重连没有再次确认"
    );
}

/// 清单 #137（I15、I17、防注入）：控制端收到 `Bye(Busy)`（`Problem::HostBusy`）不是终止性的；
/// 被控端发出 Busy 不计入静默拒绝计数、不弹窗。
#[test]
fn bye_busy_is_not_final_and_not_counted_as_decline() {
    p2p_assert!(!Problem::HostBusy.is_final(), "HostBusy 不应是终止性的");
    let host = HostConfig {
        max_pending_approvals: 1,
        ..HostConfig::default()
    };
    let t = Scenario::new(host)
        .controller(ctl(1))
        .controller(ctl(2))
        .at(0, connect(C0))
        .at(100, connect(C1))
        .run_for(10)
        .run();
    p2p_assert!(
        t.msgs(Side::Host, C1).any(|s| s.msg == Msg::Busy),
        "第二个请求没有收到 Busy（夹具前置）"
    );
    p2p_assert_eq!(t.approval_prompts.len(), 1, "Busy 的请求弹了确认窗");
    p2p_assert_eq!(t.silent_declines, 0, "Busy 被计入静默拒绝");
    p2p_assert!(t.msgs(Side::Host, C1).all(|s| s.path == Path::Relay));
}
