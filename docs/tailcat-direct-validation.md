# Tailcat direct MVP: validation and documentation audit

本记录区分代码实现、本机传输证据和待 Mac App 验收；与 [feature map](specs/tailcat-direct-feature-map.md)及中英文 README 一起维护。This record separates source implementation, local transport evidence and pending Mac App acceptance.

## Baseline and immutable archive imports

- Development branch: `feat/tailcat-airhand-transport`; PR base: `develop`.
- Verified starting HEAD: `80a48a4f9116668fcffcca2435332966a1ed322e` (includes the merged PR #17 and #18 baseline).
- Archive source: `review/tailcat-handoff` at `68bd017860b8fe6984a5350ce68fceb698ed0b7c`; never used as the development base.
- `docs/specs/tailcat-direct-mvp.md`: blob `36e5707c26fae7e0147a07a6ea4629b942d42304`; SHA-256 `5c8edb41d3b0239af7fe7106e7f403bbf727cac202ac06c6040db305e01ec2f8`. Imported byte-for-byte; its review-only wording is preserved.
- `docs/tailcat-handoff.md`: blob `f96f35793ea7ed2699c3ed3fd6ffb2f9f0f45c2c`, identical to the archive source. It still describes the handoff baseline, not the status after this implementation.
- The original relay-upgrade designs/reviews remain in their original paths, with an archive notice prepended. They are not the direct MVP's acceptance authority.

## Local evidence / 本机证据

Tooling: Linux cloud environment; Go 1.27.1, locked Tailcat v0.7.0 and locked tailscale.com module; existing Rust/Node dependencies. System build prerequisites were unpacked into a scratch sysroot; no project dependency declarations, firewall rules or macOS settings were changed.

| Check | Result and scope |
|---|---|
| Exact archive import hashes | Passed; design blob/SHA-256 and handoff blob match their specified archive |
| First-red reproduction | Committed before implementation: missing control handling/argv and zero served UDP ports failed assertions |
| Focused Go helper tests / vet | Passed: control protocol, allowlist, redaction, write watchdog and `go vet`; the full historical suite is reported separately |
| Real helper-binary transport | Passed locally: hermetic DERP/STUN, both direct path reports, 1200-byte synthetic datagram round trips, unlisted-key refusal, steady DERP data counters unchanged and EOF exit within one second; startup cancellation is also checked |
| Race instrumentation | Passed: focused helper checks and the actual helper binary built with race instrumentation, including startup/EOF cancellation |
| Local quinn/session/transcript tests | Passed: three-lane binding, duplicate/foreign/fourth streams, partial-header timeout, wrong server/client pins, default-deny consent with no Hello/media, expiry/tampering/replay/commitment/SAS and Secret Debug |
| Helper-prepared Ed25519 mTLS | Passed: stdin-seeded certificate and Rust PKCS#8 identity establish mutually authenticated quinn; this is still local QUIC, not a desktop |
| Rust supervisor lifecycle | Passed: private stdin, argv inspection and owner Drop reaping the helper within one second |
| Root and desktop Linux clippy | `--all-targets --locked -- -D warnings` passes; macOS-only code is not compiled by a Linux check |
| Desktop Rust library | 35 tests pass, 6 hardware/opt-in tests remain ignored; the opt-in supervisor test was also run explicitly and passed. The serialized store test covers concurrent updates and failed persistence without committing stale state |
| Frontend | Production build, typecheck and 53 existing tests pass; no claim of native AirDrop/TCC UI execution |
| Shared-library consumers | Web and Android-native `cargo check --all-targets --locked` pass; this does not implement direct transport on those platforms |
| Relay regression | Existing server's 10 tests pass; `server/` is unchanged |
| Workflow script self-tests | 149/149 pass; policy files and workflows are unchanged |
| Full historical Go suite | 25 existing first-red gate/relay-upgrade tests fail in the old `gate.go` / `glue.go` fixtures; these fixtures do not drive the new direct bridge |
| Full root Rust suite | Stops at 9 historical first-red `approval` fixture failures. This is not a full-suite green result; focused direct tests are reported separately |

The local binary test runs actual helper processes, not the Tailcat CLI. Its DERP metrics use the fixed-version names `magicsock_send_data_derp` and `magicsock_recv_data_derp`. Safe counters are emitted periodically and at gate closure, and exposed under App diagnostics. CurAddr/Endpoint remain internal to the helper; a direct event means the corresponding role-specific observation passed. There is no claim that detection windows carry zero DERP data.

The historic two-Mac library-level 1024-byte echo results and PR #17's relay desktop run remain in the handoff archive. Neither is counted as Tailcat App screen/input acceptance.

## Implementation choices and remaining review

- The desktop supervisor is in `client/src-tauri/src/direct/helper.rs`, using the existing standard-library process API with private pipes, EOF handling, a bounded Drop reaper and watch-based failure propagation. It provides the ownership behavior requested by the design without adding tokio process features or a new dependency. It does not claim to call `tokio::process::Command::kill_on_drop`; review this API choice alongside the lifecycle evidence.
- Go's existing standard `crypto/x509` generates a certificate from a TLS seed supplied by Rust on stdin. A separate parent-only setup response returns the derived public certificate/node identity and host invitation address; telemetry remains category/count-only. The address is consumed into the pairing record, never logged or rendered.
- The relay-facing API retains a non-optional resume-token result for compatibility; the desktop `SessionInfo.resume` is optional for direct sessions. No relay protocol version or `server/` change was needed.
- Pairing read/modify/persist/cache updates are serialized. Removal and stale-confirmation checks cannot restore a removed pair through a concurrent cached write; a failed persistence operation does not commit the in-memory mutation. Native Keychain behavior is still pending.
- macOS sleep/wake notifications stop the listener/session, in addition to path/ACK watchdogs. Their actual OS delivery and release timing are pending native testing.
- Exported invitation files are explicit transfer artifacts, contain a secret and need deletion after transfer. The controller deletes its imported copy after durable consumption. No claim of secure erasure or deletion of remote copies is made.
- 独立代码评审尚待进行，MVP 只处理 S1 和阻断性 S2。S3 留后续：分享完成回调与对象清理、提示文案/语言及焦点细节、内存秘密额外清零加固；本轮不扩大实现。

## Pending signed macOS / two-device acceptance

Every item below is **pending**, not inferred from cloud or old relay evidence. 云端无法完成以下真机项目，均未标记通过。

| Area | Required evidence |
|---|---|
| Native build and permissions | Native Apple bundle compile, existing fixed development signature on App/sidecar, isolated identifier, Keychain ACL continuity, screen recording/Accessibility on the main App, local-network prompt ownership and denial behavior |
| Pairing | App-only AirDrop R → explicit pairing allow → AirDrop I → import/delete → matching first-session SAS; no manual long strings or clipboard secrets |
| Real desktop | Controller Tauri window renders the host's actual desktop, with real keyboard/mouse input after session consent; later one-click reconnect asks again |
| LAN and hotspot | Separate ten-minute runs, both helper direct reports, unchanged steady-state DERP application-data counts |
| Negative | Third node with the address rejected without popup; local loopback process rejected by mTLS; used I, altered R/I rejected; deny/timeout exposes no picture; host pair removal makes old keys unusable |
| Revocation | Session revoke stops picture/input within two seconds; optional input consent is enforced; busy host refuses another request |
| Path failure | Hold Shift and mouse button, interrupt direct connectivity without weakening security settings; record actual ReleaseAll delay X (goal 1–1.5 seconds), lost-path UI and failure-window DERP counts; no old-session continuation after recovery |
| Process and sleep faults | Kill App: helper exits within one second; kill helper: session ends and keys release; sleep/wake leaves no old session |
| MTU / performance | MTU black-hole behavior, first-connect/PTO latency, sustained throughput, frame bitrate, input/render latency and both CPUs; no hardware performance numbers invented |
| Secret inspection | Inspect App logs, helper stderr, argv, crash reports and clipboard for address/PSK/private key/peer IP; publish only scrubbed counts/categories |

Record topology and DERP region privately using anonymized labels in review artifacts; do not publish real addresses, hostnames, personal information or credentials. Long-term DERP data fallback is undecided. Remembering a pair is a switchable experimental direction, initially enabled; standing DERP presence requires the per-run Allow connections switch. Session authorization is never remembered.

No new CI is added before this acceptance. The Draft PR is for review and real-Mac follow-up; no merge or deployment is authorized.

## Documentation audit / 逐文件核对表

Each preserved upstream-only subsection carries an explicit scope notice at its start. “已改 + 已标注 upstream” means a fork-specific section was added and the inherited content was scoped, rather than presented as this fork's current direct behavior.

| 文件 | 处理方式 | 原因 |
|---|---|---|
| `README.md` | 已改 | 改以新设计/映射/验证记录为入口，移除把多平台中转宣传当成直连能力的表述 |
| `README.zh-CN.md` | 已改 | 与英文实现状态、待验收及排除范围同步 |
| `docs/architecture.md` | 已改 + 已标注 upstream | 补直连图、双层身份、三通道与授权边界，保留旧 relay 架构及媒体实现背景 |
| `docs/architecture.zh-CN.md` | 已改 + 已标注 upstream | 同步中文直连图与范围，旧性能数值不算 Tailcat 证据 |
| `docs/deploy.md` | 已改 + 已标注 upstream | 明确直连不部署 AirHand 服务；原服务器/网页部署仅保留给继承模式 |
| `docs/deploy.zh-CN.md` | 已改 + 已标注 upstream | 同步中文无独立 relay/信令要求 |
| `docs/usage.md` | 已改 + 已标注 upstream | 加 App R/I、SAS、逐会话输入授权、撤销、持久化开关、Keychain/文件清理 |
| `docs/usage.zh-CN.md` | 已改 + 已标注 upstream | 同步中文流程；旧控制码与卸载步骤不冒充本 fork 说明 |
| `docs/security.md` | 已改 + 已标注 upstream | 加 WG/mTLS、回环、秘密、失败窗口及原生验证边界 |
| `docs/security.zh-CN.md` | 已改 + 已标注 upstream | 同步中文信任模型；旧中转 TLS 终止/猜码防护另行标注 |
| `docs/limitations.md` | 已改 + 已标注 upstream | 加单对 macOS、未验收、P1/其他平台排除、邀请副本与 DERP 待决 |
| `docs/limitations.zh-CN.md` | 已改 + 已标注 upstream | 同步中文限制与不得冒称通过的边界 |
| `docs/building.md` | 已改 + 已标注 upstream | 加已有固定签名身份、Go sidecar、macOS 覆盖配置、独立 ID；旧 ad-hoc/relay 说明仅历史 |
| `docs/building.zh-CN.md` | 已改 + 已标注 upstream | 同步中文构建与待原生编译/签名验证，不要求关闭系统安全设置 |
| `docs/development.md` | 已改 + 已标注 upstream | 加本机直连门禁、旧先红失败分类、双机通过后才新增 CI |
| `docs/development.zh-CN.md` | 已改 + 已标注 upstream | 同步中文测试范围与真实桌面验收区别 |
| `docs/android.md` | 已改 + 已标注 upstream | 明确 Android 不在本轮直连范围，手机/TV 中转说明保留 |
| `docs/android.zh-CN.md` | 已改 + 已标注 upstream | 同步中文排除范围 |
| `docs/specs/tailcat-direct-mvp.md` | 按字节导入 | 批准设计为不可改写的实现/验收依据 |
| `docs/tailcat-handoff.md` | 按字节导入 | 保留旧基线、来源和库层/中转证据的原始边界 |
| `docs/specs/tailcat-direct-feature-map.md` | 新增 | 逐项标注已实现 / 未实现 / 待真机验收，并与两份 README 联动 |
| `docs/tailcat-direct-validation.md` | 新增 | 集中记录本机证据、失败项、待真机项目及本核对表 |
| `docs/specs/tailcat-p2p.md` | 已标注历史归档 | 原路径保留；中转升级设计被新 direct MVP 取代 |
| `docs/specs/tailcat-p2p-feature-map.md` | 已标注历史归档 | 原路径保留，不再由 README 作为当前能力入口 |
| `docs/specs/tailcat-p2p-phase0-report.md` | 已标注历史归档 | 保留旧实验，避免把窗口/性能结果当作当前 App 证据 |
| `docs/specs/tailcat-p2p-review.md` | 已标注历史归档 | 保留旧设计评审，与新 direct 规格区分 |
| `docs/specs/review/tailcat-p2p-round1.md` | 已标注历史归档 | 旧设计评审记录原文及路径保留，仅加归档说明 |
| `docs/specs/review/tailcat-p2p-round2.md` | 已标注历史归档 | 同上 |
| `docs/specs/review/tailcat-p2p-round3.md` | 已标注历史归档 | 同上 |
| `docs/specs/review/tailcat-p2p-round4.md` | 已标注历史归档 | 同上 |
| `docs/specs/review/tailcat-p2p-round5.md` | 已标注历史归档 | 同上 |
| `docs/specs/review/tailcat-p2p-round6.md` | 已标注历史归档 | 同上 |

The eight requested English/Chinese document pairs were all reviewed; none was left unscoped. No archived branch was merged, no historical record was deleted and the two imported files were not rewritten.
