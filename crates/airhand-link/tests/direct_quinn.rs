//! tailcat-p2p 规格「先红验收测试清单」中在真实 quinn 上核实的用例（前置核实类 PV-2 / PV-3，
//! `docs/specs/tailcat-p2p.md` §2 O / H）。
//!
//! 每个用例注明清单编号。夹具在 [`glue`]：两端 quinn 0.11 + 可注入的内存数据报通道；胶水层接上之前是空操作，
//! 用例因正向对照不成立而以 `ASSERT:` 失败。PV 用例由 PV PR 变绿并从预期红名单删除；
//! 若因库行为不符而红，冻结对应实现、回规格。

#[macro_use]
#[path = "support/mod.rs"]
mod support;
#[path = "direct_quinn/glue.rs"]
mod glue;

use std::time::Duration;

use glue::{ChannelConfig, HandshakePacket, Loss, Observed, ProbePriority, Side};

const PROBE_PERIOD: u64 = 250;
const ACK_STALL: u64 = 1000;
const RX_SILENCE: u64 = 1000;

fn ms(v: u64) -> Duration {
    Duration::from_millis(v)
}

/// `[from, until)` 内相邻事件的最大间隔（含 from 到第一个事件、最后一个事件到 until）
fn max_gap(times: &[Duration], from: Duration, until: Duration) -> Duration {
    let mut last = from;
    let mut gap = Duration::ZERO;
    for &t in times.iter().filter(|t| **t >= from && **t < until) {
        gap = gap.max(t - last);
        last = t;
    }
    gap.max(until.saturating_sub(last))
}

fn count_within(times: &[Duration], from: Duration, until: Duration) -> usize {
    times.iter().filter(|t| **t >= from && **t < until).count()
}

/// 握手完成且两端的进展信号在健康时段里前进（≥ 每 250 ms 一次的 90%）——各用例的正向对照
fn healthy(o: &Observed, from: Duration, until: Duration, what: &str) {
    p2p_assert!(
        o.handshake_completed.is_some(),
        "{what}：直连 QUIC 握手没有完成（夹具前置）"
    );
    let slots = ((until - from).as_millis() as u64 / PROBE_PERIOD) as usize;
    for side in [Side::Host, Side::Controller] {
        let n = count_within(&o.progress_of(side), from, until);
        p2p_assert!(
            n * 10 >= slots * 9,
            "{what}：{side:?} 的进展信号在健康时段里只前进了 {n} 次（{slots} 个探测周期）"
        );
    }
}

/// §2 模型：连续失败的探测个数——探测在突发窗口内发出，或其确认（偏移 delta）落入窗口
fn runs_missing(probes: &[Duration], at: Duration, len: Duration, delta: Duration) -> u64 {
    let lost =
        |p: Duration| (p >= at && p <= at + len) || (p + delta >= at && p + delta <= at + len);
    let (mut best, mut cur) = (0, 0);
    for &p in probes {
        cur = if lost(p) { cur + 1 } else { 0 };
        best = best.max(cur);
    }
    best
}

/// 清单 #121（I3、§2 O，PV-2）：只丢被控端→控制端方向、控制端持续发 ack-eliciting 包时，被控端 O 的进展信号
/// （探测流被新确认）在 `ack_stall_ms` 内停止前进；反方向同样。前置：对端 `max_concurrent_uni_streams` ≥ 32、
/// 探测流最高发送优先级、握手完成后立即发出第一条探测流。
#[test]
fn ack_progress_signal_stalls_on_one_way_loss_real_quinn() {
    let cut = ms(5_000);
    for lost in [Side::Host, Side::Controller] {
        let o = glue::run(
            &ChannelConfig::default(),
            &[Loss::one_way(lost, cut)],
            ms(10_000),
        );
        let what = format!("丢 {lost:?}→{:?}", lost.peer());
        healthy(&o, ms(0), cut, &what);
        p2p_assert!(
            o.probes_of(lost)
                .first()
                .is_some_and(|p| *p < ms(PROBE_PERIOD)),
            "{what}：握手完成后没有立即发出第一条探测流（首条 {:?}）",
            o.probes_of(lost).first()
        );
        p2p_assert!(
            o.peer_limit_of(lost) >= 32,
            "{what}：对端额度 {} < 32",
            o.peer_limit_of(lost)
        );
        p2p_assert!(
            o.probe_priority.is_some_and(|p| p == i32::MAX),
            "{what}：探测流优先级 {:?} 不是最高",
            o.probe_priority
        );
        p2p_assert!(
            count_within(&o.ack_eliciting_of(lost.peer()), cut, ms(10_000)) > 0,
            "{what}：断路后对端没有持续发 ack-eliciting 包（夹具前置）"
        );
        let after = o
            .progress_of(lost)
            .into_iter()
            .filter(|t| *t > cut)
            .collect::<Vec<_>>();
        let stalled_by = cut + ms(ACK_STALL);
        p2p_assert!(
            after.iter().all(|t| *t < stalled_by),
            "{what}：{lost:?} 的进展信号在断路 {ACK_STALL} ms 之后仍前进（{after:?}）"
        );
    }
}

/// 清单 #122（I3、§2 O，PV-2）：被控端→控制端方向全丢、控制端持续发 ack-eliciting 包时，被控端 O 的进展信号不前进；
/// 若观测到 ACK 帧计数在该场景下增长，则以 ACK 帧计数为进展的变异实现必须被判错；未观测到增长时只记录“未出现”。
#[test]
fn ack_progress_not_advanced_by_repeated_ack_ranges() {
    const NAME: &str = "ack_progress_not_advanced_by_repeated_ack_ranges";
    let cut = ms(5_000);
    let end = ms(15_000);
    let o = glue::run(
        &ChannelConfig::default(),
        &[Loss::one_way(Side::Host, cut)],
        end,
    );
    healthy(&o, ms(0), cut, "断路前");
    p2p_assert!(
        count_within(&o.ack_eliciting_of(Side::Controller), cut, end) > 0,
        "控制端在断路期间没有持续发 ack-eliciting 包（夹具前置）"
    );
    let settle = cut + ms(ACK_STALL);
    let real = count_within(&o.progress_of(Side::Host), settle, end);
    p2p_assert_eq!(real, 0, "被控端 O 的进展信号在出站全丢时仍前进");
    let frames = count_within(&o.ack_frames_of(Side::Host), settle, end);
    support::record(
        NAME,
        format!(
            "出站全丢期间被控端收到 ACK 帧 {frames} 个；携带旧 ACK 区间：{:?}",
            o.carried_old_ack_ranges
        ),
    );
    if frames > 0 {
        let mutant = max_gap(&o.ack_frames_of(Side::Host), settle, end) < ms(ACK_STALL);
        p2p_assert!(
            mutant,
            "ACK 帧计数在增长但间隔达到 ack_stall：变异实现（以 ACK 帧计数为进展）没有被区分出来"
        );
        support::record(NAME, "以 ACK 帧计数为进展的变异实现在此场景不会关闸，判错");
    } else {
        support::record(
            NAME,
            "未出现 ACK 帧计数增长，不作变异体断言（由 #49 的假源场景承担）",
        );
    }
}

/// 清单 #123（I3、§2 O，PV-2）：探测流被对端接受并读完丢弃；长时间运行不耗尽 `max_concurrent_uni_streams`；
/// ack 停滞时在途探测流 ≤ 16、不再新开；对端额度 ≥ 32；开不出来时本周期不发并计数，进展照常按确认判定；
/// 中转 `transport_config()` 的 `max_concurrent_uni_streams` 仍为 0。
#[test]
fn ack_probe_streams_are_accepted_and_do_not_exhaust_stream_limit() {
    let stall = ms(60_000);
    let recover = ms(70_000);
    let end = ms(90_000);
    let o = glue::run(
        &ChannelConfig::default(),
        &[Loss {
            dir: Some(glue::Dir {
                from: Side::Controller,
            }),
            from: stall,
            until: Some(recover),
        }],
        end,
    );
    healthy(&o, ms(0), stall, "60 s 健康运行");
    for side in [Side::Host, Side::Controller] {
        let sent = count_within(&o.probes_of(side), ms(0), stall);
        p2p_assert!(sent >= 200, "{side:?}：60 s 只开了 {sent} 条探测流");
        p2p_assert!(
            o.accepted_of(side.peer()) >= sent,
            "{side:?}：对端只接受并读完 {} 条（发出 {sent} 条）",
            o.accepted_of(side.peer())
        );
        p2p_assert!(
            o.peer_limit_of(side) >= 32,
            "{side:?}：对端额度 {} < 32",
            o.peer_limit_of(side)
        );
        p2p_assert!(
            o.max_in_flight_of(side) <= 16,
            "{side:?}：ack 停滞时在途探测流 {} 条，超过 16",
            o.max_in_flight_of(side)
        );
    }
    p2p_assert!(
        o.skipped_of(Side::Host) > 0,
        "被控端在 ack 停滞时没有停开探测流（跳过计数为 0）"
    );
    p2p_assert!(
        count_within(&o.progress_of(Side::Host), recover + ms(1_000), end) > 0,
        "恢复之后被控端的进展信号没有再前进"
    );
    p2p_assert_eq!(
        o.relay_uni_stream_limit,
        Some(0),
        "中转的 max_concurrent_uni_streams 应仍为 0"
    );
}

/// 清单 #126（I3、§2 H，PV-3）：握手期间单个包丢失（被控端首个飞行 / 控制端 Finished 各一次）在 RTT ≤ 350 ms 时
/// 不被 H 放弃；RTT 375 / 400 ms 只记录完成时间。
#[test]
fn host_handshake_not_abandoned_under_single_loss_up_to_rtt_350ms() {
    const NAME: &str = "host_handshake_not_abandoned_under_single_loss_up_to_rtt_350ms";
    let cfg = |rtt, drop| ChannelConfig {
        rtt: ms(rtt),
        handshake_drop: drop,
        ..ChannelConfig::default()
    };
    let drops = [
        HandshakePacket::HostFirstFlight,
        HandshakePacket::ControllerFinished,
    ];
    for rtt in [375, 400] {
        for d in drops {
            let o = glue::run(&cfg(rtt, Some(d)), &[], ms(3_000));
            support::record(
                NAME,
                format!(
                    "RTT {rtt} ms 丢 {d:?}：完成 {:?}，放弃 {}",
                    o.handshake_completed, o.handshake_abandoned
                ),
            );
        }
    }
    let o = glue::run(&cfg(350, None), &[], ms(3_000));
    p2p_assert!(
        o.handshake_completed.is_some(),
        "RTT 350 ms 无丢包时握手没有完成（正向对照）"
    );
    for rtt in [100, 200, 300, 350] {
        for d in drops {
            let o = glue::run(&cfg(rtt, Some(d)), &[], ms(3_000));
            p2p_assert!(
                o.handshake_completed.is_some() && !o.handshake_abandoned,
                "RTT {rtt} ms 丢 {d:?}：握手被 H 放弃或没有完成（完成 {:?}）",
                o.handshake_completed
            );
        }
    }
}

/// 清单 #140（I3、§2 O，PV-2）：RTT 400 ms、抖动 50 ms 下双向完全丢包突发后，O 的进展恢复时间不超过 §2 公式；
/// < 250 ms 的突发任意相位都不使进展间隔达到 `ack_stall_ms`；1000 ms 突发使间隔 ≥ 1000 ms（正向对照）；
/// 媒体占满变体（100 / 240 ms）：进展间隔 < `ack_stall_ms` 且对端入站间隔 < `rx_silence_ms`。
#[test]
fn ack_progress_recovery_after_burst_real_quinn() {
    const NAME: &str = "ack_progress_recovery_after_burst_real_quinn";
    let jitter = 50;
    let base = |up, media| ChannelConfig {
        rtt: ms(400),
        up: Some(ms(up)),
        jitter: ms(jitter),
        media_saturation: media,
        ..ChannelConfig::default()
    };
    let at0 = 5_000;
    let mut runs = Vec::new();
    for len in [100u64, 250, 300, 500, 700, 1000] {
        for phase in 0..10u64 {
            for up in (0..=400u64).step_by(50) {
                runs.push((len, phase, up, false));
            }
        }
    }
    for len in [100u64, 240] {
        for phase in 0..10u64 {
            for up in [200u64, 350, 50] {
                runs.push((len, phase, up, true));
            }
        }
    }
    for (len, phase, up, media) in runs {
        let at = ms(at0 + phase * 25);
        let end = at + ms(len + 5_000);
        let o = glue::run(&base(up, media), &[Loss::burst(at, ms(len))], end);
        let what = format!(
            "{len} ms 突发（相位 {phase}、d1 = {up} ms{}）",
            if media { "、媒体占满" } else { "" }
        );
        healthy(&o, ms(0), at, &what);
        for side in [Side::Host, Side::Controller] {
            let progress = o.progress_of(side);
            let gap = max_gap(&progress, at - ms(1_000), end);
            let recovered = progress
                .iter()
                .find(|t| **t > at + ms(len))
                .map(|t| *t - (at + ms(len)));
            let inbound_gap = max_gap(&o.inbound_of(side.peer()), at - ms(1_000), end);
            support::record(
                NAME,
                format!(
                    "{what} {side:?}：最大进展间隔 {gap:?}，突发结束→首次进展 {recovered:?}，对端最大入站间隔 {inbound_gap:?}"
                ),
            );
            // 确认在探测到达对端时发出：偏移 = 本端→对端的单程时延
            let delta = ms(if side == Side::Host { 400 - up } else { up });
            let r = runs_missing(&o.probes_of(side), at, ms(len), delta);
            let bound = ms((r + 1) * PROBE_PERIOD + 25 + jitter);
            p2p_assert!(
                gap <= bound,
                "{what} {side:?}：最大进展间隔 {gap:?} 超出 §2 公式（r = {r}，上界 {bound:?}）"
            );
            if len < 250 {
                p2p_assert!(
                    gap < ms(ACK_STALL),
                    "{what} {side:?}：最大进展间隔 {gap:?} 达到 ack_stall"
                );
            }
            if media {
                p2p_assert!(
                    inbound_gap < ms(RX_SILENCE),
                    "{what} {side:?}：对端最大入站间隔 {inbound_gap:?} 达到 rx_silence"
                );
            }
            if len == 1000 {
                p2p_assert!(
                    gap >= ms(ACK_STALL),
                    "{what} {side:?}：1000 ms 突发下最大进展间隔 {gap:?} < 1000 ms（正向对照）"
                );
            }
        }
    }
    let o = glue::run(
        &base(200, false),
        &[Loss::burst(ms(at0), ms(300))],
        ms(at0 + 5_300),
    );
    support::record(
        NAME,
        format!("携带旧 ACK 区间（观测）：{:?}", o.carried_old_ack_ranges),
    );
}

/// 清单 #145（I3、§2 O，PV-2）：探测流用最高发送优先级，媒体流把拥塞窗口占满时进展间隔仍 < `ack_stall_ms`；
/// 通道 20 Mbps、基础 RTT 250 ms、尾部丢弃 FIFO 排队上限 150 ms（含排队 RTT ≤ 400 ms，否则夹具无效）；
/// 对照实现（默认优先级）的最大间隔只记录。
#[test]
fn probe_streams_use_highest_send_priority_and_survive_media_saturation_real_quinn() {
    const NAME: &str =
        "probe_streams_use_highest_send_priority_and_survive_media_saturation_real_quinn";
    let cfg = |priority| ChannelConfig {
        rtt: ms(250),
        bandwidth_bps: Some(20_000_000),
        queue_limit: Some(ms(150)),
        media_saturation: true,
        probe_priority: priority,
        ..ChannelConfig::default()
    };
    let end = ms(60_000);
    let gaps = |o: &Observed| {
        let mut p = o.progress_of(Side::Host);
        p.sort();
        let mut v: Vec<Duration> = p.windows(2).map(|w| w[1] - w[0]).collect();
        v.sort();
        v
    };
    let control = glue::run(&cfg(ProbePriority::Default), &[], end);
    let cg = gaps(&control);
    support::record(
        NAME,
        format!(
            "对照（默认优先级）：最大进展间隔 {:?}，媒体 {} 字节{}",
            cg.last(),
            control.media_bytes,
            if cg.last().is_some_and(|g| *g < ms(ACK_STALL)) {
                "；本夹具下优先级无差别"
            } else {
                ""
            }
        ),
    );

    let o = glue::run(&cfg(ProbePriority::Highest), &[], end);
    p2p_assert!(
        o.handshake_completed.is_some(),
        "直连 QUIC 握手没有完成（夹具前置）"
    );
    p2p_assert!(
        o.max_queued_rtt.is_some_and(|r| r <= ms(400)),
        "夹具无效（不判产品失败）：含排队 RTT 最大值 {:?} 超过 400 ms 或未测得",
        o.max_queued_rtt
    );
    p2p_assert!(
        o.media_bytes > 0,
        "媒体流没有吞吐，拥塞窗口没有被占满（正向对照）"
    );
    let g = gaps(&o);
    p2p_assert!(
        g.len() >= 200,
        "60 s 里探测流进展只有 {} 次（正向对照）",
        g.len()
    );
    let p50 = g[g.len() / 2];
    let p99 = g[g.len() * 99 / 100];
    let max = *g.last().unwrap_or(&Duration::ZERO);
    support::record(
        NAME,
        format!(
            "最高优先级：进展间隔 p50 {p50:?}、p99 {p99:?}、最大 {max:?}，媒体 {} 字节",
            o.media_bytes
        ),
    );
    p2p_assert!(
        max < ms(ACK_STALL),
        "媒体占满时探测流最大进展间隔 {max:?} 达到 ack_stall"
    );
}
