//! tailcat-p2p 规格「先红验收测试清单」中会话 / 直连状态机的用例（`docs/specs/tailcat-p2p.md`）。
//!
//! 每个用例注明清单编号（该表从上往下数）。场景由 [`sim`] 驱动；直连实现接上之前是空操作驱动，
//! 用例因断言不满足而失败（`ASSERT:`，见 `support`）。Phase 1 按方案 (a)（`DirectGateOpen`）编写，
//! 方案 (b) 的清单 #17 / #18 与 K 驱动的 #120 是条件用例，不在本轮。

#[macro_use]
#[path = "support/mod.rs"]
mod support;
#[path = "direct/sim.rs"]
mod sim;

use std::time::Duration;

use airhand_proto::session::ByeReason;
use sim::{
    Approve, CodeChoice, ControllerConfig, DeclineReason, Event as E, GateCause, HostConfig, Msg,
    Path, Scenario, Screen, Side, TokenUse, Trace, Upgrade, UpgradePlan,
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
        ..ControllerConfig::default()
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
    // 正向对照：栅栏等待确实发生（≥ 1 s），期间 keep-alive 帧确实按 ≤ 250 ms 出站
    let &(_, fence_start, fence_end) = t
        .fence_waits
        .iter()
        .find(|(c, ..)| *c == C0)
        .p2p_expect("没有发生栅栏等待");
    p2p_assert!(
        fence_end - fence_start >= ms(1000),
        "栅栏等待只有 {:?}，夹具没有拉长它",
        fence_end - fence_start
    );
    let ka: Vec<Duration> = t
        .keep_alive_frames
        .iter()
        .filter(|(at, side, c)| {
            *side == Side::Host && *c == C0 && *at >= fence_start && *at <= fence_end
        })
        .map(|(at, ..)| *at)
        .collect();
    p2p_assert!(
        ka.len() >= 4,
        "栅栏等待期间只出站 {} 个 keep-alive 帧",
        ka.len()
    );
    p2p_assert!(
        ka.windows(2).all(|w| w[1] - w[0] <= ms(250)),
        "栅栏等待期间 keep-alive 帧间隔超过 250 ms"
    );
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
    let first = controller_quic
        .iter()
        .map(|d| d.at)
        .min()
        .unwrap_or_default();
    p2p_assert!(
        first <= gate_open + ms(50),
        "收到 DirectGateOpen 之后 {:?} 才发出首个直连 QUIC 包（应 ≤ 50 ms）",
        first.saturating_sub(gate_open)
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
    // 正向对照：随后 epoch 匹配的 DirectGateOpen 被接受并触发首包
    let matching = t
        .first(|s| {
            s.from == Side::Host
                && s.path == Path::Relay
                && matches!(s.msg, Msg::DirectGateOpen { .. })
        })
        .and_then(|s| s.delivered)
        .p2p_expect("被控端没有发出 epoch 匹配的 DirectGateOpen");
    p2p_assert!(
        first_quic >= matching && first_quic <= matching + ms(50),
        "匹配的 DirectGateOpen（{matching:?}）没有触发首包（首包 {first_quic:?}）"
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
    // 正向对照：同一夹具里启用直连时，helper 工厂确实被调用 1 次
    let t = run(&upgraded().run_for(20));
    p2p_assert_eq!(
        t.spawns(Side::Host, C0).len(),
        1,
        "启用直连时被控端 helper 工厂应被调用 1 次"
    );
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
    p2p_assert!(
        !t.spawns(Side::Host, C0).is_empty(),
        "授予直连后被控端 helper 没有启动（正向对照）"
    );
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
    let pongs = t
        .msgs(Side::Host, C0)
        .filter(|s| s.path == Path::Direct && s.msg == Msg::Pong && s.at > up)
        .count();
    p2p_assert!(pongs >= 100, "直连上只回了 {pongs} 个 Pong");
    // 正向对照（同一夹具）：随后 StopPongs 会触发回退
    let t = run(&upgraded()
        .screen(Screen::Static)
        .at(50_000, E::StopPongs { controller: C0 })
        .run_for(60));
    p2p_assert!(
        fallback_at(&t, Side::Controller, C0).is_some_and(|fb| fb >= ms(50_000)),
        "同一夹具里停止 Pong 后没有回退"
    );
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
    p2p_assert!(
        t.exits(Side::Host, C0).iter().any(|&at| at >= ms(8000)),
        "helper 崩溃没有发生（正向对照）"
    );
    p2p_assert!(any_fallback(&t, C0), "helper 崩溃后没有回退");
    p2p_assert!(!sent_bye(&t, Side::Host, C0) && !t.session_ended(C0));
    p2p_assert_eq!(t.last_sender_path(Side::Host, C0), Some(Path::Relay));
    p2p_assert!(
        t.frames
            .iter()
            .any(|(at, c, p)| *c == C0 && *p == Path::Relay && *at > ms(9000)),
        "崩溃之后会话在中转上没有继续收到画面"
    );
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
    // 正向对照：两侧闸门都打开之后，发送端确实切换
    let t = run(&upgraded());
    p2p_assert!(
        t.sent
            .iter()
            .any(|s| matches!(s.msg, Msg::DirectSwitch { .. })),
        "两侧闸门都打开后没有发 DirectSwitch"
    );
    p2p_assert_eq!(t.last_sender_path(Side::Host, C0), Some(Path::Direct));
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

// ---- r6.2 起新增的用例 ----

fn approve(c: u8) -> E {
    E::Approve {
        controller: c,
        direct: true,
    }
}

fn host_gate_closes(t: &Trace, c: u8) -> Vec<(Duration, Option<GateCause>)> {
    t.gates
        .iter()
        .filter(|g| g.side == Side::Host && g.controller == c && !g.open)
        .map(|g| (g.at, g.cause))
        .collect()
}

fn gate_closes(t: &Trace, side: Side, c: u8) -> Vec<(Duration, Option<GateCause>)> {
    t.gates
        .iter()
        .filter(|g| g.side == side && g.controller == c && !g.open)
        .map(|g| (g.at, g.cause))
        .collect()
}

fn first_relay_after(t: &Trace, from: Side, c: u8, after: Duration) -> Option<&Msg> {
    t.msgs(from, c)
        .find(|s| s.path == Path::Relay && s.at >= after)
        .map(|s| &s.msg)
}

/// 清单 #49（I3、O（被控端））：被控端→控制端方向不通、反方向正常：被控端 O（helper 内计时，Rust 只发进展心跳）
/// 在 ≤ RTT + 25 + 1000 + 100 + 50 ms 内关闭本端闸门并经中转发 `DirectFallback`；R 与 `CurAddr` 不触发。
#[test]
fn host_gate_closes_on_asymmetric_outbound_loss_within_2s() {
    for interfere in [false, true] {
        let mut s = upgraded().direct_rtt(400).at(
            8000,
            E::OneWayLoss {
                controller: C0,
                from: Side::Host,
            },
        );
        if interfere {
            s = s.at(
                8000,
                E::AckFramesWithoutProgress {
                    side: Side::Host,
                    controller: C0,
                },
            );
        }
        let t = run(&s);
        upgrade(&t, C0);
        p2p_assert!(
            t.ack_progress
                .iter()
                .filter(|(at, side, c)| *side == Side::Host && *c == C0 && *at < ms(8000))
                .count()
                >= 4,
            "断路之前被控端的确认事件没有前进（正向对照）"
        );
        let closes = host_gate_closes(&t, C0);
        let &(at, cause) = closes
            .iter()
            .find(|(at, _)| *at >= ms(8000))
            .p2p_expect("单向断路后被控端闸门没有关闭");
        p2p_assert!(
            at <= ms(8000 + 1575),
            "被控端闸门在断路 {:?} 后才关闭，上界 1575 ms（干扰计数={interfere}）",
            at - ms(8000)
        );
        p2p_assert_eq!(cause, Some(GateCause::Ack), "关闸条件应为 O");
        p2p_assert!(
            !closes
                .iter()
                .any(|(_, c)| matches!(c, Some(GateCause::RxSilence | GateCause::CurAddr))),
            "R 或 CurAddr 触发了：O 不是唯一的关闭条件"
        );
        p2p_assert!(
            fallback_at(&t, Side::Host, C0).is_some_and(|fb| fb >= at),
            "被控端关闸后没有经中转发 DirectFallback"
        );
    }
}

/// 清单 #50（I3、O（控制端））：控制端→被控端方向不通：控制端 O（或 D）在 2 s 内关闸并经中转发
/// `DirectFallback`；被控端因 R 同时关闭；断言 D + R + O 的组合上界。
#[test]
fn controller_gate_closes_on_asymmetric_outbound_loss_within_2s() {
    let s = upgraded().direct_rtt(400).at(
        8000,
        E::OneWayLoss {
            controller: C0,
            from: Side::Controller,
        },
    );
    let t = run(&s);
    upgrade(&t, C0);
    let ctl = gate_closes(&t, Side::Controller, C0);
    let &(at, cause) = ctl
        .iter()
        .find(|(at, _)| *at >= ms(8000))
        .p2p_expect("控制端闸门没有关闭");
    p2p_assert!(
        at <= ms(10_000),
        "控制端闸门在断路 {:?} 后才关闭",
        at - ms(8000)
    );
    p2p_assert!(
        matches!(cause, Some(GateCause::Ack | GateCause::Disco)),
        "控制端关闸条件 {cause:?}，应为 O 或 D"
    );
    p2p_assert!(
        fallback_at(&t, Side::Controller, C0).is_some_and(|fb| fb >= at),
        "控制端关闸后没有经中转发 DirectFallback"
    );
    let host = host_gate_closes(&t, C0);
    let &(hat, hcause) = host
        .iter()
        .find(|(at, _)| *at >= ms(8000))
        .p2p_expect("被控端闸门没有关闭");
    p2p_assert!(
        hat <= ms(10_000),
        "被控端闸门在断路 {:?} 后才关闭",
        hat - ms(8000)
    );
    p2p_assert_eq!(hcause, Some(GateCause::RxSilence), "被控端应因入站静默关闭");
}

fn first_close_after(t: &Trace, after: Duration) -> Option<(Side, Option<GateCause>)> {
    t.gates
        .iter()
        .filter(|g| !g.open && g.at >= after)
        .min_by_key(|g| g.at)
        .map(|g| (g.side, g.cause))
}

/// 清单 #51（I2、I3、O）：健康的 RTT 400 ms 直连（15 ms 抖动、静止桌面）上 D / R / O 的误触发：随机变体（i.i.d.、
/// G–E）只记录；定长完全丢包突发（前后各留 1 s 无丢包时段）× 10 个相位 × 在途拆分 d1 ∈ [0, RTT]（每 25 ms 一档）：
/// 240 ms 不关闸；400 / 500 / 700 ms 只记录；1000 / 1200 ms 必关；正向对照读突发前后 1 s 的无丢包时段；
/// 灵敏度对照：400 ms 突发在最不利相位至少一个条件关闸。
#[test]
fn ack_watchdog_not_triggered_on_healthy_high_rtt_lossy_path() {
    const NAME: &str = "ack_watchdog_not_triggered_on_healthy_high_rtt_lossy_path";
    let base = || {
        upgraded()
            .direct_rtt(400)
            .screen(Screen::Static)
            .at(2500, E::RelayJitter(ms(15)))
    };
    for (name, bursts) in [
        ("iid", seeded_bursts(1, 1800, 0.01)),
        ("ge", seeded_bursts(2, 1800, 0.0)),
    ] {
        let mut s = base().run_for(1800);
        for (at, len) in bursts {
            s = s.at_dur(
                at,
                E::BurstLoss {
                    controller: C0,
                    length: len,
                },
            );
        }
        let t = run(&s);
        let by_cause: Vec<_> = t
            .gates
            .iter()
            .filter(|g| !g.open)
            .map(|g| (g.side, g.cause))
            .collect();
        support::record(NAME, format!("{name}: 30 min 内按条件误关 {by_cause:?}"));
    }

    // 1 s 时段里两类进展各自至少有 250 ms 时隙数的 90%（4 个时隙 ⇒ ≥ 4 次，向上取整）
    let advancing = |t: &Trace, from: Duration| {
        let within = |at: &Duration| *at >= from && *at < from + ms(1000);
        let acks = t
            .ack_progress
            .iter()
            .filter(|(at, s, c)| *s == Side::Host && *c == C0 && within(at))
            .count();
        let inbound = t
            .inbound
            .iter()
            .filter(|(at, s, c)| *s == Side::Host && *c == C0 && within(at))
            .count();
        (acks, inbound)
    };
    let mut closed_400 = 0;
    for len in [240u64, 400, 500, 700, 1000, 1200] {
        for phase in 0..10u64 {
            for d1 in (0..=400u64).step_by(25) {
                let at = 10_000 + phase * 25;
                let t = run(&base().direct_split(d1).at(
                    at,
                    E::BurstLoss {
                        controller: C0,
                        length: ms(len),
                    },
                ));
                upgrade(&t, C0);
                let what = format!("{len} ms 突发（相位 {phase}、d1 = {d1} ms）");
                let (acks, inbound) = advancing(&t, ms(at - 1000));
                p2p_assert!(
                    acks >= 4 && inbound >= 4,
                    "{what}：突发前 1 s 的无丢包时段里确认 {acks} 次、入站 {inbound} 次（应各 ≥ 4）"
                );
                let closed = first_close_after(&t, ms(at));
                match len {
                    240 => {
                        p2p_assert!(closed.is_none(), "{what}：闸门关闭 {closed:?}");
                        let (acks, inbound) = advancing(&t, ms(at + len));
                        p2p_assert!(
                            acks >= 4 && inbound >= 4,
                            "{what}：突发后 1 s 的无丢包时段里确认 {acks} 次、入站 {inbound} 次（应各 ≥ 4）"
                        );
                    }
                    1000 | 1200 => p2p_assert!(closed.is_some(), "{what}：没有关闸"),
                    _ => {
                        if len == 400 && closed.is_some() {
                            closed_400 += 1;
                        }
                        support::record(NAME, format!("{what}: {closed:?}"));
                    }
                }
            }
        }
    }
    p2p_assert!(
        closed_400 > 0,
        "灵敏度对照：400 ms 突发在任何相位 / 拆分下都没有关闸"
    );
}

/// 固定种子的短突发序列：`p > 0` 时近似 i.i.d. 单包丢失（5 ms），否则 Gilbert–Elliott 坏态（均值 150 ms、截断 240 ms）。
fn seeded_bursts(seed: u64, secs: u64, p: f64) -> Vec<(Duration, Duration)> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = move || {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    };
    let mut out = Vec::new();
    let mut at = 5_000f64;
    while at < (secs * 1000) as f64 {
        if p > 0.0 {
            at += 5.0 / p * next().max(1e-9).ln().abs();
            out.push((Duration::from_millis(at as u64), ms(5)));
        } else {
            at += (20_000.0 * next().max(1e-9).ln().abs()).max(1500.0);
            let len = (150.0 * next().max(1e-9).ln().abs()).min(240.0);
            out.push((
                Duration::from_millis(at as u64),
                Duration::from_millis(len as u64),
            ));
        }
    }
    out
}

/// 清单 #52（I3、I13）：控制端闸门在已打开后关闭（任一条件）即经中转发 `DirectFallback`，被控端据此回退。
#[test]
fn controller_gate_close_triggers_direct_fallback() {
    for cause in [
        GateCause::Disco,
        GateCause::RxSilence,
        GateCause::Ack,
        GateCause::Command,
    ] {
        let t = run(&upgraded().at(
            8000,
            E::GateClose {
                side: Side::Controller,
                controller: C0,
                cause,
            },
        ));
        upgrade(&t, C0);
        let fb =
            fallback_at(&t, Side::Controller, C0).p2p_expect("控制端关闸后没有发 DirectFallback");
        p2p_assert!(
            fb >= ms(8000) && fb <= ms(8100),
            "{cause:?}: DirectFallback 在 {fb:?}"
        );
        p2p_assert_eq!(
            t.last_sender_path(Side::Host, C0),
            Some(Path::Relay),
            "{cause:?}: 被控端没有据此回退"
        );
    }
}

/// 清单 #53（I3、I13）：被控端闸门在已打开后关闭（`CurAddr` / R / O / H）即经中转发 `DirectFallback`，
/// 控制端据此回退；H 关闭的是尚未提交的 epoch：放弃该 epoch。
#[test]
fn host_gate_close_triggers_direct_fallback() {
    for cause in [GateCause::CurAddr, GateCause::RxSilence, GateCause::Ack] {
        let t = run(&upgraded().at(
            8000,
            E::GateClose {
                side: Side::Host,
                controller: C0,
                cause,
            },
        ));
        upgrade(&t, C0);
        let fb = fallback_at(&t, Side::Host, C0).p2p_expect("被控端关闸后没有发 DirectFallback");
        p2p_assert!(
            fb >= ms(8000) && fb <= ms(8100),
            "{cause:?}: DirectFallback 在 {fb:?}"
        );
        p2p_assert_eq!(t.last_sender_path(Side::Controller, C0), Some(Path::Relay));
    }
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(
            2000,
            E::GateOpen {
                side: Side::Host,
                controller: C0,
            },
        )
        .at(
            2100,
            E::GateClose {
                side: Side::Host,
                controller: C0,
                cause: GateCause::Handshake,
            },
        ));
    let epoch = t.offer_epoch(C0).p2p_expect("升级没有开始");
    p2p_assert!(
        fallback_at(&t, Side::Host, C0).is_some(),
        "H 关闸后没有发 DirectFallback"
    );
    p2p_assert!(
        t.abandoned
            .iter()
            .any(|(_, c, e, cause)| *c == C0 && *e == epoch && *cause == GateCause::Handshake),
        "H 关闸没有放弃尚未提交的 epoch"
    );
}

/// 清单 #56（I21）：只开 `approve = never`、未开 `[direct] enabled`：`granted.direct` 为假，不发 `DirectOffer`、不启动 helper。
#[test]
fn approve_never_direct_false_when_direct_disabled() {
    let host = HostConfig {
        approve: Approve::Never,
        ..HostConfig::default()
    };
    let t = run(&Scenario::new(host)
        .controller(direct_controller(1))
        .at(0, connect(C0)));
    p2p_assert!(
        t.msgs(Side::Host, C0).any(|s| s.msg
            == Msg::Hello {
                granted_direct: false
            }),
        "会话没有以 granted_direct = false 开始"
    );
    p2p_assert!(t.offer_epoch(C0).is_none() && t.helper_spawns.is_empty());
}

/// 清单 #57（I21）：对端 `ControllerHello.direct` 为假：never + `[direct] enabled` 下 `granted.direct` 为假。
#[test]
fn approve_never_direct_false_when_peer_lacks_direct() {
    let t = run(&Scenario::new(never_direct_host())
        .controller(ControllerConfig::default())
        .at(0, connect(C0)));
    p2p_assert!(
        t.msgs(Side::Host, C0).any(|s| s.msg
            == Msg::Hello {
                granted_direct: false
            }),
        "对端未声明直连时 granted_direct 应为假"
    );
    p2p_assert!(t.offer_epoch(C0).is_none() && t.helper_spawns.is_empty());
}

/// 清单 #62（I14）：`client_key` 为空 / 无法解析时 Rust 侧拒绝 Start（不得起一个放行所有客户端的 `Server`）。
#[test]
fn empty_client_key_refuses_start() {
    for key in ["", " ", "nodekey:", "nodekey:zz", "example"] {
        let t = run(&Scenario::new(direct_host())
            .controller(direct_controller(1))
            .at(0, connect(C0))
            .at(1000, approve(C0))
            .at(
                1500,
                E::DirectRequestWithKey {
                    controller: C0,
                    key: key.into(),
                },
            ));
        p2p_assert!(
            t.msgs(Side::Host, C0)
                .any(|s| matches!(s.msg, Msg::Hello { .. })),
            "会话没有开始"
        );
        p2p_assert!(
            t.server_news.is_empty(),
            "client_key {key:?} 无效，Rust 侧仍新建了 Server"
        );
        p2p_assert!(t.upgrades.is_empty() && !t.session_ended(C0));
    }
    let valid = format!("nodekey:{}", "ab".repeat(32));
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(
            1500,
            E::DirectRequestWithKey {
                controller: C0,
                key: valid,
            },
        ));
    p2p_assert_eq!(
        t.server_news.len(),
        1,
        "有效 client_key 时应新建一个 Server（正向对照）"
    );
}

fn token_case(token: TokenUse) {
    let t = run(&upgraded().at(
        8000,
        E::PresentToken {
            controller: C0,
            token,
        },
    ));
    upgrade(&t, C0);
    p2p_assert_eq!(t.token_checks, vec![true, false], "{token:?} 的令牌应被拒");
}

/// 清单 #63（I5）：令牌用于别的会话被拒。
#[test]
fn direct_token_wrong_session_rejected() {
    token_case(TokenUse::WrongSession);
}

/// 清单 #64（I5）：令牌过期（超过 `ttl_secs`）被拒。
#[test]
fn direct_token_expired_rejected() {
    token_case(TokenUse::Expired);
}

/// 清单 #65（I5、I13）：令牌的 `epoch` 不匹配被拒。
#[test]
fn direct_token_wrong_epoch_rejected() {
    token_case(TokenUse::WrongEpoch);
}

/// 清单 #66（I5、安全模型）：直连 QUIC 证书指纹与 `quic_fp` 不符时中止升级、保持中转。
#[test]
fn direct_cert_pin_mismatch_aborts_upgrade() {
    let t = run(&upgraded().at(1900, E::CertPinMismatch { controller: C0 }));
    p2p_assert!(t.offer_epoch(C0).is_some(), "升级没有开始");
    p2p_assert!(t.upgrades.is_empty(), "证书指纹不符仍完成了升级");
    p2p_assert!(
        t.sender_paths
            .iter()
            .all(|p| p.controller != C0 || p.path == Path::Relay),
        "证书指纹不符时切到了直连"
    );
    p2p_assert!(!t.session_ended(C0));
    let ok = run(&upgraded());
    upgrade(&ok, C0);
}

/// 清单 #69（I6、I13）：被控端先在中转发 `DirectSwitch`，再把 `DirectCommit` 作为直连控制流的第一条消息发出。
#[test]
fn host_fence_then_commit_is_first_direct_message() {
    let t = run(&upgraded());
    upgrade(&t, C0);
    let switch = t
        .msgs(Side::Host, C0)
        .find(|s| s.path == Path::Relay && matches!(s.msg, Msg::DirectSwitch { .. }))
        .p2p_expect("被控端没有在中转上发 DirectSwitch");
    let first_direct = t
        .msgs(Side::Host, C0)
        .find(|s| s.path == Path::Direct)
        .p2p_expect("被控端没有在直连上发任何控制消息");
    p2p_assert!(
        matches!(first_direct.msg, Msg::DirectCommit { .. }),
        "直连上的第一条消息是 {:?}，应为 DirectCommit",
        first_direct.msg
    );
    p2p_assert!(
        switch.at <= first_direct.at,
        "DirectCommit 早于中转上的 DirectSwitch"
    );
}

/// 清单 #70（I6）：升级切换前后输入事件顺序不变、不丢失：收到的条数等于 N（N > 0），序号单调、无缺口、无重复。
#[test]
fn input_order_preserved_across_switch() {
    let t = run(&upgraded()
        .at(
            1500,
            E::Input {
                controller: C0,
                count: 50,
            },
        )
        .at(
            2000,
            E::Input {
                controller: C0,
                count: 50,
            },
        )
        .at(
            4000,
            E::Input {
                controller: C0,
                count: 50,
            },
        ));
    upgrade(&t, C0);
    let seqs: Vec<u32> = t.inputs.iter().filter(|i| i.1 == C0).map(|i| i.2).collect();
    p2p_assert_eq!(seqs.len(), 150, "收到 {} 条输入，应为 150", seqs.len());
    p2p_assert!(
        seqs.iter().enumerate().all(|(i, &s)| s == i as u32),
        "输入序号不连续或乱序"
    );
    p2p_assert!(
        t.inputs.iter().any(|i| i.3 == Path::Relay) && t.inputs.iter().any(|i| i.3 == Path::Direct),
        "输入没有跨越切换（夹具前置）"
    );
}

/// 清单 #71（I6）：回退之后迟到的直连输入被丢弃。
#[test]
fn late_direct_input_after_fallback_is_discarded() {
    let t = run(&upgraded()
        .at(10_000, E::DirectPathFails { controller: C0 })
        .at(11_000, E::LateDirectInput { controller: C0 }));
    upgrade(&t, C0);
    let fb = fallback_at(&t, Side::Host, C0)
        .or(fallback_at(&t, Side::Controller, C0))
        .p2p_expect("路径失效后没有回退");
    p2p_assert_eq!(t.late_inputs_dropped, 1, "迟到的直连输入没有被丢弃");
    p2p_assert!(
        !t.inputs.iter().any(|i| i.3 == Path::Direct && i.0 > fb),
        "回退之后仍注入了直连输入"
    );
}

/// 清单 #72（I6）：回退后被控端 `release_all()`，不残留按下的键；先断言回退前按键确实处于按下状态。
#[test]
fn keys_released_after_fallback() {
    let t = run(&upgraded()
        .at(5000, E::KeyDown { controller: C0 })
        .at(10_000, E::DirectPathFails { controller: C0 }));
    upgrade(&t, C0);
    p2p_assert!(
        t.keys_down
            .iter()
            .any(|(at, c)| *c == C0 && *at < ms(10_000)),
        "回退前按键没有处于按下状态（夹具前置）"
    );
    let fb = fallback_at(&t, Side::Host, C0)
        .or(fallback_at(&t, Side::Controller, C0))
        .p2p_expect("路径失效后没有回退");
    p2p_assert!(
        t.release_all.iter().any(|(at, c)| *c == C0 && *at >= fb),
        "回退后没有 release_all()"
    );
}

/// 清单 #75（I6、F9）：双方都启用时会话升级到直连：画面经直连流到达、中转流字节计数不再增长。
#[test]
fn session_upgrades_to_direct_when_both_sides_enable() {
    let t = run(&upgraded().screen(Screen::Moving));
    let done = upgrade(&t, C0).completed;
    p2p_assert!(
        t.frames
            .iter()
            .any(|(at, c, p)| *c == C0 && *p == Path::Direct && *at > done),
        "升级后画面没有经直连到达"
    );
    let relay: Vec<u64> = t
        .relay_media_bytes
        .iter()
        .filter(|(at, c, _)| *c == C0 && *at > done + ms(1000))
        .map(|(_, _, b)| *b)
        .collect();
    p2p_assert!(relay.len() >= 2, "没有中转字节计数采样");
    p2p_assert!(
        relay.windows(2).all(|w| w[1] == w[0]),
        "升级后中转媒体字节仍在增长"
    );
}

/// 清单 #76（F2）：对端未启用直连时留在中转。
#[test]
fn peer_without_direct_enabled_stays_on_relay() {
    let t = run(&Scenario::new(direct_host())
        .controller(ControllerConfig::default())
        .screen(Screen::Moving)
        .at(0, connect(C0))
        .at(1000, approve(C0)));
    p2p_assert!(!t.frames.is_empty(), "会话没有画面");
    p2p_assert!(t.frames.iter().all(|f| f.2 == Path::Relay));
    p2p_assert!(t.offer_epoch(C0).is_none() && t.helper_spawns.is_empty());
}

/// 清单 #77（I8）：不认识的 `DirectOffer.kind` 回 `DirectDecline`，会话继续在中转。
#[test]
fn unknown_offer_kind_is_declined() {
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .screen(Screen::Moving)
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(
            1500,
            E::OfferKind {
                controller: C0,
                kind: 9,
            },
        ));
    p2p_assert!(
        t.msgs(Side::Controller, C0).any(|s| matches!(
            s.msg,
            Msg::DirectDecline {
                reason: DeclineReason::UnknownKind,
                ..
            }
        )),
        "不认识的 kind 没有回 DirectDecline"
    );
    p2p_assert!(t.upgrades.is_empty() && !t.session_ended(C0));
    p2p_assert!(
        t.frames
            .iter()
            .any(|f| f.2 == Path::Relay && f.0 > ms(2000))
    );
}

/// 清单 #78（I2）：直连流失败只触发回退，不结束会话；正向对照：reset 与回退确实发生，之后中转上仍有画面。
#[test]
fn direct_stream_failure_does_not_end_session() {
    let t = run(&upgraded()
        .screen(Screen::Moving)
        .at(8000, E::DirectStreamReset { controller: C0 }));
    upgrade(&t, C0);
    p2p_assert!(any_fallback(&t, C0), "直连流 reset 后没有回退");
    p2p_assert!(!t.session_ended(C0) && !sent_bye(&t, Side::Host, C0));
    p2p_assert!(
        t.frames
            .iter()
            .any(|f| f.2 == Path::Relay && f.0 > ms(9000)),
        "回退后中转上没有画面"
    );
}

/// 清单 #79（I2、I10）：中转连接断开时即使直连正常也结束会话；正向对照：断开前确实在直连上运行。
#[test]
fn relay_loss_ends_session_even_when_direct() {
    let t = run(&upgraded()
        .screen(Screen::Moving)
        .at(8000, E::RelayDisconnect { controller: C0 }));
    upgrade(&t, C0);
    p2p_assert!(
        t.frames
            .iter()
            .any(|f| f.2 == Path::Direct && f.0 < ms(8000)),
        "断开前不在直连上运行"
    );
    p2p_assert!(t.session_ended(C0), "中转断开后会话没有结束");
}

/// 清单 #83（I12、I16）：`HostHello` 之前的 `DirectRequest` 被丢弃。
#[test]
fn request_before_host_hello_is_dropped() {
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(500, E::ForceDirectRequest { controller: C0 })
        .at(5000, approve(C0)));
    p2p_assert!(
        t.requests
            .iter()
            .any(|(at, c, _)| *c == C0 && *at < ms(5000)),
        "夹具没有在 Hello 之前发出 DirectRequest"
    );
    p2p_assert!(
        !t.msgs(Side::Host, C0)
            .any(|s| s.at < ms(5000) && s.msg.is_direct_control()),
        "Hello 之前的 DirectRequest 得到了应答"
    );
    p2p_assert!(t.offer_epoch(C0).is_some(), "同意之后的正常升级没有开始");
}

/// 清单 #84（I12、I16）：未取得直连同意的 `DirectRequest` 回 `DirectDecline`；正向对照：授予直连后 `DirectOffer` 确实发出。
#[test]
fn direct_request_without_direct_grant_is_declined() {
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: false,
            },
        )
        .at(3000, E::ForceDirectRequest { controller: C0 }));
    p2p_assert_eq!(
        t.declines(C0).iter().map(|d| d.1).collect::<Vec<_>>(),
        vec![DeclineReason::NotGranted]
    );
    p2p_assert!(t.offer_epoch(C0).is_none());
    let ok = run(&upgraded());
    p2p_assert!(ok.offer_epoch(C0).is_some(), "授予直连后没有 DirectOffer");
}

/// 清单 #85（I13）：控制端等不到 `DirectCommit`，在 T = max(2 s, 4 × 直连 RTT) 内经中转发 `DirectFallback`。
#[test]
fn commit_lost_controller_falls_back_within_bound() {
    let t = run(&upgraded().at(1900, E::CommitLost { controller: C0 }));
    let switch = t
        .msgs(Side::Host, C0)
        .find(|s| matches!(s.msg, Msg::DirectSwitch { .. }))
        .and_then(|s| s.delivered)
        .p2p_expect("控制端没有收到 DirectSwitch");
    let fb = fallback_at(&t, Side::Controller, C0).p2p_expect("等不到 DirectCommit 却没有回退");
    p2p_assert!(
        fb > switch && fb <= switch + ms(2000) + ms(100),
        "回退在 {fb:?}，栅栏在 {switch:?}"
    );
    p2p_assert!(!t.session_ended(C0));
}

/// 清单 #86（I13）：收到对端 `DirectFallback` 立即回退并丢弃该 epoch 的直连控制消息。
#[test]
fn fallback_message_forces_peer_to_fall_back() {
    let t = run(&upgraded().at(
        8000,
        E::GateClose {
            side: Side::Controller,
            controller: C0,
            cause: GateCause::Disco,
        },
    ));
    let epoch = upgrade(&t, C0).epoch;
    let fb = t
        .msgs(Side::Controller, C0)
        .find(|s| s.path == Path::Relay && matches!(s.msg, Msg::DirectFallback { .. }))
        .and_then(|s| s.delivered)
        .p2p_expect("控制端的 DirectFallback 没有送达");
    p2p_assert_eq!(t.last_sender_path(Side::Host, C0), Some(Path::Relay));
    p2p_assert!(
        !t.msgs(Side::Host, C0).any(|s| s.path == Path::Direct
            && s.at > fb
            && matches!(s.msg, Msg::DirectCommit { epoch: e } | Msg::DirectSwitch { epoch: e } if e == epoch)),
        "回退之后仍在处理该 epoch 的直连控制消息"
    );
}

/// 清单 #87（I13）：被控端超时放弃的 epoch 上，迟到的 `DirectRequest` 被忽略。
#[test]
fn late_request_after_host_timeout_is_ignored() {
    let t = run(&Scenario::new(direct_host())
        .controller(direct_controller(1))
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(15_000, E::LateDirectRequest { controller: C0 }));
    let epoch = t.offer_epoch(C0).p2p_expect("升级没有开始");
    p2p_assert!(
        t.abandoned
            .iter()
            .any(|(_, c, e, _)| *c == C0 && *e == epoch),
        "被控端没有超时放弃该 epoch（夹具前置）"
    );
    p2p_assert!(
        !t.msgs(Side::Host, C0)
            .any(|s| s.at >= ms(15_000) && s.msg.is_direct_control()),
        "迟到的 DirectRequest 得到了应答"
    );
}

#[derive(serde::Deserialize)]
struct NoopResult {
    results: std::collections::BTreeMap<String, NoopCase>,
}

#[derive(serde::Deserialize)]
struct NoopCase {
    status: String,
    assert: Option<bool>,
}

/// 清单 #98（S2-2 元用例）：核对 A 级登记表与两个空操作作业的结果文件——元用例恰为 #98、#99、#139；
/// “适用”的非元用例都是断言失败；不在名单里的“不适用”用例通过；“不适用”的非元用例登记了替代正向对照。
/// 本元用例自身从第一个先红 PR 起为绿。
#[test]
fn negative_assertions_fail_on_noop_driver() {
    let root = support::repo_root();
    let reg = std::fs::read_to_string(root.join("tests/p2p-a-registry.txt"))
        .p2p_expect("读取 A 级登记表");
    let list = std::fs::read_to_string(root.join("tests/p2p-expected-red.txt"))
        .p2p_expect("读取预期红名单");
    let listed: std::collections::BTreeSet<&str> = list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();
    let mut metas = Vec::new();
    let mut entries = Vec::new();
    for line in reg
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let f: Vec<&str> = line.split(" | ").collect();
        p2p_assert_eq!(f.len(), 6, "登记表行格式不对：{line}");
        let meta = f[1].ends_with("元用例");
        if meta {
            metas.push(f[0]);
        }
        let lenient = f[3] == "lenient=适用";
        let strict = f[4] == "strict=适用";
        if !meta && !(lenient && strict) {
            p2p_assert!(
                f[5] != "-",
                "{} 在某个极性下不适用，但没有登记替代正向对照",
                f[0]
            );
        }
        entries.push((f[0], meta, lenient, strict));
    }
    metas.sort();
    p2p_assert_eq!(
        metas,
        vec![
            "go p2p/gate_test.go::TestNegativeAssertionsFailOnNoopGate",
            "node tests/p2p-expected-red.mjs::expected_red_gate_rejects_unexpected_failures_passes_and_wrong_reason_red",
            "rust crates/airhand-link/tests/direct.rs::negative_assertions_fail_on_noop_driver",
        ],
        "元用例例外名单只能是 #98、#99、#139"
    );
    for pol in ["lenient", "strict"] {
        let Ok(raw) = std::fs::read_to_string(root.join(format!("target/p2p-noop-{pol}.json")))
        else {
            continue;
        };
        let res: NoopResult = p2p_ok!(serde_json::from_str(&raw), "解析 p2p-noop-{pol}.json");
        for &(id, meta, lenient, strict) in &entries {
            if meta || !id.starts_with("rust ") {
                continue;
            }
            let Some(got) = res.results.get(id) else {
                continue;
            };
            let applicable = if pol == "lenient" { lenient } else { strict };
            if applicable {
                p2p_assert!(
                    got.status == "fail" && got.assert == Some(true),
                    "{pol}: {id} 应断言失败，结果为 {} / {:?}",
                    got.status,
                    got.assert
                );
            } else if !listed.contains(id) {
                p2p_assert_eq!(
                    got.status.as_str(),
                    "pass",
                    "{pol}: 不在名单里的“不适用”用例 {id} 应通过"
                );
            }
        }
    }
}

/// 回退后的重新升级：提交成功后注入路径故障，之后每次再升级都失败。
fn retry_period(plan: UpgradePlan, run_secs: u64) -> Scenario {
    let mut c = direct_controller(1);
    c.plan = plan;
    let s = Scenario::new(direct_host())
        .controller(c)
        .at(0, connect(C0))
        .at(1000, approve(C0));
    let s = if plan == UpgradePlan::CommitThenFailRetries {
        gates(s, 2000, C0).at(8000, E::DirectPathFails { controller: C0 })
    } else {
        s.at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
    };
    s.run_for(run_secs)
}

fn spaced_by_30s(at: &[Duration]) -> bool {
    at.windows(2)
        .all(|w| w[1] - w[0] >= Duration::from_secs(30) && w[1] - w[0] <= Duration::from_secs(31))
}

/// 清单 #104（I25、§2 重试）：已提交的直连回退后进入重试期：每 30 s 尝试一次，直到某次提交成功或本会话累计 3 次；
/// 每个新 epoch 无论结局如何都计 1 次。
#[test]
fn reupgrade_attempts_capped_at_3_per_session_every_30s() {
    let t = run(&retry_period(UpgradePlan::CommitThenFailRetries, 200));
    let first = upgrade(&t, C0).epoch;
    let retries: Vec<(Duration, u32)> = t
        .offers(C0)
        .into_iter()
        .filter(|(_, e)| *e > first)
        .collect();
    p2p_assert_eq!(
        retries.len(),
        3,
        "重试期内尝试了 {} 次，应为 3 次",
        retries.len()
    );
    let at: Vec<Duration> = retries.iter().map(|r| r.0).collect();
    p2p_assert!(spaced_by_30s(&at), "重试间隔不是 30 s：{at:?}");
    p2p_assert!(
        retries.windows(2).all(|w| w[1].1 > w[0].1),
        "每次重试应使用新 epoch"
    );
    p2p_assert_eq!(
        t.retry_counts.iter().map(|r| r.2).max(),
        Some(3),
        "累计重试计数应到 3"
    );
    p2p_assert!(!t.session_ended(C0) && !sent_bye(&t, Side::Host, C0));
}

/// 清单 #105（§2 重试（OQ-20））：第一次升级在控制端闸门从未打开时失败（升级超时 / 只有 DERP / `DirectDecline`；
/// 含被控端已判 direct、控制端始终没判 direct）不自动重试，会话保持中转，不计入 3 次。
#[test]
fn initial_upgrade_failure_not_retried() {
    let base = || {
        Scenario::new(direct_host())
            .controller(direct_controller(1))
            .screen(Screen::Moving)
            .at(0, connect(C0))
            .at(1000, approve(C0))
            .run_for(120)
    };
    let variants = [
        ("升级超时", base()),
        (
            "只有 DERP",
            base().at(
                2000,
                E::GateClose {
                    side: Side::Controller,
                    controller: C0,
                    cause: GateCause::DerpOnly,
                },
            ),
        ),
        (
            "DirectDecline",
            base().at(
                1500,
                E::OfferKind {
                    controller: C0,
                    kind: 9,
                },
            ),
        ),
        (
            "被控端已判 direct",
            base().at(
                2000,
                E::GateOpen {
                    side: Side::Host,
                    controller: C0,
                },
            ),
        ),
    ];
    for (name, s) in variants {
        let t = run(&s);
        p2p_assert_eq!(t.offers(C0).len(), 1, "{name}：初次失败后又尝试了升级");
        p2p_assert!(t.retry_counts.is_empty(), "{name}：初次失败被计入重试次数");
        p2p_assert!(
            t.frames
                .iter()
                .any(|f| f.2 == Path::Relay && f.0 > ms(60_000)),
            "{name}：会话没有在中转上继续"
        );
    }
    // 对照组：闸门打开之后被 H 放弃 ⇒ 进入重试期
    let t = run(&retry_period(UpgradePlan::AbandonAfterGateOpen, 120));
    p2p_assert!(
        t.offers(C0).len() > 1,
        "对照组：闸门曾打开的放弃没有进入重试期"
    );
    let ok = run(&upgraded());
    upgrade(&ok, C0);
}

/// 清单 #106（I25、§2）：写超时回退后重新升级用新 epoch，并在新 helper 进程里进行。
#[test]
fn fallback_then_reupgrade_uses_new_epoch_and_new_process() {
    let t = run(&upgraded()
        .at(
            8000,
            E::HelperWriteStall {
                side: Side::Host,
                controller: C0,
            },
        )
        .run_for(60));
    let old = upgrade(&t, C0).epoch;
    let fb = fallback_at(&t, Side::Host, C0).p2p_expect("写超时后没有回退");
    let (re_at, _) = *t
        .offers(C0)
        .iter()
        .find(|(_, e)| *e > old)
        .p2p_expect("回退后没有用新 epoch 重新升级");
    let old_exit = t
        .exits(Side::Host, C0)
        .into_iter()
        .find(|&at| at >= fb)
        .p2p_expect("旧 helper 没有结束");
    p2p_assert!(
        t.spawns(Side::Host, C0)
            .iter()
            .any(|&at| at > old_exit && at <= re_at),
        "重新升级没有在新 helper 进程里进行"
    );
    p2p_assert!(
        t.max_live_helpers(Side::Host, C0) <= 1,
        "同一会话同时存活了多个 helper"
    );
}

/// 清单 #110（I7）：Rust 侧不把 helper 的 stderr 原样写入日志，只记录退出码与脱敏摘要。
#[test]
fn helper_stderr_not_logged_verbatim() {
    let leak = "magicsock: endpoint 203.0.113.10:41641 for [abcde]";
    let t = run(&upgraded()
        .at(
            8000,
            E::HelperStderr {
                side: Side::Host,
                controller: C0,
                text: leak.into(),
            },
        )
        .at(
            9000,
            E::HelperCrash {
                side: Side::Host,
                controller: C0,
            },
        ));
    upgrade(&t, C0);
    p2p_assert!(
        t.logs
            .iter()
            .any(|l| l.contains("helper") && l.contains("exit")),
        "日志里没有 helper 退出码摘要（正向对照）"
    );
    p2p_assert!(
        !t.logs
            .iter()
            .any(|l| l.contains(leak) || l.contains("203.0.113.10")),
        "helper 的 stderr 被原样写进了日志"
    );
}

/// 清单 #111（I7、SAS）：校验串、计算中间值不进日志与 `Debug`。
#[test]
fn verification_string_not_logged() {
    let t = run(&upgraded());
    upgrade(&t, C0);
    let (_, _, sas) = t.sas_shown.first().p2p_expect("没有显示校验串");
    p2p_assert!(!t.logs.is_empty(), "没有日志");
    p2p_assert!(
        !t.logs.iter().any(|l| l.contains(sas.as_str())),
        "校验串进了日志"
    );
}

/// 清单 #115（§2 存活检测）：被控端 K 从收到首个直连 `Ping` 起算；首个 `Ping` 到达之前用
/// “提交时刻 + 栅栏等待上限 + K”作为总期限。K = max(1.5 s, 4 × 250 ms + 2 × RTT)。
#[test]
fn host_liveness_k_starts_at_first_direct_ping() {
    let k = ms(1500);
    let t = run(&upgraded().at(10_000, E::StopPings { controller: C0 }));
    upgrade(&t, C0);
    let last_ping = t
        .msgs(Side::Controller, C0)
        .filter(|s| s.path == Path::Direct && s.msg == Msg::Ping && s.at < ms(10_000))
        .filter_map(|s| s.delivered)
        .max()
        .p2p_expect("直连上没有 Ping");
    let fb = fallback_at(&t, Side::Host, C0).p2p_expect("Ping 停止后被控端没有回退");
    p2p_assert!(
        fb >= last_ping + k && fb <= last_ping + k + ms(250),
        "被控端在最后一个 Ping 之后 {:?} 回退，应为 K",
        fb.saturating_sub(last_ping)
    );

    let t = run(&upgraded().at(1500, E::StopPings { controller: C0 }));
    let commit = t
        .msgs(Side::Host, C0)
        .find(|s| matches!(s.msg, Msg::DirectCommit { .. }))
        .map(|s| s.at)
        .p2p_expect("被控端没有提交");
    let fb = fallback_at(&t, Side::Host, C0).p2p_expect("首个 Ping 始终没来，被控端没有回退");
    p2p_assert!(
        fb >= commit + ms(1000) + k && fb <= commit + ms(1000) + k + ms(250),
        "首个 Ping 之前的总期限不对：提交后 {:?} 回退",
        fb - commit
    );
}

/// 清单 #116（I25）：helper 周期性上报写进度心跳；`max(1 s, 2 × write_timeout_ms)` 内没有心跳即结束 helper 并回退。
#[test]
fn helper_progress_silence_triggers_restart() {
    let t = run(&upgraded().at(
        8000,
        E::HelperProgressSilence {
            side: Side::Host,
            controller: C0,
        },
    ));
    upgrade(&t, C0);
    p2p_assert!(
        t.progress_heartbeats
            .iter()
            .any(|(at, s, c)| *s == Side::Host && *c == C0 && *at < ms(8000)),
        "静默之前 helper 没有上报过心跳（正向对照）"
    );
    let exit = t
        .exits(Side::Host, C0)
        .into_iter()
        .find(|&at| at >= ms(8000))
        .p2p_expect("心跳停止后 helper 没有被结束");
    p2p_assert!(
        exit <= ms(8000 + 1000 + 250),
        "helper 在心跳停止 {:?} 后才被结束",
        exit - ms(8000)
    );
    p2p_assert!(fallback_at(&t, Side::Host, C0).is_some(), "没有回退");
}

/// 清单 #124（I3、§2 H）：首个入站之后、直连 QUIC 握手 H 内未完成：helper 关闸（粘性）、放弃该 epoch、
/// 经中转发 `DirectFallback`，会话继续在中转；R 不触发；对照 A：握手完成则不关闸、H 被撤销。
#[test]
fn host_gate_closes_if_handshake_stalls_after_first_inbound() {
    let t = run(&upgraded()
        .at(1900, E::HandshakeStall { controller: C0 })
        .run_for(40));
    let epoch = t.offer_epoch(C0).p2p_expect("升级没有开始");
    let &(at, cause) = host_gate_closes(&t, C0)
        .first()
        .p2p_expect("握手卡住，被控端闸门没有关闭");
    p2p_assert_eq!(cause, Some(GateCause::Handshake), "关闸条件应为 H");
    p2p_assert!(
        at >= ms(2000 + 1500) && at <= ms(2000 + 1510 + 50),
        "H 在 {at:?} 关闸，应在首个入站 + 1500 … 1510 ms"
    );
    p2p_assert!(
        t.abandoned
            .iter()
            .any(|(_, c, e, cs)| *c == C0 && *e == epoch && *cs == GateCause::Handshake),
        "epoch 没有被放弃"
    );
    p2p_assert!(
        matches!(
            first_relay_after(&t, Side::Host, C0, at),
            Some(Msg::DirectFallback { .. })
        ),
        "DirectFallback 不是回退后中转上的第一条消息"
    );
    p2p_assert!(!sent_bye(&t, Side::Host, C0) && !t.session_ended(C0));
    p2p_assert!(!t.retry_counts.is_empty(), "H 放弃后没有进入重试期");
    let ok = run(&upgraded());
    upgrade(&ok, C0);
    p2p_assert!(
        ok.watchdog_disarmed
            .iter()
            .any(|(_, s, c)| *s == Side::Host && *c == C0),
        "对照 A：握手完成后 H 没有被撤销"
    );
}

/// 清单 #127（I2、I3、§2 H、OQ-20）：H 放弃只放弃 direct 尝试、不使会话失败；第一次升级的 H 放弃进入重试期并计 1 次，
/// 之后每 30 s 再试，直到提交成功或累计 3 次（即最多再试 2 次）。
#[test]
fn handshake_watchdog_abandon_keeps_session_on_relay_and_counts_by_phase() {
    let t = run(&retry_period(UpgradePlan::AbandonAfterGateOpen, 200));
    let offers: Vec<Duration> = t.offers(C0).iter().map(|o| o.0).collect();
    p2p_assert_eq!(
        offers.len(),
        3,
        "H 放弃后共尝试 {} 个 epoch，应为 3",
        offers.len()
    );
    p2p_assert!(spaced_by_30s(&offers), "再试间隔不是 30 s：{offers:?}");
    p2p_assert!(
        t.abandoned
            .iter()
            .filter(|a| a.3 == GateCause::Handshake)
            .count()
            == 3,
        "每个 epoch 都应被 H 放弃"
    );
    p2p_assert!(!t.session_ended(C0) && !sent_bye(&t, Side::Host, C0));
}

/// 清单 #128（§2 重试（S2-11））：回退后进入重试期，失败的重新升级（超时 / 只有 DERP / H 放弃）也继续重试并计数。
#[test]
fn failed_reupgrade_is_retried_until_cap() {
    let t = run(&retry_period(UpgradePlan::CommitThenFailRetries, 200));
    let first = upgrade(&t, C0).epoch;
    let retries: Vec<Duration> = t
        .offers(C0)
        .iter()
        .filter(|o| o.1 > first)
        .map(|o| o.0)
        .collect();
    p2p_assert_eq!(
        retries.len(),
        3,
        "失败的重新升级之后只重试了 {} 次",
        retries.len()
    );
    p2p_assert!(spaced_by_30s(&retries), "{retries:?}");
    let counts: Vec<u32> = t.retry_counts.iter().map(|r| r.2).collect();
    p2p_assert_eq!(counts, vec![1, 2, 3], "每次失败的重新升级都应计数");
}

/// 清单 #141（I3、§2 O）：O 的计时基准 = 握手完成命令到达 helper 的时刻，同时撤销 H。A：握手完成后从未有确认；
/// B：ACK 帧计数增长但探测流不前进；C：有进展后断路。错误实现（基准取首个确认）在 A、B 里永不关闸。
#[test]
fn host_ack_watchdog_armed_at_handshake_completion() {
    for (name, ev_at, ev) in [
        (
            "A",
            1990,
            E::OneWayLoss {
                controller: C0,
                from: Side::Host,
            },
        ),
        (
            "B",
            1990,
            E::AckFramesWithoutProgress {
                side: Side::Host,
                controller: C0,
            },
        ),
    ] {
        let t = run(&upgraded().at(ev_at, ev));
        let hc = t
            .handshake_complete
            .iter()
            .find(|(_, s, c)| *s == Side::Host && *c == C0)
            .map(|h| h.0)
            .p2p_expect("握手完成命令没有送达被控端 helper");
        p2p_assert!(
            !t.ack_progress
                .iter()
                .any(|(at, s, _)| *s == Side::Host && *at > hc),
            "{name}：握手完成后出现了确认，场景不能判出“基准取首个确认”的错误实现"
        );
        let &(at, cause) = host_gate_closes(&t, C0).first().p2p_expect("O 没有关闸");
        p2p_assert_eq!(cause, Some(GateCause::Ack), "{name}：关闸条件应为 O");
        p2p_assert!(
            at >= hc + ms(1000) && at <= hc + ms(1100),
            "{name}：关闸在命令送达后 {:?}，应在 1000–1100 ms",
            at.saturating_sub(hc)
        );
    }
    let t = run(&upgraded().at(
        8000,
        E::OneWayLoss {
            controller: C0,
            from: Side::Host,
        },
    ));
    let last = t
        .ack_progress
        .iter()
        .filter(|(_, s, c)| *s == Side::Host && *c == C0)
        .map(|a| a.0)
        .max()
        .p2p_expect("C：断路前没有进展");
    let &(at, _) = host_gate_closes(&t, C0).first().p2p_expect("C：O 没有关闸");
    p2p_assert!(
        at <= last + ms(1000 + 100 + 50) && at <= ms(8000 + 1575),
        "C：关闸在 {at:?}"
    );
    let ok = run(&upgraded());
    let hc = ok
        .handshake_complete
        .iter()
        .find(|h| h.1 == Side::Host)
        .map(|h| h.0)
        .p2p_expect("对照组：没有握手完成命令");
    let probe = ok
        .first_probe
        .iter()
        .find(|p| p.1 == Side::Host)
        .map(|p| p.0)
        .p2p_expect("对照组：握手完成后没有发探测流");
    p2p_assert!(
        probe <= hc + ms(20 + 25 + 50),
        "首条探测流在命令后 {:?} 才发出",
        probe - hc
    );
    p2p_assert!(host_gate_closes(&ok, C0).is_empty(), "对照组被关闸");
    p2p_assert!(
        ok.watchdog_disarmed
            .iter()
            .any(|w| w.1 == Side::Host && w.0 >= hc),
        "握手完成命令后 H 没有被撤销"
    );
}

/// 清单 #144（§2 重试期规则）：提交之前、控制端闸门曾打开之后的任何放弃进入重试期，被放弃的 epoch 计 1 次，
/// 之后每 30 s 一次直到成功或累计 3 次；从未达到 direct 的失败不重试；任一时刻最多一个 epoch 的闸门打开。
#[test]
fn gate_opened_abandon_before_commit_enters_retry_period_and_counts_once() {
    let causes = [
        GateCause::Handshake,
        GateCause::Disco,
        GateCause::RxSilence,
        GateCause::Ack,
        GateCause::CurAddr,
        GateCause::CommitTimeout,
        GateCause::GateOpenTimeout,
    ];
    for cause in causes {
        let mut c = direct_controller(1);
        c.plan = UpgradePlan::AbandonAfterGateOpen;
        let s = Scenario::new(direct_host())
            .controller(c)
            .at(0, connect(C0))
            .at(1000, approve(C0))
            .at(1500, E::RelayLatency(ms(1500)))
            .at(
                2000,
                E::GateOpen {
                    side: Side::Controller,
                    controller: C0,
                },
            )
            .at(
                2100,
                E::GateClose {
                    side: Side::Controller,
                    controller: C0,
                    cause,
                },
            )
            .run_for(200);
        let t = run(&s);
        let offers: Vec<Duration> = t.offers(C0).iter().map(|o| o.0).collect();
        p2p_assert_eq!(
            offers.len(),
            3,
            "{cause:?}：共 {} 个 epoch，应为 3",
            offers.len()
        );
        p2p_assert!(spaced_by_30s(&offers), "{cause:?}：{offers:?}");
        p2p_assert_eq!(
            t.retry_counts.first().map(|r| r.2),
            Some(1),
            "{cause:?}：被放弃的 epoch 应计 1 次"
        );
        p2p_assert!(t.requests.len() <= 3 && t.derp_contacts(Side::Controller, C0).len() <= 3);
        p2p_assert!(!t.session_ended(C0) && !sent_bye(&t, Side::Host, C0));
        let mut open: Vec<(Duration, u32, bool)> = t
            .gates
            .iter()
            .filter(|g| g.controller == C0 && g.side == Side::Controller)
            .map(|g| (g.at, g.epoch, g.open))
            .collect();
        open.sort_by_key(|g| g.0);
        let mut cur: Option<u32> = None;
        for (_, e, o) in open {
            if o {
                p2p_assert!(
                    cur.is_none() || cur == Some(e),
                    "{cause:?}：两个 epoch 的闸门同时打开"
                );
                cur = Some(e);
            } else if cur == Some(e) {
                cur = None;
            }
        }
    }
    for (name, s) in [
        (
            "控制端闸门从未打开（升级超时）",
            Scenario::new(direct_host())
                .controller(direct_controller(1))
                .at(0, connect(C0))
                .at(1000, approve(C0))
                .at(
                    2000,
                    E::GateOpen {
                        side: Side::Host,
                        controller: C0,
                    },
                )
                .run_for(120),
        ),
        (
            "只有 DERP",
            Scenario::new(direct_host())
                .controller(direct_controller(1))
                .at(0, connect(C0))
                .at(1000, approve(C0))
                .at(
                    2000,
                    E::GateClose {
                        side: Side::Controller,
                        controller: C0,
                        cause: GateCause::DerpOnly,
                    },
                )
                .run_for(120),
        ),
    ] {
        let t = run(&s);
        p2p_assert_eq!(t.offers(C0).len(), 1, "{name}：不应重试");
    }
    let mut c = direct_controller(1);
    c.plan = UpgradePlan::AbandonAfterGateOpen;
    let t = run(&Scenario::new(direct_host())
        .controller(c)
        .at(0, connect(C0))
        .at(1000, approve(C0))
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .at(10_000, E::RevokeDirect { controller: C0 })
        .run_for(120));
    p2p_assert_eq!(t.offers(C0).len(), 1, "撤销直连权限后仍在重试");
    let t = run(&gates(
        Scenario::new(direct_host())
            .controller(direct_controller(1))
            .at(0, connect(C0))
            .at(1000, approve(C0))
            .at(
                2000,
                E::GateOpen {
                    side: Side::Controller,
                    controller: C0,
                },
            )
            .at(
                2100,
                E::GateClose {
                    side: Side::Controller,
                    controller: C0,
                    cause: GateCause::Disco,
                },
            )
            .run_for(150),
        33_000,
        C0,
    ));
    upgrade(&t, C0);
    p2p_assert_eq!(t.offers(C0).len(), 2, "第二次尝试提交成功后不应再尝试");
}

// ---- OQ-27：被控端执行重试上限（#146–#148，r6.5.2）----
//
// 被控端规则：(a) 既有检查（同意、并发上限）；(b) 会话里第一个被受理的请求是初次升级、不计入，其后每个被受理的请求
// 算一次再升级，已达 3 次 ⇒ `retry_limit`；(c) 距上一个被受理请求的到达时刻 t_last < 30 s ⇒ `retry_interval`，
// 不论上一个 epoch 是否已结束；(d) 否则受理。`a` 与 `t_last` 在 Rust 侧的会话状态里（不在 helper 里），按会话串行、
// 原子地判定与更新，只在会话结束时丢弃：I25 重启 helper、收回再授予都不清零。计数口径：每会话一个 helper 进程、
// 每个 epoch 在进程内新建 `Server`：受理个数 = `Server` 新建次数 = DERP 联系次数；helper 启动次数 ≤ 1 + I25 重启次数。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    Accepted,
    Declined(DeclineReason),
    None,
}

fn scripted_controller() -> ControllerConfig {
    ControllerConfig {
        plan: UpgradePlan::Scripted,
        ..direct_controller(1)
    }
}

const T0: u64 = 5_000;

/// 已取得直连同意的会话，脚本化控制端在 `T0 + 偏移`（毫秒）各发一个新 epoch 的 `DirectRequest`
fn scripted_session(host: HostConfig, offsets: &[u64]) -> Scenario {
    let mut s = Scenario::new(host)
        .controller(scripted_controller())
        .controller(scripted_controller())
        .at(0, connect(C0))
        .at(1000, approve(C0));
    for &o in offsets {
        s = s.at(T0 + o, E::ScriptedDirectRequest { controller: C0 });
    }
    s.run_for((T0 + offsets.iter().max().copied().unwrap_or(0)) / 1000 + 30)
}

/// 每个请求的应答：请求时刻到下一个请求之前，被控端发出的第一条 `DirectOffer` / `DirectDecline`
fn replies(t: &Trace, c: u8, at: &[Duration]) -> Vec<Reply> {
    at.iter()
        .enumerate()
        .map(|(i, &from)| {
            let until = at.get(i + 1).copied().unwrap_or(Duration::MAX);
            t.msgs(Side::Host, c)
                .filter(|s| s.at >= from && s.at < until)
                .find_map(|s| match s.msg {
                    Msg::DirectOffer { .. } => Some(Reply::Accepted),
                    Msg::DirectDecline { reason, .. } => Some(Reply::Declined(reason)),
                    _ => None,
                })
                .unwrap_or(Reply::None)
        })
        .collect()
}

fn abs(offsets: &[u64]) -> Vec<Duration> {
    offsets.iter().map(|o| ms(T0 + o)).collect()
}

fn servers(t: &Trace, c: u8) -> usize {
    t.server_news.iter().filter(|s| s.1 == c).count()
}

fn i25_restarts(t: &Trace, c: u8) -> usize {
    t.i25_restarts
        .iter()
        .filter(|r| r.1 == Side::Host && r.2 == c)
        .count()
}

/// 计数口径：受理个数 = `Server` 新建次数 = DERP 联系次数；helper 启动次数 ≤ 1 + I25 重启次数；
/// 被拒的请求（从请求时刻到下一个请求之前）不联系 DERP、不新建 `Server`、不启动 helper。
fn assert_counts(t: &Trace, c: u8, at: &[Duration], rs: &[Reply], what: &str) {
    let accepted = rs.iter().filter(|r| **r == Reply::Accepted).count();
    p2p_assert_eq!(
        servers(t, c),
        accepted,
        "{what}：Server 新建次数应等于受理个数"
    );
    p2p_assert_eq!(
        t.derp_contacts(Side::Host, c).len(),
        accepted,
        "{what}：DERP 联系次数应等于受理个数"
    );
    let spawns = t.spawns(Side::Host, c).len();
    p2p_assert!(
        spawns <= 1 + i25_restarts(t, c),
        "{what}：helper 启动 {spawns} 次，超过 1 + I25 重启 {} 次（每会话一个 helper）",
        i25_restarts(t, c)
    );
    for (i, r) in rs.iter().enumerate() {
        if !matches!(r, Reply::Declined(_)) {
            continue;
        }
        let (from, until) = (at[i], at.get(i + 1).copied().unwrap_or(Duration::MAX));
        let within = |x: &Duration| *x >= from && *x < until;
        p2p_assert!(
            !t.derp_contacts(Side::Host, c).iter().any(within)
                && !t.spawns(Side::Host, c).iter().any(within)
                && !t.server_news.iter().any(|(x, cc)| *cc == c && within(x)),
            "{what}：第 {} 个请求被拒后仍联系了 DERP / 启动了 helper / 新建了 Server",
            i + 1
        );
    }
}

use DeclineReason::{NotGranted, RetryInterval, RetryLimit};
use Reply::{Accepted, Declined};

/// 清单 #146（OQ-27）：被控端对同一会话内超过上限的再升级请求回 `DirectDecline { reason = retry_limit }`。间隔取 45 s，
/// 只测上限、不触及 30 s 间隔：前 4 个（初次 + 3 次再升级）受理，`Server` 新建 4 次、DERP 联系 4 次、helper 启动 1 次；
/// 第 5、6 个 `retry_limit`，各计数不再增加；会话保持中转、无 `Bye`、无错误提示；会话 B 的第 1 个受理；常量为 2 时第 4 个被拒。
#[test]
fn host_declines_direct_request_over_session_cap() {
    let offsets = [0, 45_000, 90_000, 135_000, 180_000, 225_000];
    let t = run(&scripted_session(direct_host(), &offsets)
        .at(T0 + 230_000, connect(C1))
        .at(T0 + 231_000, approve(C1))
        .at(T0 + 235_000, E::ScriptedDirectRequest { controller: C1 }));
    let at = abs(&offsets);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(rs[0], Accepted, "第 1 个（初次）请求应被受理（正向对照）");
    p2p_assert_eq!(&rs[1..4], &[Accepted; 3], "前 3 次再升级应被受理");
    p2p_assert_eq!(
        &rs[4..],
        &[Declined(RetryLimit); 2],
        "第 5 个（第 4 次再升级）与第 6 个请求应以 retry_limit 被拒"
    );
    assert_counts(&t, C0, &at, &rs, "会话 A");
    p2p_assert_eq!(
        t.spawns(Side::Host, C0).len(),
        1,
        "会话 A 的 helper 应只启动 1 次（没有 I25 重启）"
    );
    p2p_assert!(
        !t.session_ended(C0) && !sent_bye(&t, Side::Host, C0),
        "会话 A 不应结束"
    );
    p2p_assert_eq!(
        t.last_sender_path(Side::Host, C0),
        Some(Path::Relay),
        "会话 A 应保持中转"
    );
    p2p_assert!(t.error_prompts.is_empty(), "被拒不应弹错误提示");
    p2p_assert_eq!(
        replies(&t, C1, &[ms(T0 + 235_000)]),
        vec![Accepted],
        "会话 B 的第 1 个请求应被受理（计数按会话隔离）"
    );

    let host = HostConfig {
        retry_cap: 2,
        ..direct_host()
    };
    let offsets = [0, 45_000, 90_000, 135_000];
    let t = run(&scripted_session(host, &offsets));
    p2p_assert_eq!(
        replies(&t, C0, &abs(&offsets)),
        vec![Accepted, Accepted, Accepted, Declined(RetryLimit)],
        "上限为 2 时第 3 次再升级应被拒"
    );
}

/// 清单 #147（OQ-27）：距上一个被受理请求的到达时刻 < 30 s 的请求回 `retry_interval`，不论上一个 epoch 是否已结束；
/// 恰好 30.0 s 受理；被拒的请求不计数、不重置计时起点、无副作用。变体 A 间隔；B 起算点（被控端自己放弃之后）；
/// C 上一个 epoch 已提交并在用，t0 + 31 s 受理、不回 `retry_*`；D 被拒之后仍可受理到第 3 次再升级。
#[test]
fn host_declines_direct_request_within_retry_interval() {
    // 变体 A 与 D
    let offsets = [0, 5_000, 29_900, 30_000, 60_000, 90_000];
    let t = run(&scripted_session(direct_host(), &offsets));
    let at = abs(&offsets);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        rs[3],
        Accepted,
        "A：t0 + 30.0 s 应被受理（计时起点是 t0，正向对照）"
    );
    p2p_assert_eq!(rs[0], Accepted, "A：t0 的初次请求应被受理（正向对照）");
    p2p_assert_eq!(
        &rs[1..3],
        &[Declined(RetryInterval); 2],
        "A：t0 + 5 s 与 t0 + 29.9 s 应以 retry_interval 被拒"
    );
    p2p_assert_eq!(&rs[4..], &[Accepted; 2], "D：被拒的请求不应占用再升级次数");
    assert_counts(&t, C0, &at, &rs, "A");

    // 变体 B：被控端在 t0 + 3 s 自己放弃（H 关闸）；计时起点仍是 t0
    let offsets = [0, 10_000, 30_000];
    let t = run(&scripted_session(direct_host(), &offsets)
        .at(
            T0 + 2_000,
            E::GateOpen {
                side: Side::Host,
                controller: C0,
            },
        )
        .at(
            T0 + 3_000,
            E::GateClose {
                side: Side::Host,
                controller: C0,
                cause: GateCause::Handshake,
            },
        ));
    let rs = replies(&t, C0, &abs(&offsets));
    p2p_assert_eq!(
        rs[2],
        Accepted,
        "B：t0 + 30.0 s 应被受理（从放弃时刻起算的实现会拒）"
    );
    p2p_assert_eq!(
        rs[1],
        Declined(RetryInterval),
        "B：t0 + 10 s（距 t0 不足 30 s）应被拒"
    );

    // 变体 C：初次升级已提交、直连在用，t0 + 31 s 的新请求照常受理
    let s = gates(
        scripted_session(direct_host(), &[0, 31_000]),
        T0 + 1_000,
        C0,
    );
    let t = run(&s);
    let rs = replies(&t, C0, &abs(&[0, 31_000]));
    p2p_assert!(
        t.upgrades
            .iter()
            .any(|u| u.controller == C0 && u.completed < ms(T0 + 31_000)),
        "C：初次升级没有在 t0 + 31 s 之前提交（夹具前置）"
    );
    p2p_assert_eq!(
        rs,
        vec![Accepted, Accepted],
        "C：上一个 epoch 在用不应成为拒绝理由"
    );
    p2p_assert!(
        !t.declines(C0)
            .iter()
            .any(|d| matches!(d.1, RetryLimit | RetryInterval)),
        "C：不应回 retry_*"
    );
    p2p_assert_eq!(servers(&t, C0), 2, "C：t0 + 31 s 受理后应再新建一个 Server");
    let second = t
        .server_news
        .iter()
        .filter(|s| s.1 == C0)
        .map(|s| s.0)
        .max()
        .unwrap_or_default();
    p2p_assert!(
        t.server_closes
            .iter()
            .any(|(x, c)| *c == C0 && *x <= second),
        "C：新 epoch 之前旧 epoch 的 Server 没有 Close"
    );
    p2p_assert!(
        at_most_one_gate_open(&t, C0),
        "C：两个 epoch 的闸门同时打开"
    );
}

/// 任一时刻至多一个 epoch 的闸门打开（按控制端、按侧）
fn at_most_one_gate_open(t: &Trace, c: u8) -> bool {
    [Side::Host, Side::Controller].into_iter().all(|side| {
        let mut ev: Vec<(Duration, u32, bool)> = t
            .gates
            .iter()
            .filter(|g| g.controller == c && g.side == side)
            .map(|g| (g.at, g.epoch, g.open))
            .collect();
        ev.sort_by_key(|g| g.0);
        let mut open = std::collections::BTreeSet::new();
        ev.into_iter().all(|(_, e, o)| {
            if o {
                open.insert(e);
            } else {
                open.remove(&e);
            }
            open.len() <= 1
        })
    })
}

/// 守规则控制端（#144 夹具）与被控端配对：入口 ①（初次提交后每次再升级都失败）、入口 ②（每个 epoch 都在闸门打开后被 H 放弃）
fn paired(plan: UpgradePlan) -> Scenario {
    retry_period(plan, 260)
}

fn controller_requests(t: &Trace, c: u8) -> Vec<Duration> {
    t.requests
        .iter()
        .filter(|r| r.1 == c)
        .map(|r| r.0)
        .collect()
}

/// 清单 #148（OQ-27）：边界、一致性与状态归属。(i) 30.0 s 间隔下第 3 次再升级受理、第 4 次拒；第 4 个之后 5 s 到达的
/// 请求回 `retry_limit` 而不是 `retry_interval`（(b) 先于 (c)）；(ii) 守规则的控制端（入口 ① 1 + 3 个、入口 ② 3 个请求）
/// 全部受理、被控端 `retry_*` 拒绝数为 0；(iii) 不守规则的控制端被 (b)(c) 拦住；(iv) 既有检查先于上限；(v) 收回直连后再授予
/// 不清零；(vi) I25 重启 helper 不清零；(vii)(viii) 两个抖动变体只记录、不断言。
#[test]
fn host_third_retry_allowed_fourth_retry_declined_and_matches_controller_cap() {
    const NAME: &str = "host_third_retry_allowed_fourth_retry_declined_and_matches_controller_cap";
    // (vii)(viii) 只记录，先跑，避免被后面的断言提前结束
    let t = run(&paired(UpgradePlan::AbandonAfterGateOpen)
        .at(0, E::RelayLatency(ms(300)))
        .at(4_000, E::RelayLatency(ms(0))));
    let req = controller_requests(&t, C0);
    support::record(
        NAME,
        format!(
            "(vii) 入口 ② H 放弃，第一个请求中转时延 300 ms、重试 0 ms：请求 {} 个，应答 {:?}（预期第二个受理）",
            req.len(),
            replies(&t, C0, &req)
        ),
    );
    let mut fast = direct_controller(1);
    fast.plan = UpgradePlan::LocalFailFast;
    let t = run(&Scenario::new(direct_host())
        .controller(fast)
        .at(0, connect(C0))
        .at(0, E::RelayLatency(ms(50)))
        .at(1000, approve(C0))
        .at(
            2000,
            E::GateOpen {
                side: Side::Controller,
                controller: C0,
            },
        )
        .at(4_000, E::RelayLatency(ms(0)))
        .run_for(120));
    let req = controller_requests(&t, C0);
    support::record(
        NAME,
        format!(
            "(viii) 控制端本地 5 ms 失败，第一个请求中转时延 50 ms、重试 0 ms：请求 {} 个，应答 {:?}，控制端重试计数 {:?}（预期第二个 retry_interval、计一次失败）",
            req.len(),
            replies(&t, C0, &req),
            t.retry_counts.iter().map(|r| r.2).collect::<Vec<_>>()
        ),
    );

    // (i)
    let offsets = [0, 30_000, 60_000, 90_000, 95_000, 120_000];
    let t = run(&scripted_session(direct_host(), &offsets));
    let at = abs(&offsets);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        &rs[..4],
        &[Accepted; 4],
        "(i) 初次 + 3 次再升级（间隔恰为 30.0 s）应被受理"
    );
    p2p_assert_eq!(
        rs[4],
        Declined(RetryLimit),
        "(i) 第 4 个之后 5 s 到达的请求应回 retry_limit（(b) 先于 (c)），不是 retry_interval"
    );
    p2p_assert_eq!(
        rs[5],
        Declined(RetryLimit),
        "(i) t0 + 120 s 应回 retry_limit"
    );
    assert_counts(&t, C0, &at, &rs, "(i)");

    // (ii)
    for (plan, want) in [
        (UpgradePlan::CommitThenFailRetries, 4),
        (UpgradePlan::AbandonAfterGateOpen, 3),
    ] {
        let t = run(&paired(plan));
        let req = controller_requests(&t, C0);
        p2p_assert_eq!(
            req.len(),
            want,
            "(ii) {plan:?}：守规则的控制端应发 {want} 个请求、不多发"
        );
        let rs = replies(&t, C0, &req);
        p2p_assert_eq!(rs, vec![Accepted; want], "(ii) {plan:?}：被控端应全部受理");
        p2p_assert!(
            !t.declines(C0)
                .iter()
                .any(|d| matches!(d.1, RetryLimit | RetryInterval)),
            "(ii) {plan:?}：被控端拒绝数应为 0"
        );
        assert_counts(&t, C0, &req, &rs, "(ii)");
    }

    // (iii)
    let rogue = [0, 1_000, 31_000, 61_000, 91_000, 121_000];
    let t = run(&scripted_session(direct_host(), &rogue));
    let at = abs(&rogue);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        rs,
        vec![
            Accepted,
            Declined(RetryInterval),
            Accepted,
            Accepted,
            Accepted,
            Declined(RetryLimit)
        ],
        "(iii) 不守规则的控制端"
    );
    assert_counts(&t, C0, &at, &rs, "(iii)");
    p2p_assert!(
        t.error_prompts.is_empty() && !t.session_ended(C0),
        "(iii) 被拒不应弹错误或结束会话"
    );

    // (iv)
    let t = run(&Scenario::new(direct_host())
        .controller(scripted_controller())
        .at(0, connect(C0))
        .at(
            1000,
            E::Approve {
                controller: C0,
                direct: false,
            },
        )
        .at(T0, E::ScriptedDirectRequest { controller: C0 })
        .at(T0 + 1_000, E::GrantDirect { controller: C0 })
        .at(T0 + 2_000, E::ScriptedDirectRequest { controller: C0 })
        .at(T0 + 32_000, E::ScriptedDirectRequest { controller: C0 })
        .at(T0 + 62_000, E::ScriptedDirectRequest { controller: C0 })
        .at(T0 + 92_000, E::ScriptedDirectRequest { controller: C0 })
        .run_for(130));
    let at = abs(&[0, 2_000, 32_000, 62_000, 92_000]);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        rs,
        vec![Declined(NotGranted), Accepted, Accepted, Accepted, Accepted],
        "(iv) 无同意的请求回既有拒绝、不占次数；取得同意后 4 个仍都可受理"
    );
    assert_counts(&t, C0, &at, &rs, "(iv)");

    // (v) 收回直连后再授予：计数不清零
    let t = run(&scripted_session(
        direct_host(),
        &[0, 30_000, 50_000, 90_000, 120_000, 150_000],
    )
    .at(T0 + 40_000, E::RevokeDirect { controller: C0 })
    .at(T0 + 70_000, E::GrantDirect { controller: C0 }));
    let at = abs(&[0, 30_000, 50_000, 90_000, 120_000, 150_000]);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        rs,
        vec![
            Accepted,
            Accepted,
            Declined(NotGranted),
            Accepted,
            Accepted,
            Declined(RetryLimit)
        ],
        "(v) 收回期间回既有拒绝且不计数；再授予后计数延续，t0 + 150 s 应回 retry_limit"
    );
    assert_counts(&t, C0, &at, &rs, "(v)");

    // (vi) I25 结束并重启 helper：计数不清零
    let t = run(
        &scripted_session(direct_host(), &[0, 30_000, 60_000, 90_000, 120_000]).at(
            T0 + 40_000,
            E::HelperWriteStall {
                side: Side::Host,
                controller: C0,
            },
        ),
    );
    let at = abs(&[0, 30_000, 60_000, 90_000, 120_000]);
    let rs = replies(&t, C0, &at);
    p2p_assert_eq!(
        rs,
        vec![Accepted, Accepted, Accepted, Accepted, Declined(RetryLimit)],
        "(vi) helper 重启没有把计数清零：t0 + 120 s 应回 retry_limit"
    );
    p2p_assert_eq!(i25_restarts(&t, C0), 1, "(vi) I25 重启没有发生（夹具前置）");
    assert_counts(&t, C0, &at, &rs, "(vi)");
}
