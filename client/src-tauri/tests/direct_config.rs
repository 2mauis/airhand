//! tailcat-p2p 规格「先红验收测试清单」中 `[direct]` 配置的用例（`docs/specs/tailcat-p2p.md`）。
//! 每个用例注明清单编号（该表从上往下数）。

#[macro_use]
#[path = "../../../crates/airhand-link/tests/support/mod.rs"]
mod support;

use airhand_lib::direct_config::{DirectConfig, parse};

fn parsed(text: &str) -> DirectConfig {
    p2p_ok!(parse(text), "解析 {text:?} 失败")
}

/// 清单 #23（配置、I26）：`bitrate_cap_kbps` 默认为 `100000`；`enabled` 默认关闭。
#[test]
fn direct_config_bitrate_cap_default_is_100mbps() {
    let c = parsed("");
    p2p_assert_eq!(c.bitrate_cap_kbps, 100_000, "bitrate_cap_kbps 默认值");
    p2p_assert!(!c.enabled, "enabled 默认应关闭");
    let c = parsed("[direct]\nenabled = true\n");
    p2p_assert_eq!(
        c.bitrate_cap_kbps,
        100_000,
        "只开 enabled 时 bitrate_cap_kbps 取默认值"
    );
    p2p_assert!(c.enabled, "enabled = true 没有生效");
}

/// 清单 #24（配置、I26）：`bitrate_cap_kbps` > 100000 被拒绝，范围内的值照常使用。
#[test]
fn direct_config_rejects_bitrate_cap_over_100mbps() {
    p2p_assert_eq!(
        parsed("[direct]\nbitrate_cap_kbps = 100000\n").bitrate_cap_kbps,
        100_000,
        "上限值本身应被接受"
    );
    p2p_assert_eq!(
        parsed("[direct]\nbitrate_cap_kbps = 30000\n").bitrate_cap_kbps,
        30_000,
        "范围内的值应照常使用"
    );
    let err = parse("[direct]\nbitrate_cap_kbps = 100001\n");
    p2p_assert!(err.is_err(), "bitrate_cap_kbps = 100001 被接受：{err:?}");
    let msg = err.err().map(|e| format!("{e:#}")).unwrap_or_default();
    p2p_assert!(
        msg.contains("bitrate_cap_kbps"),
        "错误信息没有指出键名：{msg}"
    );
}

/// 清单 #25（F1、I1）：`[direct] enabled` 默认 false（没有 `[direct]` 段、空段、只有其它设置时都是关闭）。
#[test]
fn direct_config_defaults_off() {
    for text in [
        "",
        "[direct]\n",
        "server = \"relay.example.com:4433\"\nautostart = true\n",
    ] {
        let c = parsed(text);
        p2p_assert!(!c.enabled, "{text:?} 下直连被打开");
        p2p_assert_eq!(
            c.upgrade_timeout_secs,
            15,
            "{text:?}：upgrade_timeout_secs 默认值"
        );
        p2p_assert_eq!(c.rx_silence_ms, 1000, "{text:?}：rx_silence_ms 默认值");
        p2p_assert_eq!(c.write_timeout_ms, 500, "{text:?}：write_timeout_ms 默认值");
        p2p_assert_eq!(c.max_sessions, 2, "{text:?}：max_sessions 默认值");
    }
}

/// 解析失败且错误信息指出键名
fn rejected(text: &str, key: &str) {
    let err = parse(text);
    p2p_assert!(err.is_err(), "{text:?} 被接受：{err:?}");
    let msg = err.err().map(|e| format!("{e:#}")).unwrap_or_default();
    p2p_assert!(
        msg.contains(key),
        "{text:?} 的错误信息没有指出键名 {key}：{msg}"
    );
}

/// 清单 #73（I1、F1）：`derp_map_url` 非 https 被拒；https 的值照常使用。
#[test]
fn direct_config_rejects_non_https_derpmap() {
    let url = "https://derp.example.com/derpmap.json";
    p2p_assert_eq!(
        parsed(&format!("[direct]\nderp_map_url = \"{url}\"\n")).derp_map_url,
        url,
        "https 的 derp_map_url 应照常使用"
    );
    for bad in [
        "http://derp.example.com/derpmap.json",
        "file:///etc/derpmap.json",
        "derp.example.com/derpmap.json",
    ] {
        rejected(
            &format!("[direct]\nderp_map_url = \"{bad}\"\n"),
            "derp_map_url",
        );
    }
}

/// 清单 #74（I1、F1）：`[direct]` 没有 helper 路径选项，出现即被拒（helper 只从安装目录取）。
#[test]
fn direct_config_has_no_helper_path_option() {
    p2p_assert!(
        parsed("[direct]\nenabled = true\n").enabled,
        "合法的 [direct] 段没有生效（正向对照）"
    );
    for key in ["helper_path", "helper", "tailcat_path"] {
        rejected(
            &format!("[direct]\nenabled = true\n{key} = \"/opt/example/helper\"\n"),
            key,
        );
    }
}

/// 范围检查的共同形状：上下界本身被接受、取值照用，越界一侧各一个值被拒
fn range_checked(key: &str, lo: u32, hi: u32, get: fn(&DirectConfig) -> u32) {
    for v in [lo, hi] {
        p2p_assert_eq!(
            get(&parsed(&format!("[direct]\n{key} = {v}\n"))),
            v,
            "{key} = {v}（范围端点）应被接受并照用"
        );
    }
    for v in [lo - 1, hi + 1] {
        rejected(&format!("[direct]\n{key} = {v}\n"), key);
    }
}

/// 清单 #133（I3、§4 配置）：`[direct] rx_silence_ms` 低于 1000 或高于 1200 被拒绝。
#[test]
fn direct_config_rejects_rx_silence_out_of_range() {
    range_checked("rx_silence_ms", 1000, 1200, |c| c.rx_silence_ms);
}

/// 清单 #134（I3、§4 配置）：`[direct] ack_stall_ms` 低于 1000 或高于 1100 被拒绝。
#[test]
fn direct_config_rejects_ack_stall_out_of_range() {
    range_checked("ack_stall_ms", 1000, 1100, |c| c.ack_stall_ms);
}

/// 清单 #135（I3、§4 配置）：`[direct] handshake_watchdog_ms`（H）低于 1000 或高于 1500 被拒绝。
#[test]
fn direct_config_rejects_handshake_watchdog_out_of_range() {
    range_checked("handshake_watchdog_ms", 1000, 1500, |c| {
        c.handshake_watchdog_ms
    });
}
