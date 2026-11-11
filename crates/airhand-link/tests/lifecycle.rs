//! tailcat-p2p 规格「先红验收测试清单」中会话生命周期与权限收回的用例（`docs/specs/tailcat-p2p.md`）。
//! 每个用例注明清单编号。场景由 `direct/sim.rs` 驱动；直连实现接上之前是空操作驱动，用例因断言不满足而失败。

#[macro_use]
#[path = "support/mod.rs"]
mod support;
#[path = "direct/sim.rs"]
mod sim;

use std::time::Duration;

use airhand_proto::session::ByeReason;
use sim::{
    CodeChoice, ControllerConfig, Event as E, HostConfig, Msg, Path, Scenario, Screen, Side, Trace,
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

fn direct_controller(device: u8) -> ControllerConfig {
    ControllerConfig {
        direct_enabled: true,
        device: [device; 16],
        ..ControllerConfig::default()
    }
}

/// 两端启用直连、1 s 允许、2 s 两侧闸门打开的会话，`n` 个控制端
fn upgraded(n: u8) -> Scenario {
    let mut s = Scenario::new(HostConfig {
        direct_enabled: true,
        ..HostConfig::default()
    })
    .screen(Screen::Moving);
    for c in 0..n {
        s = s.controller(direct_controller(c + 1)).at(0, connect(c)).at(
            1000,
            E::Approve {
                controller: c,
                direct: true,
            },
        );
        for side in [Side::Controller, Side::Host] {
            s = s.at(
                2000,
                E::GateOpen {
                    side,
                    controller: c,
                },
            );
        }
    }
    s
}

fn require_direct(t: &Trace, c: u8) {
    p2p_assert!(
        t.upgrades.iter().any(|u| u.controller == c),
        "控制端 {c} 没有升级到直连（正向对照）"
    );
    p2p_assert!(
        !t.spawns(Side::Host, c).is_empty(),
        "控制端 {c} 的 helper 没有运行过"
    );
}

fn lanes_closed_after(t: &Trace, c: u8, at: Duration) -> bool {
    !t.frames.iter().any(|f| f.1 == c && f.0 > at + ms(500))
        && t.exits(Side::Host, c).iter().any(|&x| x >= at)
}

/// 清单 #80（I10）：`KickAll` 结束两条路径。
#[test]
fn kick_all_ends_direct_and_relay_lanes() {
    let t = upgraded(2).at(8000, E::KickAll).run();
    for c in [C0, C1] {
        require_direct(&t, c);
        p2p_assert!(t.session_ended(c), "KickAll 后控制端 {c} 的会话没有结束");
        p2p_assert!(
            lanes_closed_after(&t, c, ms(8000)),
            "控制端 {c} 仍有路径在运行"
        );
    }
}

/// 清单 #81（I10、I18）：`NewCode` 结束直连会话。
#[test]
fn new_code_ends_direct_session() {
    let t = upgraded(1).at(8000, E::NewCode).run();
    require_direct(&t, C0);
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .any(|s| s.msg == Msg::Bye(Some(ByeReason::CodeChanged))),
        "换码后没有发 Bye(CodeChanged)"
    );
    p2p_assert!(lanes_closed_after(&t, C0, ms(8000)), "换码后直连仍在运行");
}

/// 清单 #82（I10）：`Bye` 一律经中转发送。
#[test]
fn bye_always_sent_on_relay() {
    for ev in [E::KickAll, E::NewCode, E::Kick { controller: C0 }] {
        let t = upgraded(1).at(8000, ev.clone()).run();
        require_direct(&t, C0);
        let byes: Vec<_> = t
            .msgs(Side::Host, C0)
            .filter(|s| matches!(s.msg, Msg::Bye(_)))
            .collect();
        p2p_assert!(!byes.is_empty(), "{ev:?}：没有发 Bye");
        p2p_assert!(
            byes.iter().all(|s| s.path == Path::Relay),
            "{ev:?}：Bye 走了直连"
        );
    }
}

/// 清单 #88（I18）：停止共享结束中转与直连两条路径；正向对照：停止前两条路径都有数据，helper 确实运行过。
#[test]
fn stop_sharing_ends_relay_and_direct_paths() {
    let t = upgraded(1).at(8000, E::StopSharing).run();
    require_direct(&t, C0);
    p2p_assert!(
        t.frames
            .iter()
            .any(|f| f.2 == Path::Direct && f.0 < ms(8000))
            && t.frames
                .iter()
                .any(|f| f.2 == Path::Relay && f.0 < ms(8000)),
        "停止前两条路径没有都有数据"
    );
    p2p_assert!(
        lanes_closed_after(&t, C0, ms(8000)),
        "停止共享后仍有路径在运行"
    );
}

/// 清单 #89（I18）：停止共享之后迟到的直连连接 / 令牌被拒；正向对照：停止之前同一令牌可连。
#[test]
fn late_direct_connection_after_stop_is_refused() {
    let t = upgraded(1)
        .at(8000, E::StopSharing)
        .at(9000, E::LateDirectConnection { controller: C0 })
        .run();
    require_direct(&t, C0);
    p2p_assert_eq!(
        t.token_checks,
        vec![true, false],
        "停止共享后的迟到连接应被拒"
    );
}

/// 清单 #90（I18、S2-10）：停止共享（换码）后，换码前取得的重连凭证被拒；旧码为 NotFound；旧等待者在被控端以新码
/// 上线后没有收到上线通知，且该等待项被清理；单条连接的等待项上限 `MAX_WAITING` = 32、超出时忘掉最早的一项，
/// 不影响另一条连接。真实 broker（`server/src/broker.rs`）是二进制 crate，这里经场景驱动的 broker 钩子断言。
#[test]
fn stop_sharing_invalidates_prior_resume_token() {
    let t = upgraded(1)
        .controller(direct_controller(9))
        .at(
            5000,
            E::ResumeWait {
                controller: C0,
                stale: false,
            },
        )
        .at(8000, E::StopSharing)
        .at(
            9000,
            E::ResumeWait {
                controller: C0,
                stale: true,
            },
        )
        .at(
            9500,
            E::Connect {
                controller: C0,
                code: CodeChoice::Previous,
            },
        )
        .at(12_000, E::HostOnline)
        .at(
            13_000,
            E::FillWaits {
                controller: C0,
                count: 40,
            },
        )
        .at(
            13_000,
            E::FillWaits {
                controller: C1,
                count: 3,
            },
        )
        .run();
    p2p_assert!(
        t.waiters
            .iter()
            .any(|w| w.1 == C0 && w.0 >= ms(9000) && w.0 < ms(12_000) && w.2 >= 1),
        "持旧凭证的等待者没有进入等待列表（夹具前置）"
    );
    p2p_assert!(
        t.connects
            .iter()
            .any(|c| c.code == CodeChoice::Previous && !c.accepted),
        "旧码没有被拒（NotFound）"
    );
    p2p_assert!(
        !t.online_notices
            .iter()
            .any(|n| n.1 == C0 && n.0 >= ms(12_000)),
        "旧等待者收到了新码的上线通知"
    );
    let after = |c: u8| t.waiters.iter().rev().find(|w| w.1 == c).map(|w| w.2);
    p2p_assert_eq!(after(C0), Some(32), "单条连接的等待项上限应为 32");
    p2p_assert_eq!(after(C1), Some(3), "一条连接占满等待项影响了另一条连接");
}

/// 清单 #93（I19）：控制端 `Bye` 后关闭直连并结束 helper；正向对照：`Bye` 之前 helper 与直连确实在运行。
#[test]
fn controller_bye_closes_direct_and_kills_helper() {
    let t = upgraded(1)
        .at(8000, E::ControllerBye { controller: C0 })
        .run();
    require_direct(&t, C0);
    p2p_assert!(
        t.exits(Side::Host, C0).iter().any(|&x| x >= ms(8000)),
        "Bye 后被控端 helper 没有结束"
    );
    p2p_assert!(
        t.exits(Side::Controller, C0).iter().any(|&x| x >= ms(8000)),
        "Bye 后控制端 helper 没有结束"
    );
    p2p_assert!(
        !t.frames
            .iter()
            .any(|f| f.2 == Path::Direct && f.0 > ms(8500)),
        "Bye 后直连仍有画面"
    );
}

/// 清单 #94（I19）：控制端 `Bye` 经中转发送。
#[test]
fn controller_bye_sent_on_relay() {
    let t = upgraded(1)
        .at(8000, E::ControllerBye { controller: C0 })
        .run();
    require_direct(&t, C0);
    let bye = t
        .msgs(Side::Controller, C0)
        .find(|s| s.msg == Msg::Bye(None))
        .p2p_expect("控制端没有发 Bye");
    p2p_assert_eq!(bye.path, Path::Relay, "控制端的 Bye 走了直连");
}

/// 清单 #95（I20）：收回输入权限后丢弃后续输入。
#[test]
fn revoking_input_permission_drops_input() {
    let t = upgraded(1)
        .at(
            5000,
            E::Input {
                controller: C0,
                count: 10,
            },
        )
        .at(8000, E::RevokeInput { controller: C0 })
        .at(
            9000,
            E::Input {
                controller: C0,
                count: 10,
            },
        )
        .run();
    p2p_assert_eq!(
        t.inputs.iter().filter(|i| i.0 < ms(8000)).count(),
        10,
        "收回之前的输入没有注入（正向对照）"
    );
    p2p_assert!(
        !t.inputs.iter().any(|i| i.0 >= ms(8000)),
        "收回输入权限后仍注入了输入"
    );
}

/// 清单 #96（I20）：收回直连权限立即回退并销毁 helper，本会话不再重试；正向对照：收回之前直连与 helper 确实在运行。
#[test]
fn revoking_direct_permission_falls_back_and_destroys_helper() {
    let t = upgraded(1)
        .at(8000, E::RevokeDirect { controller: C0 })
        .run_for(120)
        .run();
    require_direct(&t, C0);
    let fb = t
        .msgs(Side::Host, C0)
        .find(|s| s.path == Path::Relay && matches!(s.msg, Msg::DirectFallback { .. }))
        .map(|s| s.at)
        .p2p_expect("收回直连权限后没有回退");
    p2p_assert!(fb <= ms(8100), "回退不是立即的（{fb:?}）");
    p2p_assert!(
        t.exits(Side::Host, C0)
            .iter()
            .any(|&x| x >= ms(8000) && x <= ms(8500)),
        "helper 没有被销毁"
    );
    p2p_assert!(
        !t.msgs(Side::Host, C0)
            .any(|s| s.at > ms(8000) && matches!(s.msg, Msg::DirectOffer { .. })),
        "收回直连权限后本会话仍在重试"
    );
}

/// 清单 #97（I20）：收回剪贴板后两个方向都停；正向对照：收回之前两个方向各通过 ≥ 1 条。
#[test]
fn revoking_clipboard_stops_both_directions() {
    let t = upgraded(1)
        .at(5000, E::Clipboard { controller: C0 })
        .at(8000, E::RevokeClipboard { controller: C0 })
        .at(9000, E::Clipboard { controller: C0 })
        .run();
    for dir in [Side::Host, Side::Controller] {
        p2p_assert!(
            t.clipboard.iter().any(|c| c.1 == dir && c.0 < ms(8000)),
            "收回之前 {dir:?} 方向没有剪贴板通过（正向对照）"
        );
    }
    p2p_assert!(
        !t.clipboard.iter().any(|c| c.0 >= ms(8000)),
        "收回剪贴板后仍有剪贴板通过"
    );
}
