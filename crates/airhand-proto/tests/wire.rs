//! tailcat-p2p 规格「先红验收测试清单」中的协议用例（`docs/specs/tailcat-p2p.md`）。
//!
//! 直连控制消息还不存在，所以这里不引用新变体，而是按规格 §3 给出的线格式手写 postcard 字节，
//! 断言当前的 `HostMsg` / `ControllerMsg` 能解出对应变体、并且重新编码后字节不变。
//!
//! 变体编号按 §3 的列出顺序追加在枚举末尾：
//! `HostMsg` 现有 0–8（`Hello` … `Bye`），之后依次是 `DirectOffer` 9、`DirectDecline` 10、`DirectCommit` 11、
//! `DirectSwitch` 12、`DirectFallback` 13、`DirectGateOpen` 14、`PermissionsChanged` 15；
//! `ControllerMsg` 现有 0–8（`Hello` … `Audio`），之后依次是 `DirectRequest` 9、`DirectDecline` 10、
//! `DirectSwitch` 11、`DirectFallback` 12、`Bye` 13。
//! 实现定稿时若调整顺序或字段，须在同一提交里改这里的字节，并把 `PROTOCOL_VERSION` 维持在 10。

#[macro_use]
#[path = "../../airhand-link/tests/support/mod.rs"]
mod support;

use airhand_proto::PROTOCOL_VERSION;
use airhand_proto::framing::{decode, encode};
use airhand_proto::session::{ControllerMsg, HostMsg};

const EPOCH: u8 = 7;
const CLIENT_KEY: &str = "nodekey:example";

fn host(bytes: &[u8]) -> HostMsg {
    let msg = p2p_ok!(
        decode::<HostMsg>(bytes),
        "HostMsg 解不出 {bytes:?}：协议里还没有对应的直连变体"
    );
    p2p_assert_eq!(
        p2p_ok!(encode(&msg), "重新编码失败"),
        bytes,
        "重新编码后字节变化"
    );
    msg
}

fn controller(bytes: &[u8]) -> ControllerMsg {
    let msg = p2p_ok!(
        decode::<ControllerMsg>(bytes),
        "ControllerMsg 解不出 {bytes:?}：协议里还没有对应的直连变体"
    );
    p2p_assert_eq!(
        p2p_ok!(encode(&msg), "重新编码失败"),
        bytes,
        "重新编码后字节变化"
    );
    msg
}

fn direct_offer_bytes() -> Vec<u8> {
    let mut b = vec![9, EPOCH, 1, 3, b'a', b'b', b'c'];
    b.extend_from_slice(&[0xab; 32]);
    b.extend_from_slice(&[0xcd; 16]);
    b.push(30);
    b
}

fn direct_request_bytes() -> Vec<u8> {
    let mut b = vec![9, EPOCH, CLIENT_KEY.len() as u8];
    b.extend_from_slice(CLIENT_KEY.as_bytes());
    b
}

/// 清单 #27（I4、F4、I24）：含 `DirectGateOpen` 在内的直连控制消息线格式稳定。
#[test]
fn direct_messages_wire_is_stable() {
    let offer = format!("{:?}", host(&direct_offer_bytes()));
    p2p_assert!(
        offer.starts_with("DirectOffer { epoch: 7, kind: 1, body: [97, 98, 99]"),
        "{offer}"
    );
    p2p_assert!(offer.ends_with("ttl_secs: 30 }"), "{offer}");

    for (bytes, expected) in [
        ([11, EPOCH], "DirectCommit { epoch: 7 }"),
        ([12, EPOCH], "DirectSwitch { epoch: 7 }"),
        ([13, EPOCH], "DirectFallback { epoch: 7 }"),
        ([14, EPOCH], "DirectGateOpen { epoch: 7 }"),
    ] {
        p2p_assert_eq!(format!("{:?}", host(&bytes)), expected);
    }

    let request = format!("{:?}", controller(&direct_request_bytes()));
    p2p_assert_eq!(
        request,
        format!("DirectRequest {{ epoch: 7, client_key: {CLIENT_KEY:?} }}")
    );
    for (bytes, expected) in [
        ([11, EPOCH], "DirectSwitch { epoch: 7 }"),
        ([12, EPOCH], "DirectFallback { epoch: 7 }"),
    ] {
        p2p_assert_eq!(format!("{:?}", controller(&bytes)), expected);
    }
}

/// 清单 #28（I4）：协议版本升到 10（v9 与 v10 不互通）。
#[test]
fn protocol_version_is_10() {
    p2p_assert_eq!(PROTOCOL_VERSION, 10, "PROTOCOL_VERSION 应升到 10");
}
