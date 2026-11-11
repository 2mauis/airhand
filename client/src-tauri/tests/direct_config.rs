//! tailcat-p2p 规格「先红验收测试清单」中 `[direct]` 配置的用例（`docs/specs/tailcat-p2p.md`）。
//! 每个用例注明清单编号（该表从上往下数，1–44）。

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
