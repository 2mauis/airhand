# Security

[← Back to README](../README.md) · [简体中文](security.zh-CN.md)

## This fork: direct trust boundary

See the [approved design](specs/tailcat-direct-mvp.md) and [status map](specs/tailcat-direct-feature-map.md). A nonempty WireGuard allowlist, pinned host address and pair-pinned mutual TLS are all required; LAN membership is not identity. The local QUIC socket accepts the helper's fixed tuple and authenticates the peer certificate, protecting against a process reaching loopback. No insecure direct TLS mode exists.

R commits to the controller nonce; I binds H(R), the host nonce and host certificate pin. Requests expire after at most ten minutes; I is consumed once, with replay tombstones retained until expiry. The authenticated first connection reveals the nonce for a six-digit SAS shown by both Apps. Currently only the host must confirm the match; mandatory controller confirmation remains S2-4 for a later round. Host SAS confirmation and per-session consent are distinct, default-deny steps; input permission is off initially. A busy host rejects additional requests.

Direct loss, lack of new-data ACK progress, revocation, helper failure and session cancellation close the connection; the host releases held keys and discards later input. Counters may still increase during the detection window; zero DERP leakage is not claimed. Long-term DERP fallback remains undecided.

Durable secrets use native Keychain APIs. The App passes node/TLS seeds, PSK and address to the helper only over private stdin, never argv/environment. `Secret<T>` redacts Debug. Helper stderr is discarded, and telemetry contains only closed categories/counters. A separate private setup reply returns the derived public certificate/identity and, for the host, the invitation address directly to the parent; it is never emitted as telemetry or rendered/logged. Explicit invitation files are the transfer exception: they contain a secret and need deletion after AirDrop/import. The helper uses Tailcat's in-memory store and does not access Keychain or write keys to disk.

Keychain ACLs, signed helper identity, local-process injection, crash/log/clipboard grep, deny/timeout behavior and release timing still require macOS acceptance. Existing library or relay tests do not certify these properties end to end. No new dependency or security policy allowlist is introduced by this implementation.

## Inherited relay security model

> Upstream relay-mode behavior, not this fork's target design.

- TLS 1.3 throughout (QUIC); the client verifies the server by **certificate fingerprint pinning**
- Brute-force protection: at most 20 failed attempts per IP (IPv6 grouped by /64) per 10 minutes, and each failure adds a 400 ms delay
- Resume tokens: when a controller connects, the server gives it a token derived from the host's code and secret (it can't be computed or reversed without the secret), and the controller presents it when reconnecting. Requests with a token don't count as failures and aren't rate limited, so however long a host stays offline, automatic reconnects can't lock the controller (or a web console sharing one IP) out. With a wrong token the server always answers "offline", revealing nothing about whether the code exists, so tokens can't be used to guess codes. A token stops working when the host changes its code
- Getting a control code back after a reconnect requires a random secret, so nobody else can claim it
- Fixed control codes: the server only stores a hash of the host's secret (in `airhand.db`), so the database can't be used to claim codes. On Windows the host's secret is encrypted with DPAPI for the SYSTEM account, so a copied `config.toml` doesn't carry the code to another computer; on macOS / Linux `config.toml` is readable by your account only. **Change control code** releases the old code on the server
- My devices holds other computers' control codes: on Windows they're encrypted with your account (DPAPI), on macOS / Linux protected by the file permissions (600). Don't share `config.toml`
- The Windows host runs as SYSTEM so that it can operate UAC prompts and the sign-in screen: whoever has the control code controls the computer with administrator rights. Give the code only to people you trust; whoever sits at the computer sees who is connected and can disconnect them, change the code or quit Airhand
- The host's tray always shows who is controlling the computer; disconnect them or change the code with one click
- The relay server can see the content of session streams (TLS ends at the server), so run it on a machine you trust
- Web console: the WebSocket only accepts pages from the same site (so other websites can't connect through your browser); an access password can be set (HTTP Basic login, then an HttpOnly, SameSite=Strict cookie; wrong passwords are answered with a delay); use HTTPS from other computers. The web console sees the picture too, so run it on a machine you trust. Everyone using it reaches the relay from that machine's IP, so they share the brute-force allowance
