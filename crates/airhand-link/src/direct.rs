//! Phase 1 viability primitives. These do not switch sessions or operate helper gates.

pub mod probe;

use quinn::{TransportConfig, VarInt};
use std::{sync::Arc, time::Duration};

/// Independent direct-connection configuration; relay settings remain in `net`.
pub fn direct_transport_config() -> TransportConfig {
    let mut t = TransportConfig::default();
    t.max_concurrent_bidi_streams(VarInt::from_u32(256));
    t.max_concurrent_uni_streams(VarInt::from_u32(32));
    t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
    t.keep_alive_interval(Some(Duration::from_millis(250)));
    t.stream_receive_window(VarInt::from_u32(16 * 1024 * 1024));
    t.receive_window(VarInt::from_u32(64 * 1024 * 1024));
    t.send_window(64 * 1024 * 1024);
    t.congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    // tailcat's UDP payload limit is 1232. Disable path-MTU growth above 1200.
    t.initial_mtu(1200).min_mtu(1200).mtu_discovery_config(None);
    t
}
