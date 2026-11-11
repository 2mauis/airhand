//! tailcat-p2p 规格「先红验收测试清单」中被控端配置重载（放宽延后生效、点击时复查、提示超时）的用例
//! （`docs/specs/tailcat-p2p.md` I21 / 防注入）。每个用例注明清单编号。驱动在 [`driver`]，接上实现之前是空操作。

#[macro_use]
#[path = "../../../crates/airhand-link/tests/support/mod.rs"]
mod support;
#[path = "host_config_reload/driver.rs"]
mod driver;

use driver::{Approve, Host, Outcome};

const ASK_DIRECT: &str = "[host]\napprove = \"ask\"\n\n[direct]\nenabled = true\n";
const NEVER_DIRECT: &str = "[host]\napprove = \"never\"\n\n[direct]\nenabled = true\n";
const NEVER_NO_DIRECT: &str = "[host]\napprove = \"never\"\n";

/// 启动后当前配置确实是 `ask`（各用例的正向对照）
fn started(config: &str) -> Host {
    let h = Host::start(config);
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Ask),
        "启动配置没有生效（夹具前置）"
    );
    h
}

/// 没有带输入权限的在途会话时，改为 never 后弹本机确认，点击后生效（各用例的正向对照）
fn loosen_without_input_session_applies(config: &str) {
    let mut h = started(config);
    h.edit(NEVER_DIRECT);
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Ask),
        "放宽在本机确认之前就生效了"
    );
    p2p_assert!(
        h.prompt_shown(),
        "没有在途输入会话时改为 never 没有弹本机确认"
    );
    h.click_confirm();
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Never),
        "本机确认之后放宽没有生效（正向对照）"
    );
}

/// 清单 #58（I21、防注入）：存在已授权且带输入权限的会话时，把 `approve` 改为 never（或在 never 下打开
/// `[direct] enabled`）不立即生效，待这些会话全部结束且本机确认后才生效。
#[test]
fn config_change_to_never_during_input_session_not_applied() {
    loosen_without_input_session_applies(ASK_DIRECT);

    let mut h = started(ASK_DIRECT);
    let s = h.session(true);
    h.edit(NEVER_DIRECT);
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Ask),
        "带输入权限的会话在途时改为 never 立即生效了"
    );
    p2p_assert!(!h.prompt_shown(), "带输入权限的会话在途时就弹了确认提示");
    h.end_session(s);
    p2p_assert!(h.prompt_shown(), "输入会话结束后没有弹本机确认");
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Ask),
        "会话结束后未经本机确认就生效了"
    );
    h.click_confirm();
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Never),
        "会话结束且本机确认后没有生效"
    );

    // never 下打开 [direct]
    let mut h = Host::start(NEVER_NO_DIRECT);
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Never),
        "启动配置没有生效（夹具前置）"
    );
    let s = h.session(true);
    h.edit(NEVER_DIRECT);
    p2p_assert!(
        !h.effective().direct_enabled,
        "never 下有输入会话时打开 [direct] 立即生效了"
    );
    h.end_session(s);
    h.click_confirm();
    p2p_assert!(
        h.effective().direct_enabled,
        "会话结束且本机确认后 [direct] 没有生效"
    );
}

/// 清单 #112（防注入、I21）：放宽配置的本机确认在点击时重新检查“没有带输入权限的在途会话”，有则不生效。
#[test]
fn enable_never_confirmation_rechecks_no_input_session_at_click() {
    loosen_without_input_session_applies(ASK_DIRECT);

    let mut h = started(ASK_DIRECT);
    let s = h.session(false);
    h.edit(NEVER_DIRECT);
    p2p_assert!(
        h.prompt_shown(),
        "只有无输入权限的会话时改为 never 没有弹本机确认"
    );
    h.grant_input(s);
    h.click_confirm();
    p2p_assert_eq!(
        h.effective().approve,
        Some(Approve::Ask),
        "点击时已有带输入权限的会话，放宽仍然生效了"
    );
}

/// 清单 #113（防注入、I21）：放宽配置的确认提示显示期间，新会话按拒绝处理（`Bye(Busy)`）；提示结束后恢复正常。
#[test]
fn new_session_rejected_while_loosen_prompt_shown() {
    let mut h = started(ASK_DIRECT);
    p2p_assert_eq!(
        h.new_session(),
        Outcome::Handled,
        "没有提示时新会话应按当前配置处理（正向对照）"
    );
    h.edit(NEVER_DIRECT);
    p2p_assert!(h.prompt_shown(), "改为 never 没有弹本机确认");
    p2p_assert_eq!(
        h.new_session(),
        Outcome::Busy,
        "提示显示期间的新会话应回 Bye(Busy)"
    );
    h.click_confirm();
    p2p_assert_eq!(
        h.new_session(),
        Outcome::Handled,
        "提示结束后新会话应恢复正常处理"
    );
}

/// 清单 #138（防注入、I21）：放宽配置的本机确认提示在 `approval_timeout_secs`（默认 60 s）内无点击即视为不放宽并消失，
/// Busy 拒绝窗口随之结束。
#[test]
fn loosen_prompt_times_out_at_approval_timeout() {
    for (config, timeout) in [
        (ASK_DIRECT.to_string(), 60),
        (
            "[host]\napprove = \"ask\"\napproval_timeout_secs = 30\n\n[direct]\nenabled = true\n"
                .to_string(),
            30,
        ),
    ] {
        let mut h = started(&config);
        h.edit(NEVER_DIRECT);
        p2p_assert!(
            h.prompt_shown(),
            "超时 {timeout} s：改为 never 没有弹本机确认"
        );
        h.advance(timeout - 1);
        p2p_assert!(
            h.prompt_shown(),
            "超时 {timeout} s：提示在 {} s 时就消失了",
            timeout - 1
        );
        p2p_assert_eq!(
            h.new_session(),
            Outcome::Busy,
            "超时 {timeout} s：提示期间的新会话应回 Bye(Busy)"
        );
        h.advance(1);
        p2p_assert!(!h.prompt_shown(), "超时 {timeout} s：到时后提示没有消失");
        p2p_assert_eq!(
            h.effective().approve,
            Some(Approve::Ask),
            "超时 {timeout} s：超时应视为不放宽"
        );
        p2p_assert_eq!(
            h.new_session(),
            Outcome::Handled,
            "超时 {timeout} s：提示消失后 Busy 窗口应结束"
        );
    }
}
