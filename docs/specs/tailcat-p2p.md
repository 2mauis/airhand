# tailcat 临时点对点直连（Direct P2P transport）

> 状态：**规格草案，未评审，未实现**。本文件只描述设计；不含功能代码。
> 基线：上游 `sqlpxc/airhand` `main` @ `a317551`（协议版本 v9，工作区版本 0.2.2，MIT）。
> 集成对象：`https://github.com/tailscale/tailcat` @ `v0.7.0`（调研时 `main` @ `e00cdde`，BSD-3-Clause）。

## 背景/目标

现状（见 [architecture](../architecture.md)、`crates/airhand-link/src/net.rs`、`server/src/broker.rs`）：

- 被控端与控制端都**主动连出**到自建中转服务器（QUIC/UDP 4433，证书指纹固定）。
- 信令流（第一条双向流）完成注册 / 配对；每个会话的控制 / 画面 / 声音三条流由服务器按会话 ID 配对后**零拷贝透传**。
- 因此画面、声音、输入的全部流量都经过中转服务器：延迟多一跳、占中转带宽、服务器能看到会话内容（`docs/security.md` 已说明）。

目标：在**不改变使用方式**（仍用 6 位控制码连接）的前提下，让两端在条件允许时建立**临时的点对点 UDP 直连**，
媒体与输入改走直连，中转服务器只保留**信令 / 配对 / 兜底**职责，从而降低对中转的带宽与信任依赖。

选用 tailcat 作为 NAT 穿透与加密隧道：它复用 Tailscale 的数据面（userspace WireGuard + magicsock 打洞 + DERP 仅用于引导），
**不需要 Tailscale 账号、不需要 root / 管理员、不改路由与 DNS**；连接元数据由调用方带外交换（这里由 airhand 现有信令承载）。

### 已核实的 tailcat 事实（来自其 README / 源码，`v0.7.0`）

| 项目 | 结论 |
|---|---|
| LICENSE | BSD-3-Clause（允许使用、修改、再分发二进制与源码；须保留版权声明与免责声明，二进制再分发须在文档 / 材料中复现声明；不得借用贡献者名义背书）。传递依赖（`tailscale.com` BSD-3、`wireguard-go` MIT、gVisor Apache-2.0 等）须在实现阶段做完整许可审计并写入第三方声明 |
| 形态 | Go 库（`github.com/tailscale/tailcat`）+ CLI（`cmd/tailcat`）。`go.mod` 要求 Go 1.27.x |
| 机制 | 服务端得到一个 `tc…` 地址（含服务端 WireGuard 公钥、独立的路径发现公钥、默认的 256 bit 预共享密钥、DERP 区域）；客户端凭地址经 DERP 发 “Meow” 引导，随后 magicsock 用 STUN + disco 打洞，**成功则升级为直连 UDP**，失败则继续走 DERP 兜底 |
| 是否需账号 | 否（无登录、无控制面） |
| 是否真正直连 | 通常是（“usually”）；双方都在困难 NAT 且无端口映射时只能走 DERP。`Client.Ping` / `tailcat ping --until-direct` 能区分 “via DERP” 与 “via <直连地址>” |
| DERP | 默认使用 Tailscale 托管的**限速**免费 DERP（`https://tailcat.dev/derpmap.json`）；官方声明**无 SLA、可随时撤销**；可自建 `derper` 并把主机名嵌入地址 |
| 安全属性 | 默认地址是**不记名凭据**（含 PSK）；支持 `AllowClient` 钩子 / `--allow` 限制可连接的客户端节点公钥；威胁模型文档明确“早期实验性、原本面向同一人的两端”，对互不信任双方的使用仍在加固 |
| UDP | 库提供 `OnUDP` / `DialUDPPort`（`ConnPacketConn`），**单包载荷上限 `MaxUDPPayload = 1232` 字节**；**CLI 的 `serve` / `forward` 只转发 TCP**（UDP 只在 `exit-node` 与 socks UDP ASSOCIATE 里出现） |
| 稳定性 | README 明确：Go API、CLI 参数、线格式**都可能变化**，无稳定性承诺 |

### 集成形态（本规格的设计选择，待评审确认）

airhand 的传输是 QUIC（UDP）。只用 tailcat CLI 只能得到 TCP 转发，等于要把三条流改成 TCP 多路复用，
放弃 QUIC 的分流队列、BBR 与无队头阻塞，不可接受；而把 Go 运行时 cgo 嵌进 Tauri / Android 进程构建与体积代价过大。
因此选择：

- **方案 B（推荐）**：仓库内新增一个很小的 Go 模块 `p2p/`，产出独立可执行文件 `airhand-p2p`（“helper”），
  以 **tailcat 库（固定到某个已核对的 tag/commit）** 为依赖；Rust 侧以**子进程**方式拉起，通过 **stdin/stdout JSON 行**控制。
  helper 只做一件事：把 tailcat 隧道里的**一个 UDP 端口**与本机**回环 UDP 套接字**之间做数据报转发。
  QUIC 仍然是 Rust 的 quinn，只是对端地址变成回环地址；helper 不解析 QUIC。
- 方案 A（否决）：直接子进程调用 `tailcat` CLI —— 只有 TCP，且地址会出现在命令行参数（`ps` 可见）。
- 方案 C（否决）：cgo / `c-shared` 把 tailcat 嵌入 Rust —— 体积、交叉编译与崩溃隔离问题。
- 方案 D（否决）：用 Rust 重写 magicsock / disco / WireGuard 数据面。

方案 B 的代价与前提（Phase 0 验证项，见下文）：QUIC 数据报必须 ≤ 1232 字节；多一层用户态 WireGuard + gVisor 的 CPU 与延迟需实测。

## 总体设计

### 1. 会话先走中转，再就地升级到直连（make-before-break）

1. 连接流程不变：控制端用控制码经中转建立会话，三条流经服务器配对，**画面立即可用**。
2. 双方在 `Hello` 里声明是否支持直连（见“协议改动”）。仅当**双方都启用并支持**时才进入升级流程；任何一方不支持 / 未启用则与今天完全一致。
3. 被控端启动（或复用）helper 的 serve 模式，得到一次性的 tailcat 地址；在**控制流**（经中转）上发 `HostMsg::DirectOffer`。
4. 控制端启动 helper 的 dial 模式，在 `ControllerMsg::DirectAccept` 里回传自己 helper 的**客户端节点公钥**；
   被控端据此把 helper 的 `AllowClient` 限定为这一个公钥。
5. helper 报告路径状态；**仅当路径为 `direct` 时**才继续。超时（默认 8 秒，可配）或只有 DERP 则放弃升级、会话继续走中转，不报错。
6. 控制端用 quinn 经回环 UDP 连接被控端的**直连 QUIC 监听**（被控端监听回环地址，仅 helper 可达），并出示一次性令牌；
   被控端校验令牌与会话后，双方在直连连接上各开三条流（与中转上同样的 `SessionHeader`）。
7. **切换**：被控端把画面 / 声音 / 控制的**发送端**切到直连流，并立刻发一个关键帧；控制端同样把发送端切到直连流。
   **接收端始终同时读取中转流与直连流**，按媒体头里的帧序号 / 包序号去重，丢弃早于最近一次切换纪元（epoch）关键帧的旧流画面包。
8. **中转流保持打开作为热备**（空闲，不发媒体）。直连变差（见“健康检测”）时，发送端各自单方面切回中转流并请求关键帧；
   因为接收端一直两路都读，不需要握手即可恢复。

### 2. 健康检测与回退

- helper 周期性（默认 1 秒）报告路径；一旦变为 `derp` 或 `none` 即视为直连失效（**不让媒体走 Tailscale 的共享 DERP**，见不变量 I3）。
- 直连 QUIC 连接空闲 / 关闭、或超过 3 秒收不到对端 Ping / Ack，也视为失效。
- 失效后发送端切回中转；之后每隔 30 秒、最多 N 次（默认 3）重新尝试升级，避免来回抖动。
- helper 进程退出 / 崩溃：视同直连失效，会话不受影响；helper 由 Rust 侧监督，不自动无限重启。

### 3. 协议改动（需要把 `PROTOCOL_VERSION` 由 9 升到 10）

- **中转服务器代码不改**：直连协商全部走会话控制流（服务器只透传字节）。服务器仍按版本号拒绝不一致的客户端。
- `ClientMsg::Hello` / `ServerMsg::Welcome` / `Incompatible` 及 ALPN **编码不得变化**（`version_handshake_wire_is_stable` 保护）。
- `crates/airhand-proto/src/session.rs`：在 `HostMsg` / `ControllerMsg` 枚举**末尾**追加变体（占位示意，字段细节在实现阶段定稿）：
  - `HostHello` 增加能力位 `direct: bool`；`ControllerHello` 同。
  - `HostMsg::DirectOffer { epoch: u32, addr: String /* tc…，不记名凭据 */, quic_fp: [u8; 32], token: [u8; 16], ttl_secs: u16 }`
  - `ControllerMsg::DirectAccept { epoch: u32, client_key: String /* nodekey:… */ }` / `DirectDecline { reason }`
  - `HostMsg::DirectActive { epoch }` / `ControllerMsg::DirectActive { epoch }`（各自宣告发送端已切换，仅用于展示 / 诊断）
  - `HostMsg::DirectFallback { epoch }` / `ControllerMsg::DirectFallback { epoch }`（宣告已退回中转）
- `DirectOffer` 的负载设计成**可扩展枚举**（`kind`）：Phase 1 只有 `Tailcat`；预留 `Udp { candidates }` 给后续的 Tailscale / ZeroTier（见“后续阶段”）。
- 媒体包头（24 字节）与 `tests/protocol.json` 样例**不变**；Kotlin / TypeScript 实现不受影响（控制消息新增变体需要它们**忽略未知变体**——见 I8）。

### 4. 配置（默认关闭）

`config.toml` 新增 `[direct]`（桌面）与同名环境变量（网页控制台，Phase 1.5）：

| 键 | 默认 | 说明 |
|---|---|---|
| `enabled` | `false` | 总开关；关闭时**不会启动 helper、不会联系任何 tailcat / DERP 主机** |
| `helper_path` | 随安装包 | helper 可执行文件；校验存在且为普通文件 |
| `derp_map_url` | tailcat 默认 | 必须 `https://`；可指向自建 DERP 图；自建 DERP 时主机名可嵌入地址，不再访问 Tailscale 托管服务 |
| `upgrade_timeout_secs` | `8` | 等待直连路径的时间 |

### 5. 安全模型

- **威胁与信任假设**：中转服务器今天就是受信的（它能读会话内容）。本特性**不扩大**对它的信任：地址、令牌、证书指纹经它转发，
  但在升级完成后**媒体与输入不再经过它**，因此对中转运营者与路径上的被动观察者更安全；恶意中转仍可冒充（与现状等价），需在文档中明确。
- tailcat 地址是不记名凭据 → **每个会话一次性**：由被控端为该会话新建（或在 helper 内新建临时 Server），会话结束 / 升级失败立即销毁；
  永远不使用 `tailcat genkey` 的保存密钥；地址只经会话控制流下发，**不写日志、不进 `config.toml`、不放命令行参数、不进崩溃报告**。
- **双重限定**：helper 的 `AllowClient` 只允许控制端上报的那一个节点公钥；直连 QUIC 还要出示一次性令牌，并用被控端为该会话生成的临时自签证书指纹做固定校验（沿用现有证书固定逻辑）。
- **最小暴露面**：被控端 helper 只服务**一个 UDP 端口**，转发到回环地址的直连 QUIC 监听；不开 TCP、不开 exit-node / ssh / files / exec 服务。该监听只绑回环，不对外网卡开放。
- **同意与可见性不变**：直连会话与中转会话经过完全相同的“请求同意 / 托盘显示谁在控制 / 一键断开 / 更换控制码”路径；`Peer` 的展示信息仍取自中转看到的控制端信息。
- 被控端 Windows 服务以 SYSTEM 运行：helper 子进程应使用**最低权限**令牌启动（见风险）；helper 不读写磁盘、不加载保存的 tailcat 密钥。
- 隐私：启用时被控端 / 控制端会向 tailcat 的 DERP 与 DERP 图服务暴露自己的公网 IP 与连接时间（Tailscale 基础设施）；因此默认关闭，且文档与首次启用提示中说明；可改用自建 DERP。

## 非目标

- 不修改中转服务器（`server/`）与 Docker 部署；不改变 6 位控制码与重连凭证机制。
- Phase 1 **不做** Android 与网页控制台（`web/`）；它们列为 Phase 1.5 TODO。Phase 1 只覆盖桌面客户端（Windows / macOS / Linux）两端互连。
- **不实现 Tailscale 与 ZeroTier**（只在“后续阶段”写扩展点）；不接入任何 Tailscale 控制面 / 账号 / tailnet。
- 不使用 tailcat 的 ssh / files / exec / exit-node / socks 功能；不提供“把 airhand 当通用隧道”的能力。
- 不实现多路径聚合 / 同时多条直连；不做端口映射（UPnP / NAT-PMP）的额外逻辑（magicsock 自带的除外）。
- 不上游提交；不发布；不写 `main`。

## 不变量

- **I1** 默认关闭；关闭时行为与 v9 的字节级流量一致（不启动子进程、不联网到 tailcat / DERP）。
- **I2** 直连失败、超时、对端不支持、helper 缺失或崩溃，一律**静默回退**中转，不中断会话、不弹错误（只在诊断面板留痕）。
- **I3** 媒体 / 输入**永不走共享 DERP**：路径不是 `direct` 就不切换；已切换后变为 `derp` 立即退回中转。
- **I4** 握手消息（`Hello` / `Welcome` / `Incompatible`）与 ALPN 的编码不变；`PROTOCOL_VERSION` 升到 10。
- **I5** 会话 ID / 令牌一次性：令牌只对一个会话、一个 `epoch`、有效期内（`ttl_secs`）有效；重放、过期、错会话一律拒绝。
- **I6** 同一时刻每条逻辑流只有一个**发送**通道；接收端按帧序号 / 包序号去重，不重复显示、不倒退。
- **I7** tailcat 地址、客户端 / 服务端节点密钥、令牌不出现在日志、`Debug` 输出、命令行参数、`config.toml`、错误文案与崩溃信息里。
- **I8** 旧版本对端遇到未知控制消息不得崩溃（实现阶段需确认现有 postcard 解码对未知变体的行为；若会断开会话，则能力位握手必须先于任何新消息）。
- **I9** 直连监听只绑回环地址；helper 只服务一个 UDP 端口；不暴露 TCP 或其他服务。
- **I10** 同意 / 可见性 / 断开 / 更换控制码对直连会话的语义与中转会话相同。
- **I11** QUIC 在直连路径上的最大 UDP 载荷 ≤ 1232 字节且关闭 PMTU 探测上调（超限数据报会被 tailcat 丢弃）。

## 验收测试清单

测试命名采用 Rust `#[test]` / `#[tokio::test]` 函数名与 Go `Test…` 名；文件为**将要创建**的路径，实现阶段按“先红后绿”提交。
验证面均为**本机自动化**（回环 + 假 helper）；真实 NAT / 跨公网表现只能在**线上手动验证**（见风险）。

- [ ] I4：`PROTOCOL_VERSION == 10`，握手编码不变；测试：`crates/airhand-proto/src/framing.rs` →「`version_handshake_wire_is_stable`」（既有，必须仍通过）与 `crates/airhand-proto/src/session.rs` →「`direct_messages_wire_is_stable`」；输入与预期：固定样例的 postcard 字节与快照一致；验证面：本机。
- [ ] I1：测试：`crates/airhand-link/tests/direct.rs` →「`disabled_by_default_never_spawns_helper`」；输入：默认配置建立会话；预期：假 helper 工厂 0 次调用、无到 tailcat/DERP 的连接尝试；验证面：本机。
- [ ] 配置：测试：`client/src-tauri/src/config.rs` 单测 →「`direct_config_defaults_off`」「`direct_config_rejects_non_https_derpmap`」「`direct_config_has_no_shared_derp_media_override`」（配置里不存在允许共享 DERP 承载媒体的键）；验证面：本机。
- [ ] 升级：测试：`crates/airhand-link/tests/direct.rs` →「`session_upgrades_to_direct_when_both_sides_enable`」；输入：假 helper 回环直通、双方启用；预期：升级后画面包经直连流到达、中转流字节计数不再增长；验证面：本机。
- [ ] 对端不支持：「`peer_without_direct_capability_stays_on_relay`」；预期：无新消息发出、会话照常。
- [ ] I2/I3：「`direct_failure_falls_back_to_relay`」（断开假 helper 后 ≤ 3 秒内恢复画面且收到关键帧）、「`derp_only_path_never_carries_media`」（假 helper 报 `derp` ⇒ 不切换；已切换后报 `derp` ⇒ 退回）、「`upgrade_timeout_keeps_relay`」。
- [ ] I5：「`direct_token_is_one_time`」「`direct_token_wrong_session_rejected`」「`direct_token_expired_rejected`」；预期：被控端直连监听拒绝并保持中转会话。
- [ ] 证书固定：「`direct_cert_pin_mismatch_aborts_upgrade`」；预期：不切换、不泄露错误细节到用户界面。
- [ ] I6：「`duplicate_frames_across_lanes_are_dropped`」「`stale_lane_frames_before_epoch_keyframe_are_dropped`」；预期：帧序号单调、无重复渲染。
- [ ] I11：「`direct_quic_payload_capped_to_p2p_mtu`」；预期：直连用传输配置的最大载荷 ≤ 1232 且关闭 PMTU 探测；中转配置不变。
- [ ] I7：「`direct_secrets_are_redacted_in_debug_and_logs`」；预期：`Debug` / 日志文本不含 `tc` 地址、节点公钥、令牌。
- [ ] I9：Go 测试 `p2p/helper_test.go` →「`TestServesExactlyOneUDPPort`」「`TestNoTCPListeners`」（含对其他端口 / TCP 的拒绝）；Rust：`crates/airhand-link/tests/direct.rs` →「`direct_listener_binds_loopback_only`」。
- [ ] 一个客户端节点限定：`p2p/helper_test.go` →「`TestOnlyAllowedClientKeyConnects`」；输入：两个客户端，只有一个在 `AllowClient` 内；预期：另一个无法转发数据报。
- [ ] helper 协议：「`TestControlProtocolNeverEchoesAddressToArgvOrStderr`」；预期：地址只经 stdout JSON 行输出。
- [ ] I10：「`direct_session_requires_same_approval_and_shows_controller`」「`kick_all_ends_direct_and_relay_lanes`」「`new_code_ends_direct_session`」。
- [ ] I8：「`unknown_future_control_variant_is_ignored_or_rejected_cleanly`」（记录既有 postcard 行为，决定是否需要先握手能力位）。
- [ ] 回归：服务器与既有测试不变：`cargo test`（根工作区）、`cargo test`（`web/`）、`npm test`（`client/`）、`cargo test`（`android/native`）全部仍通过；`server/` 无 diff（CI 检查「`server_unchanged`」可用 `git diff --stat` 脚本校验）。
- [ ] 手动（不计入自动通过数，须单独记录）：两台不同网络的真机，开启 `direct.enabled` 后 `tailcat ping --until-direct` 等价路径为 direct；记录升级耗时、码率、延迟、CPU；对称 NAT 下确认回退中转。

## 阶段与门禁（对应 workflow v2）

| 阶段 | 内容 | 门禁 / 证据 |
|---|---|---|
| 0 调研 | 上游机制、tailcat 核实 | 已完成：见本文与 `tailcat-p2p-feature-map.md`、PR 描述 |
| 1 规格 | 本文 + feature map，走分支 + PR | 本 PR（Draft） |
| 2 设计评审 | 需求与架构评审（独立于实现），出分级 findings，修订后复审至无严重问题 | 见 `tailcat-p2p-review.md`（评审后提交） |
| 3 项目负责人批准 | 本变更属重大改动（安全边界、跨子系统、新增第三方运行时依赖）；spec 不代替批准 | 待 项目负责人对“评审通过的规格 + 开放问题的结论”给出批准 |
| 4 先红后绿 | 先提交行为断言失败的测试，再最小实现；Phase 0 spike 的结论先写入 PR | 实现 PR（另开分支） |
| 5 验收 | 本地全部套件的真实通过数 + 手动线上验证记录 | PR 描述 `## 验收清单` / `## 风险分级` |
| 6 合并 | 仅在本 fork，不向上游提 PR；需 项目负责人批准 | — |

> 工作流适配说明：`docs/workflow-v2.md` 与 `AGENTS.md` 来自另一个仓库（passkey 演示），其中 `tests/*.mjs`、CI 检查名（`spec-requires-tests`、`agent-trailer`、`privacy-scan`）是那个仓库的实现。
> 本 fork 目前没有这些脚本与 CI（上游也没有 `.github/`）。本规格把“验收映射到测试用例名”改写为 Rust / Go 用例名；
> 是否把检查脚本与 CI 工作流移植进本 fork，作为开放问题交 项目负责人决定（见下）。

## 后续阶段（TODO，仅扩展点，本次不实现）

- **Phase 1.5**：网页控制台（`web/` 容器内置 helper）与 Android（需要 gomobile / 嵌入式方案，无法像桌面一样拉子进程）。
- **Phase 2 — Tailscale**：已在 tailnet 内的两端不需要 tailcat：被控端把本机 tailnet 地址（Tailscale 使用的 CGNAT 段与 ULA 段）作为 `DirectOffer::Udp { candidates }` 候选，
  直连 QUIC 监听绑定到该地址（需要改为非回环监听，安全模型要单独评审），仍用一次性令牌 + 证书固定。地址发现只枚举本机网卡；**airhand 不接触 Tailscale 登录 / auth key / API**。
- **Phase 3 — ZeroTier**：同上；候选地址来自 ZeroTier 虚拟网卡（按网卡名前缀 / 用户配置的 CIDR 白名单枚举）。**网络 ID 与 token 不入库，airhand 不调用 ZeroTier 本地服务 API**。
- 共同点：`DirectOffer` 的 `kind` 扩展 + 升级 / 回退状态机复用；对应的新规格与评审另开。

## 风险

- **tailcat 无稳定性承诺**：API / 线格式可能变；对策：helper 依赖固定到已核对的 tag/commit 并记录哈希，升级走单独 PR + 回归；两端 helper 版本不一致时握手失败即回退中转。
- **性能未知**：QUIC 经回环 + 用户态 WireGuard + gVisor 的额外 CPU / 延迟 / 吞吐上限（4K、120 fps 码率较高）；Phase 0 spike 必须先量，不达标则本特性降级为“仅低码率档可用”或放弃。
- **1232 字节上限**：QUIC 载荷被迫 ≤ 1232；吞吐 / 开销略降；需验证 quinn 配置项在不改动中转配置的情况下可单独设置。
- **打洞失败率**：对称 NAT / 严格企业网络下只能走 DERP；本设计选择“拒绝 DERP 承载媒体、退回自建中转”，因此这类网络上**没有收益但也没有损失**。
- **共享 DERP 的可用性与隐私**：Tailscale 托管 DERP 限速、可撤销、可见元数据；默认关闭 + 可自建 DERP；若撤销，直连仍可能成立（DERP 仅引导）但引导失败时回退中转。
- **Windows SYSTEM 被控端**：子进程权限与加固（受限令牌、无文件写权限、Job 对象）需要实现阶段给出可验证的方案；不能让 helper 以 SYSTEM 身份面向网络。
- **新增运行时依赖与供应链**：Go 工具链、tailscale.com / gVisor / wireguard-go 体积（数十 MB 量级，待实测）与许可声明；安装包体积增长；需要 `THIRD_PARTY_NOTICES`。
- **协议版本升级**：v9 ↔ v10 不互通（中转本来就要求同版本）；回退方式：关闭 `direct.enabled` 并回滚版本。
- **仍需线上验证**：真实跨网络打洞成功率、码率与延迟、DERP 引导可用性——自动测试无法覆盖。

## 开放问题（需 项目负责人 / 评审结论）

1. 集成形态：确认方案 B（自带 Go helper 子进程，回环 UDP 转发）。备选 A（CLI，仅 TCP）、C（cgo 嵌入）已否决，理由见上；是否接受新增 Go 构建与第三方许可声明？
2. DERP 策略：默认使用 Tailscale 托管的限速 DERP 仅做引导，是否可接受？还是要求先自建 `derper`？“只有 DERP 时不承载媒体、退回自建中转”是否符合预期？
3. 升级方式：就地升级 + 中转热备（本设计）vs. 断开后以直连重建会话（更简单但有中断）。
4. 直连监听 / helper 在 Windows SYSTEM 被控端上的权限方案是否需要在批准前先做原型？
5. Phase 1 范围：仅桌面（Windows / macOS / Linux）；网页控制台与 Android 是否确认后置？
6. 本 fork 是否移植 workflow v2 的检查脚本与 CI（隐私扫描、署名 trailer、spec-requires-tests），还是沿用人工 + 一次性扫描？
7. fork 保持 MIT；helper 引入的第三方许可（BSD-3 / MIT / Apache-2.0 等）是否接受？是否需要在 README 加声明？

## 是否重大改动

**是**。理由：新增监听与子进程（权限与安全边界变更）、跨 `proto` / `link` / `client` / 新增 `p2p` 多个子系统、新增第三方运行时依赖、协议版本升级。
按 workflow v2「角色与硬约束」：重大改动必须等待 项目负责人批准；spec 与设计评审都不代替批准。批准结论记录在 PR 评论中。
