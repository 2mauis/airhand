//! tailcat-p2p 规格「先红验收测试清单」中 `[direct]` 配置的用例（`docs/specs/tailcat-p2p.md`）。
//! 每个用例注明清单编号（该表从上往下数，1–44）。

use airhand_lib::direct_config::{DirectConfig, parse};

fn parsed(text: &str) -> DirectConfig {
    match parse(text) {
        Ok(c) => c,
        Err(e) => panic!("解析 {text:?} 失败：{e:#}"),
    }
}

/// 清单 #23（配置、I26）：`bitrate_cap_kbps` 默认为 `100000`；`enabled` 默认关闭。
#[test]
fn direct_config_bitrate_cap_default_is_100mbps() {
    let c = parsed("");
    assert_eq!(c.bitrate_cap_kbps, 100_000);
    assert!(!c.enabled);
    let c = parsed("[direct]\nenabled = true\n");
    assert_eq!(c.bitrate_cap_kbps, 100_000);
    assert!(c.enabled);
}

/// 清单 #24（配置、I26）：`bitrate_cap_kbps` > 100000 被拒绝，范围内的值照常使用。
#[test]
fn direct_config_rejects_bitrate_cap_over_100mbps() {
    assert_eq!(
        parsed("[direct]\nbitrate_cap_kbps = 100000\n").bitrate_cap_kbps,
        100_000
    );
    assert_eq!(
        parsed("[direct]\nbitrate_cap_kbps = 30000\n").bitrate_cap_kbps,
        30_000
    );
    let err = parse("[direct]\nbitrate_cap_kbps = 100001\n");
    assert!(err.is_err(), "bitrate_cap_kbps = 100001 被接受：{err:?}");
    let msg = format!("{:#}", err.unwrap_err());
    assert!(
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
        assert!(!c.enabled, "{text:?} 下直连被打开");
        assert_eq!(c.upgrade_timeout_secs, 15);
        assert_eq!(c.rx_silence_ms, 1000);
        assert_eq!(c.write_timeout_ms, 500);
        assert_eq!(c.max_sessions, 2);
    }
}
