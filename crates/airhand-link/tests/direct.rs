//! tailcat-p2p 规格「先红验收测试清单」中会话 / 直连状态机的用例（`docs/specs/tailcat-p2p.md`）。
//!
//! 每个用例注明清单编号（该表从上往下数，1–44）。场景由 [`sim`] 驱动；直连实现接上之前是空操作驱动，
//! 用例因断言不满足而失败（`ASSERT:`，见 `support`）。Phase 1 按方案 (a)（`DirectGateOpen`）编写，
//! 方案 (b) 的清单 #17 / #18 不在本轮。

#[macro_use]
#[path = "support/mod.rs"]
mod support;
#[path = "direct/sim.rs"]
mod sim;

use std::time::Duration;

use airhand_proto::session::ByeReason;
use sim::{
    Approve, CodeChoice, ControllerConfig, Event as E, HostConfig, Msg, Path, Scenario, Screen,
    Side, Trace, Upgrade,
};
use support::Expect;

const C0: u8 = 0;
const C1: u8 = 1;

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

fn direct_host() -> HostConfig {
    HostConfig {
        direct_enabled: true,
        ..HostConfig::default()
    }
}

fn never_direct_host() -> HostConfig {
    HostConfig {
        approve: Approve::Never,
        ..direct_host()
    }
}

fn direct_controller(device: u8) -> ControllerConfig {
    ControllerConfig {
        direct_enabled: true,
        device: [device; 16],
    }
}

fn connect(c: u8) -> E {
    E::Connect {
        controller: c,
        code: CodeChoice::Current,
    }
}

fn gates(s: Scenario, at: u64, c: u8) -> Scenario {
    s.at(
        at,
        E::GateOpen {
            side: Side::Controller,
            controller: c,
        },
    )
    .at(
        at,
        E::GateOpen {
            side: Side::Host,
            controller: c,
        },
    )
}

/// ask + 两端启用直连：0 s 连上，1 s 用户允许（含直连），2 s 两侧闸门打开
fn upgraded() -> Scenario {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        );
    gates(s, 2000, C0)
}

fn run(s: &Scenario) -> Trace {
    s.run()
}

fn upgrade(t: &Trace, c: u8) -> &Upgrade {
    p2p_some!(
        t.upgrades.iter().find(|u| u.controller == c),
        "控制端 {c} 没有完成直连升级"
    )
}

fn fallback_at(t: &Trace, from: Side, c: u8) -> Option<Duration> {
    t.msgs(from, c)
        .find(|s| s.path == Path::Relay && matches!(s.msg, Msg::DirectFallback { .. }))
        .map(|s| s.at)
}

fn any_fallback(t: &Trace, c: u8) -> bool {
    fallback_at(t, Side::Host, c).is_some() || fallback_at(t, Side::Controller, c).is_some()
}

fn sent_bye(t: &Trace, from: Side, c: u8) -> bool {
    t.msgs(from, c).any(|s| matches!(s.msg, Msg::Bye(_)))
}

/// 清单 #7（I3、I2、I24）：栅栏等待期间、静止桌面、无控制流 Ping 时被控端闸门保持打开；
/// 直连 `keep_alive_interval` ≤ 250 ms。
#[test]
fn host_gate_not_closed_during_fence_wait_on_static_screen() {
    // 中转单向 1.5 s：控制端要很久才读到 `DirectSwitch`，这期间直连控制流上没有 Ping
    let s = upgraded()
        .screen(Screen::Static)
        .at(1500, E::RelayLatency(ms(1500)));
    let t = run(&s);
    let keep_alive = t
        .direct_keep_alive
        .p2p_expect("直连传输配置未设置 keep_alive_interval");
    p2p_assert!(
        keep_alive <= ms(250),
        "keep_alive_interval = {keep_alive:?}"
    );
    let up = upgrade(&t, C0);
    let opened = t
        .gates
        .iter()
        .find(|g| g.side == Side::Host && g.epoch == up.epoch && g.open)
        .map(|g| g.at)
        .p2p_expect("被控端闸门从未打开");
    p2p_assert!(
        !t.gates
            .iter()
            .any(|g| g.side == Side::Host && g.epoch == up.epoch && !g.open && g.at > opened),
        "栅栏等待期间被控端闸门因静默关闭"
    );
    p2p_assert!(!any_fallback(&t, C0));
}

/// 清单 #13（I24）：注入被控端闸门晚于控制端打开的时间差，升级期间因闸门关闭而丢弃的直连 QUIC 包为 0，
/// 且没有因 Initial 丢失而走 PTO 的重传。
#[test]
fn first_quic_initial_not_lost_to_host_gate() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        )
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .at(
            2300,
            E::GateOpen {
                side: Side::Host,
                controller: C0,
            },
        );
    let t = run(&s);
    upgrade(&t, C0);
    p2p_assert!(
        t.datagrams.iter().any(|d| d.from == Side::Controller),
        "控制端没有发出任何直连 QUIC 包"
    );
    let dropped = t.datagrams.iter().filter(|d| d.dropped_by_gate).count();
    p2p_assert_eq!(dropped, 0, "升级期间有 {dropped} 个直连 QUIC 包被闸门丢弃");
    p2p_assert_eq!(t.pto_retransmits, 0);
}

/// 清单 #14（I24 方案 (a)、F4）：未收到 `DirectGateOpen` 时，控制端发出的直连 QUIC 包数为 0。
#[test]
fn controller_waits_for_direct_gate_open_before_first_quic() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        )
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .at(
            3000,
            E::GateOpen {
                side: Side::Host,
                controller: C0,
            },
        );
    let t = run(&s);
    let gate_open = t
        .first(|s| {
            s.from == Side::Host
                && s.path == Path::Relay
                && matches!(s.msg, Msg::DirectGateOpen { .. })
        })
        .and_then(|s| s.delivered)
        .p2p_expect("控制端没有收到 DirectGateOpen");
    let controller_quic: Vec<_> = t
        .datagrams
        .iter()
        .filter(|d| d.from == Side::Controller)
        .collect();
    p2p_assert!(!controller_quic.is_empty(), "控制端没有发出直连 QUIC 包");
    p2p_assert!(
        controller_quic.iter().all(|d| d.at >= gate_open),
        "控制端在收到 DirectGateOpen（{gate_open:?}）之前就发了直连 QUIC 包"
    );
}

/// 清单 #15（I24 方案 (a)、F4）：被控端闸门打开后经中转发 `DirectGateOpen { epoch }`，`epoch` 与当前升级一致。
#[test]
fn host_sends_direct_gate_open_when_gate_opens() {
    let t = run(&upgraded());
    let epoch = t.offer_epoch(C0).p2p_expect("没有 DirectOffer");
    let opened = t
        .gates
        .iter()
        .find(|g| g.side == Side::Host && g.controller == C0 && g.open)
        .map(|g| g.at)
        .p2p_expect("被控端闸门没有打开");
    let sent = t
        .first(|s| {
            s.from == Side::Host
                && s.path == Path::Relay
                && matches!(s.msg, Msg::DirectGateOpen { .. })
        })
        .p2p_expect("被控端没有发 DirectGateOpen");
    p2p_assert_eq!(sent.msg, Msg::DirectGateOpen { epoch });
    p2p_assert!(sent.at >= opened && sent.at - opened <= ms(100));
}

/// 清单 #16（I13 / I24 方案 (a)）：迟到或 `epoch` 不匹配的 `DirectGateOpen` 被忽略。
#[test]
fn late_or_mismatched_direct_gate_open_is_ignored() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        )
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .at(
            2200,
            E::InjectDirectGateOpen {
                controller: C0,
                epoch_delta: 1,
            },
        )
        .at(
            2400,
            E::InjectDirectGateOpen {
                controller: C0,
                epoch_delta: -1,
            },
        )
        .at(
            4000,
            E::GateOpen {
                side: Side::Host,
                controller: C0,
            },
        );
    let t = run(&s);
    upgrade(&t, C0);
    let first_quic = t
        .datagrams
        .iter()
        .filter(|d| d.from == Side::Controller)
        .map(|d| d.at)
        .min()
        .p2p_expect("控制端没有发出直连 QUIC 包");
    p2p_assert!(
        first_quic >= ms(4000),
        "控制端因不匹配的 DirectGateOpen 提前发包（{first_quic:?}）"
    );
}

/// 清单 #20（I25、I2）：假写路径超时后仍阻塞 ⇒ 回退中转、结束 helper 进程并拉起新进程后开新 `epoch`、
/// 会话继续，不发结束会话的 `Bye`；计入最多 3 次重试。
#[test]
fn helper_write_stall_triggers_fallback() {
    let s = upgraded().at(
        8000,
        E::HelperWriteStall {
            side: Side::Host,
            controller: C0,
        },
    );
    let t = run(&s);
    let old = upgrade(&t, C0).epoch;
    let fb = fallback_at(&t, Side::Host, C0).p2p_expect("写卡住后没有回退");
    p2p_assert!(
        fb >= ms(8000) && fb <= ms(8000) + ms(500) + ms(250),
        "{fb:?}"
    );
    p2p_assert!(
        t.exits(Side::Host, C0).iter().any(|&at| at >= ms(8000)),
        "卡住的 helper 进程没有被结束"
    );
    p2p_assert!(
        t.spawns(Side::Host, C0).iter().any(|&at| at > fb),
        "回退后没有拉起新的 helper 进程"
    );
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .any(|s| matches!(s.msg, Msg::DirectOffer { epoch } if epoch > old)),
        "没有用新 epoch 重新升级"
    );
    p2p_assert!(!sent_bye(&t, Side::Host, C0) && !t.session_ended(C0));
    p2p_assert_eq!(t.upgrade_retries, vec![(C0, 1)]);
}

/// 清单 #21（I26、F10）：注入较低的 C（30 Mbps），目标码率高于 C 时被限制在 C，且不发 `DirectFallback`。
#[test]
fn direct_session_caps_bitrate_instead_of_falling_back() {
    let mut s = upgraded().at(
        8000,
        E::TargetBitrate {
            controller: C0,
            kbps: 80_000,
        },
    );
    s.host.bitrate_cap_kbps = 30_000;
    let t = run(&s);
    let direct: Vec<_> = t
        .bitrate
        .iter()
        .filter(|b| b.path == Path::Direct && b.at >= ms(8000))
        .collect();
    p2p_assert!(!direct.is_empty(), "直连期间没有应用目标码率");
    p2p_assert!(direct.iter().all(|b| b.kbps <= 30_000));
    p2p_assert!(
        direct.iter().any(|b| b.kbps == 30_000),
        "目标码率没有被限制在 C"
    );
    p2p_assert!(!any_fallback(&t, C0), "码率达到上限不应回退中转");
}

/// 清单 #22（I26、F10）：同样在注入的较低 C 下，回退中转后解除该上限。
#[test]
fn fallback_to_relay_lifts_direct_bitrate_cap() {
    let mut s = upgraded()
        .at(
            8000,
            E::TargetBitrate {
                controller: C0,
                kbps: 80_000,
            },
        )
        .at(10_000, E::DirectPathFails { controller: C0 })
        .at(
            12_000,
            E::TargetBitrate {
                controller: C0,
                kbps: 80_000,
            },
        );
    s.host.bitrate_cap_kbps = 30_000;
    let t = run(&s);
    p2p_assert!(
        fallback_at(&t, Side::Host, C0).is_some()
            || fallback_at(&t, Side::Controller, C0).is_some()
    );
    let after = t
        .bitrate
        .iter()
        .find(|b| b.path == Path::Relay && b.at >= ms(12_000))
        .p2p_expect("回退后没有应用目标码率");
    p2p_assert_eq!(after.kbps, 80_000, "回退中转后仍受直连上限 C 限制");
}

/// 清单 #26（I1）：未启用时假 helper 工厂 0 次调用，无 `Direct*` 消息。
#[test]
fn disabled_by_default_never_spawns_helper() {
    let s = Scenario::new(HostConfig::default())
        .controller(ControllerConfig::default())
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: false,
            },
        )
        .run_for(20);
    let t = run(&s);
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .any(|s| matches!(s.msg, Msg::Hello { .. })),
        "会话没有开始"
    );
    p2p_assert!(t.helper_spawns.is_empty());
    p2p_assert!(t.network.is_empty());
    p2p_assert!(!t.sent.iter().any(|s| s.msg.is_direct_control()));
}

/// 清单 #29（I11）：直连上观测到的最大数据报 ≤ 1232；超限包被丢弃而非依赖库报错。
#[test]
fn direct_quic_never_sends_datagram_over_1232() {
    let s = upgraded()
        .screen(Screen::Moving)
        .at(
            8000,
            E::VideoFrame {
                controller: C0,
                bytes: 400_000,
            },
        )
        .at(
            8100,
            E::TargetBitrate {
                controller: C0,
                kbps: 100_000,
            },
        );
    let t = run(&s);
    p2p_assert!(!t.datagrams.is_empty(), "没有观测到直连数据报");
    let max = t.datagrams.iter().map(|d| d.size).max().unwrap_or(0);
    p2p_assert!(max <= 1232, "观测到 {max} 字节的直连数据报");
    p2p_assert!(!t.datagrams.iter().any(|d| d.dropped_oversize));
}

/// 清单 #30（I12 / I16）：取得直连同意前不发 `DirectOffer`、不启动被控端 helper。
#[test]
fn no_offer_before_approval() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(1000, E::ForceDirectRequest { controller: C0 })
        .at(
            5000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        );
    let t = run(&s);
    let offers: Vec<_> = t
        .msgs(Side::Host, C0)
        .filter(|s| matches!(s.msg, Msg::DirectOffer { .. }))
        .collect();
    p2p_assert!(!offers.is_empty(), "同意之后也没有 DirectOffer");
    p2p_assert!(
        offers.iter().all(|s| s.at >= ms(5000)),
        "同意之前发了 DirectOffer"
    );
    p2p_assert!(t.spawns(Side::Host, C0).iter().all(|&at| at >= ms(5000)));
}

/// 清单 #31（I16）：控制端在 `DirectOffer` 到达之前 helper 无网络活动（不拉 DERP 图、不连 DERP）。
#[test]
fn controller_helper_no_network_before_offer() {
    let t = run(&upgraded());
    let offer = t
        .first(|s| s.from == Side::Host && matches!(s.msg, Msg::DirectOffer { .. }))
        .and_then(|s| s.delivered)
        .p2p_expect("控制端没有收到 DirectOffer");
    p2p_assert!(
        !t.spawns(Side::Controller, C0).is_empty(),
        "控制端没有创建 helper（取 Client.PublicKey 用）"
    );
    let net: Vec<_> = t
        .network
        .iter()
        .filter(|n| n.side == Side::Controller)
        .collect();
    p2p_assert!(!net.is_empty(), "控制端 helper 始终没有网络活动");
    p2p_assert!(
        net.iter().all(|n| n.at >= offer),
        "DirectOffer 之前控制端 helper 已有网络活动：{:?}",
        net.iter().find(|n| n.at < offer)
    );
}

/// 清单 #32（I21）：`never` + `[direct] enabled` 且对端声明直连时不弹窗即可升级。
#[test]
fn approve_never_allows_direct_when_enabled() {
    let s = gates(
        Scenario::new(never_direct_host())
            .controller(direct_controller(1))
            .at(0, connect(C0)),
        1000,
        C0,
    );
    let t = run(&s);
    p2p_assert!(t.approval_prompts.is_empty());
    p2p_assert!(
        !t.msgs(Side::Host, C0)
            .any(|s| s.msg == Msg::AwaitingApproval)
    );
    p2p_assert!(t.msgs(Side::Host, C0).any(|s| s.msg
        == Msg::Hello {
            granted_direct: true
        }));
    upgrade(&t, C0);
}

/// 清单 #33（I18 / I21 已知风险）：被“断开此控制端”的持码人在换码前可以再连回来（记录已知行为，
/// 不把拒绝重连当成默认）；停止共享换码后旧码被拒。
#[test]
fn never_mode_kicked_device_can_return_until_code_changed() {
    let s = gates(
        Scenario::new(never_direct_host())
            .controller(direct_controller(1))
            .at(0, connect(C0)),
        1000,
        C0,
    )
    .at(5000, E::Kick { controller: C0 })
    .at(6000, connect(C0))
    .at(12_000, E::StopSharing)
    .at(
        13_000,
        E::Connect {
            controller: C0,
            code: CodeChoice::Previous,
        },
    );
    let t = run(&s);
    let attempts: Vec<_> = t.connects.iter().map(|c| (c.at, c.accepted)).collect();
    p2p_assert_eq!(
        attempts,
        vec![(ms(0), true), (ms(6000), true), (ms(13_000), false)]
    );
    let hellos = t
        .msgs(Side::Host, C0)
        .filter(|s| {
            s.msg
                == Msg::Hello {
                    granted_direct: true,
                }
        })
        .count();
    p2p_assert_eq!(hellos, 2, "再连回来时没有按 I21 再次授予直连");
}

/// 清单 #34（I21）：`never` 与 `[direct] enabled` 两项显式配置视为同意，可发 `DirectOffer` / 启动 helper；
/// 只开 `never` 时不交换地址、不启动 helper。
#[test]
fn approve_never_operator_config_counts_as_consent() {
    let consent = run(&Scenario::new(never_direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0)));
    p2p_assert!(consent.approval_prompts.is_empty());
    p2p_assert!(
        consent.offer_epoch(C0).is_some(),
        "never + 直连没有发 DirectOffer"
    );
    p2p_assert!(!consent.spawns(Side::Host, C0).is_empty());

    let never_only = HostConfig {
        approve: Approve::Never,
        ..HostConfig::default()
    };
    let t = run(&Scenario::new(never_only)
        .controller(direct_controller(1))
        .at(0, connect(C0)));
    p2p_assert!(t.msgs(Side::Host, C0).any(|s| s.msg
        == Msg::Hello {
            granted_direct: false
        }));
    p2p_assert!(t.offer_epoch(C0).is_none());
    p2p_assert!(t.helper_spawns.is_empty());
}

/// 清单 #35（I2）：无画面无声音数十秒，Ping/Pong 正常 ⇒ 不回退。
#[test]
fn static_screen_does_not_trigger_fallback() {
    let s = upgraded().screen(Screen::Static).run_for(60);
    let t = run(&s);
    let up = upgrade(&t, C0).completed;
    let pings = t
        .msgs(Side::Controller, C0)
        .filter(|s| s.path == Path::Direct && s.msg == Msg::Ping && s.at > up)
        .count();
    p2p_assert!(
        pings >= 100,
        "直连上只发了 {pings} 个 Ping（应为 250 ms 间隔）"
    );
    p2p_assert!(!any_fallback(&t, C0));
    p2p_assert_eq!(t.last_sender_path(Side::Host, C0), Some(Path::Direct));
}

/// 清单 #36（I2、存活检测）：直连上 K 秒收不到 `Pong` ⇒ `DirectFallback`，会话保持中转。
#[test]
fn missing_pong_triggers_fallback() {
    let s = upgraded().at(8000, E::StopPongs { controller: C0 });
    let t = run(&s);
    upgrade(&t, C0);
    let fb = fallback_at(&t, Side::Controller, C0).p2p_expect("收不到 Pong 却没有回退");
    p2p_assert!(
        fb >= ms(8000) + ms(1500) && fb <= ms(8000) + ms(2500),
        "{fb:?}"
    );
    p2p_assert!(!t.session_ended(C0));
    p2p_assert_eq!(t.last_sender_path(Side::Controller, C0), Some(Path::Relay));
}

/// 清单 #37（I2、I10）：helper 崩溃只回退，不结束会话。
#[test]
fn helper_crash_does_not_end_session() {
    let s = upgraded().at(
        8000,
        E::HelperCrash {
            side: Side::Host,
            controller: C0,
        },
    );
    let t = run(&s);
    upgrade(&t, C0);
    p2p_assert!(any_fallback(&t, C0), "helper 崩溃后没有回退");
    p2p_assert!(!sent_bye(&t, Side::Host, C0) && !t.session_ended(C0));
    p2p_assert_eq!(t.last_sender_path(Side::Host, C0), Some(Path::Relay));
}

/// 清单 #38（I3）：任一侧闸门未开时不切换发送端。
#[test]
fn no_switch_before_both_gates_open() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        )
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .run_for(25);
    let t = run(&s);
    p2p_assert!(t.offer_epoch(C0).is_some(), "升级没有开始");
    p2p_assert!(
        !t.sent
            .iter()
            .any(|s| matches!(s.msg, Msg::DirectSwitch { .. } | Msg::DirectCommit { .. }))
    );
    p2p_assert!(t.sender_paths.iter().all(|p| p.path == Path::Relay));
    p2p_assert!(t.upgrades.is_empty() && !t.session_ended(C0));
}

/// 清单 #39（I5）：直连令牌只能使用一次。
#[test]
fn direct_token_is_one_time() {
    let s = upgraded().at(8000, E::ReplayDirectToken { controller: C0 });
    let t = run(&s);
    upgrade(&t, C0);
    p2p_assert_eq!(t.token_checks, vec![true, false]);
}

/// 清单 #41（I18）：停止共享换码，旧码立即被拒。
#[test]
fn stop_sharing_rotates_code_old_code_rejected() {
    let s = upgraded()
        .at(8000, E::StopSharing)
        .at(
            9000,
            E::Connect {
                controller: C0,
                code: CodeChoice::Previous,
            },
        )
        .at(10_000, connect(C0));
    let t = run(&s);
    upgrade(&t, C0);
    p2p_assert_eq!(t.code_changes, vec![ms(8000)]);
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .any(|s| s.path == Path::Relay && s.msg == Msg::Bye(Some(ByeReason::CodeChanged)))
    );
    p2p_assert!(t.exits(Side::Host, C0).iter().any(|&at| at >= ms(8000)));
    let attempts: Vec<_> = t
        .connects
        .iter()
        .filter(|c| c.at >= ms(9000))
        .map(|c| (c.code, c.accepted))
        .collect();
    p2p_assert_eq!(
        attempts,
        vec![(CodeChoice::Previous, false), (CodeChoice::Current, true)]
    );
}

/// 清单 #42（I18）：“断开此控制端”关两条路径、不换码，不影响其他控制端。
#[test]
fn disconnect_one_controller_keeps_code_and_closes_both_paths() {
    let s = Scenario::new(direct_host())
        .controller(direct_controller(1))
        .controller(direct_controller(2))
        .at(0, connect(C0))
        .at(0, connect(C1))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: true,
            },
        )
        .at(
            1000,
            E::Approve {
                controller: C1,
                direct: true,
            },
        );
    let s = gates(gates(s, 2000, C0), 2000, C1).at(8000, E::Kick { controller: C0 });
    let t = run(&s);
    upgrade(&t, C0);
    upgrade(&t, C1);
    p2p_assert!(
        t.msgs(Side::Host, C0)
            .any(|s| s.path == Path::Relay && s.msg == Msg::Bye(Some(ByeReason::Kicked)))
    );
    p2p_assert!(t.exits(Side::Host, C0).iter().any(|&at| at >= ms(8000)));
    p2p_assert!(t.session_ended(C0));
    p2p_assert!(!t.session_ended(C1) && t.exits(Side::Host, C1).is_empty());
    p2p_assert!(t.code_changes.is_empty());
}

/// 清单 #43（I7）：地址 / 令牌 / 密钥不出现在调试日志明文里。
#[test]
fn direct_secrets_are_redacted_in_debug_and_logs() {
    let t = run(&upgraded());
    upgrade(&t, C0);
    p2p_assert!(!t.secrets.is_empty(), "没有记录本次运行的直连机密");
    p2p_assert!(!t.logs.is_empty());
    for secret in &t.secrets {
        p2p_assert!(
            !t.logs.iter().any(|l| l.contains(secret.as_str())),
            "日志或调试输出里出现了直连机密的明文"
        );
    }
}

/// 清单 #44（SAS）：升级不因用户未比对校验串而阻塞或失败；两端显示同一个校验串。
#[test]
fn verification_string_is_displayed_not_required() {
    let t = run(&upgraded());
    let up = upgrade(&t, C0);
    p2p_assert!(up.completed <= ms(2000) + ms(12_000));
    let shown = |side| {
        t.sas_shown
            .iter()
            .find(|(s, c, _)| *s == side && *c == C0)
            .map(|(_, _, v)| v.clone())
    };
    let host = shown(Side::Host).p2p_expect("被控端没有显示校验串");
    p2p_assert!(!host.is_empty());
    p2p_assert_eq!(shown(Side::Controller), Some(host));
}
