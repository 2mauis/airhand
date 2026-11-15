<p align="center"><img src="assets/icon.svg" width="96" alt="AirHand"></p>

# AirHand · Tailcat direct MVP

[English](README.md) · [简体中文](README.zh-CN.md)

This fork of [sqlpxc/airhand](https://github.com/sqlpxc/airhand) implements a single-pair macOS desktop path managed by **Tailcat**, this repository's direct transport. Tailcat is not Tailscale. This path needs no independent AirHand relay or signaling service; it retains Tailcat-native DERP/STUN for discovery.

The authority is the [approved direct design](docs/specs/tailcat-direct-mvp.md), imported without changing its bytes. The [direct feature map](docs/specs/tailcat-direct-feature-map.md) records implemented, unimplemented and pending-Mac-validation items. The [handoff archive](docs/tailcat-handoff.md) preserves the prior evidence and its limits.

**Status: source integration and local transport tests; two-Mac App acceptance pending.** The helper-binary test carries synthetic UDP datagrams, not a desktop. The prior Tailcat library tests and PR #17's relay desktop tests do not demonstrate that this App's screen or keyboard/mouse path has passed.

| Area | Current state |
|---|---|
| Tailcat helper | Implemented: fixed control argv, stdin secrets, real Server/Client, one-port loopback UDP bridge, direct gate, redacted events and DERP data counters; local binary test passes |
| Direct QUIC | Implemented: independent ALPN, pair-pinned mutual TLS, one session / three lanes, binding/timeout/extra-stream rejection; local tests pass |
| Pairing and consent | Implemented in source: AirDrop R/I files, H(R), expiry, one-time consumption, host-confirmed SAS, native Keychain, per-session default-deny consent, optional input and revocation; native macOS validation pending |
| Screen and keyboard/mouse | Existing real capture/encoding/rendering/input pipelines wired after authentication, consent and direct gating; actual two-Mac behavior remains unverified |
| Failure handling and packaging | Input release/discard, controller stop, session closure, separate macOS identifier, signed sidecar build path, local-network description; Mac timing/TCC/signing validation pending |
| Deliberately unimplemented | Windows/Android/iOS direct flow and P1 items, including socketpair, QR/Bonjour, authorization memory, gate recovery within a session and new CI before Mac acceptance |

Only **UDP direct** opens the application gate in this trial. Path loss closes the session and releases input; a recovered gate does not resume it. The detection window can still contain DERP packets and must be measured. Long-term DERP application-data fallback remains a product decision. Remembering a pair is an experimental, switchable preference; persistent host key/PSK and standing DERP presence are not settled production policy.

This round adds real control-bridge regression tests for the single UDP service port, silent TCP SYN drops, the node allowlist and control output. Desktop tasks use the Tauri runtime and local revocation takes priority over incoming input. Mandatory controller SAS confirmation remains S2-4 for a later round. These checks do not certify screen or keyboard/mouse behavior on actual Macs.

## Build and try on two Macs

Use the [fixed-signature macOS build](docs/building.md), then the [App pairing flow](docs/usage.md): controller exports R → AirDrop → host explicitly allows pairing → AirDrop I → controller imports once → both show SAS → host alone must confirm → host authorizes each session. No long address/key is entered or copied to the clipboard. The host's **Allow connections** switch starts off; its menu bar provides a persistent disconnect action.

Credentials persist in Keychain; private helper configuration goes only through stdin. Explicit invitation files contain a secret and need deletion after transfer/import. `Secret<T>` redacts Rust Debug; neither telemetry nor the normal UI exposes addresses, keys or peer IPs. Clipboard synchronization is disabled for direct sessions.

Run the local checks documented in [Development](docs/development.md). The [validation record](docs/tailcat-direct-validation.md) lists evidence and the pending same-LAN/hotspot, negative, failure and secret checks. Native Apple bundle building, AirDrop/Keychain/TCC and ten-minute screen/input acceptance require actual Macs. Do not alter firewall or macOS security settings to manufacture a pass.

## Documentation

| Guide | Fork-specific content |
|---|---|
| [Architecture](docs/architecture.md) | Helper bridge, mutual TLS, direct session/consent boundary and reused media pipeline |
| [Deployment](docs/deploy.md) | No AirHand service deployment for this direct path |
| [Usage](docs/usage.md) | App pairing, session consent, disconnect, forgetting and persistence switch |
| [Security](docs/security.md) | Two-layer identity, secrets, loopback protection and fail-closed limits |
| [Limitations](docs/limitations.md) | Pending native acceptance, detection window and excluded scope |
| [Building](docs/building.md) | Separate bundle ID, existing fixed signing identity and sidecar |
| [Development](docs/development.md) | Local tests, historical first-red fixtures and Mac acceptance |
| [Android](docs/android.md) | Out of direct-MVP scope; inherited phone/TV behavior retained |

The inherited relay implementation remains in the repository. Its guides explicitly label retained sections **“Upstream relay-mode behavior, not this fork's target design.”** Its multi-platform feature/performance claims and deployment recipes are not direct-MVP guarantees. Old Tailcat relay-upgrade designs and review records remain at their original paths with archive notices; use the new direct design/map instead.

When functionality changes, update **both READMEs and the direct feature map together**, keeping source implementation separate from real-device acceptance. `server/` and CI policy are unchanged by this work. New dependencies, policy changes, credentials, merge or deployment require separate approval.

## License

MIT; retain the upstream copyright and license in [LICENSE](LICENSE).
