# AirHand × Tailcat：单对 macOS 真机 MVP 方案评审（只评审，不改代码）

## Context

- 目标：在 fork 的本地 `feat/tailcat-desktop`（基于 `2mauis/airhand@0740a91a`）上做单对 macOS MVP：Mac mini 是被控端，MBP 是控制端。**不用 AirHand relay 或信令服务**，只保留 Tailcat v0.7.0 原生的 DERP/STUN。
- 验收：真实桌面画面和键鼠都走 Tailcat，并且 **UDP direct 才算直连**；允许经 DERP 引导。应用数据能不能长期走 DERP 兜底是**产品待决项**，本方案不擅自决定。试验期间一旦路径丢失，就停止输入、释放按键，不冒称直连。
- 用户最关心的是：怎样融入现有 AirHand 的交互，不用手输 Tailcat 长串。

### 来源核对情况（只读公开 raw URL，没有申请任何权限）

| 来源 | 状态 |
|---|---|
| `docs/specs/tailcat-p2p.md` @SHA | 已读技术部分：背景、已核实事实、集成形态、§1 升级流程、§2 路径检测（部分）、§5 安全模型、握手/主动断开/防注入。跳过了历史归属与各轮处理记录。全文 341 KB，部分长行被截断，**不声称已逐字读完** |
| `p2p/helper.go` | 全文已读 |
| `crates/airhand-link/src/{controller/mod.rs, host.rs, transport.rs}` | 全文已读 |
| `server/src/broker.rs` | 已读信令、配对、`attach_stream` 和 `relay` 部分 |
| tailcat `v0.7.0` `README.md` | 全文已读 |
| tailcat `v0.7.0` `tailcat.go` | 已读 `Server` 和 `Client` 的字段、`Start`、Meow 处理、`AddAllowedClient`、`PublicKey`、`DiscoPing`、`Status` |
| 顺带读到的文件 | `net.rs`（grep）、`client/src-tauri/src/host/session.rs`（grep）、`tauri.conf.json`、`Info.plist`、`p2p/go.mod`、`p2p/helper_test.go`（只看了测试名） |
| 打不开 | GitHub tree API：本会话没有该仓库的 API 权限，按要求没有申请。未读：Android 确认代码、`pipeline.rs`、`airhand-proto` 细节 |

---

## 1. 为什么现有真机证据不覆盖 App

1. **helper 是“先红”骨架**（已有代码）：
   - `RunControl` 返回 nil，`HelperArgv` 返回 nil；
   - `Server` 的方法都是 `noop == lenient` 的桩；
   - `EncodeEvent` 和 `TunnelWriter` 在 `noopActive` 时直接短路。

   所以 App 根本无法拉起和驱动 helper。双机证据来自 Tailcat CLI 或测试程序，不是 App 进程。
2. **会话建立硬依赖 broker**（已有代码）：
   - `ControllerService::connect` 需要 `live`（连着服务器的连接）和 `SessionGrant`；
   - `host::run` 只在收到 `ServerMsg::Incoming` 后才开始会话；
   - `open_session` 让**两端都** `open_bi` 三条流，再由 `broker::attach_stream` 按 `session_id`、角色、通道配对，然后 `relay` 透传。

   直接对接的话，会得到 6 条没人配对的流，无法工作。
3. **合成 UDP 回显 ≠ quinn QUIC**：
   - 没验证过 1232 字节 MTU 下的 quinn（MTU 探测上探超过 1232 时，包会被静默丢弃）；
   - 没验证过 BBR、三条通道的优先级、keep-alive 与闸门的交互；
   - 没测过真实画面码率下，用户态 WireGuard + gVisor 的 CPU 和延迟。
4. **白名单拒绝只证明了隧道层**：
   - 被控端本机进程能否打到回环上的 QUIC 监听、能否开会话注入输入，没有覆盖；
   - App 层的 mTLS、会话绑定、用户授权都还不存在。桌面被控端 `session.rs` 一上来就发 `HostMsg::Hello`，没有确认步骤。
5. **macOS 运行面没测过**：
   - 屏幕录制和辅助功能权限（TCC）、本地网络隐私权限、签名后的 sidecar 都没测；
   - `tauri.conf.json` 的 `bundle.targets` 只有 `nsis`，没有 macOS 包配置。
6. **没测过路径丢失**：断路时 `ReleaseAll`、停止输入、UI 不冒称直连，这些在 App 里都没测过。

---

## 2. 不依赖 AirHand server 的方案对比

| 方案 | 做法 | 优点 | 缺点 | 结论 |
|---|---|---|---|---|
| **A：helper sidecar + 回环 UDP 桥 + 端到端 quinn QUIC**（spec 方案 B 的 (i)） | Go helper 链接 tailcat 库。被控端：`OnUDP` 单端口 ↔ `127.0.0.1` 上的 quinn 服务端。控制端：`DialUDPPort` ↔ 本机 quinn 客户端。三条通道照旧在一条直连 QUIC 上 | 复用 `transport.rs`、`HostReceiver`、`Pipeline`、`Injector`、`net.rs` 的证书固定；helper 已有接口和测试接缝；崩溃隔离；秘密只走 stdin | 回环端口对本机其他进程可见 → **必须加 mTLS**；多一层用户态 WG 的开销需实测 | **推荐（MVP）** |
| B：同 A，但数据报走继承的 socketpair（spec (ii)） | 为 quinn 自写 `AsyncUdpSocket`，经 Unix 数据报 socketpair 收发 | 不开本机端口 | 自定义 socket 的工作量和风险更大 | 列为 P1 加固 |
| C：Tailcat TCP（CLI `serve`/`forward` 或库的 `OnTCP`） | 三条通道改成 TCP 复用 | 实现简单 | 丢掉 QUIC 的分流与无队头阻塞，要重写传输；用 CLI 时地址会出现在 argv | 否决 |
| D：cgo `c-shared` 把 tailcat 嵌进 Tauri | 进程内调用 | 不用管子进程 | Go 运行时进 App、体积大、崩溃不隔离、交叉编译麻烦 | 否决（同 spec） |

**推荐 A。** 和 spec 的差异：没有中转，就没有“先中转、再升级”。改成**纯直连会话**：直连 QUIC 建立之前没有画面也没有输入；不满足 direct 就不启动会话。

---

## 3. 四个层次：发现、认证、授权、秘密，以及六位短码的边界

| 层 | 由什么承担 | 说明 |
|---|---|---|
| **发现 / 可达** | Tailcat address 里的 DERP 区域 + 服务端公钥；DERP 负责 rendezvous，STUN/disco 负责打洞 | 不需要 AirHand 目录服务。address 要用 `TailcatAddr()` 的完整形态（内嵌 DERP 区域，免拉 DERP 图） |
| **身份认证（双向）** | ① 隧道层：被控端 `AllowedClients=[控制端节点公钥]`；控制端用 address 固定被控端的服务端公钥，再加 PSK。② App 层：直连 QUIC 双向 mTLS，各自固定对方的自签证书指纹（复用 `net.rs` 的 `PinnedVerifier`，再加一个客户端证书校验器） | 两层都要有。① 挡住外部节点（双机已实测）；② 挡住被控端本机进程经回环端口注入（设想，需实测） |
| **会话授权** | 每个会话在被控端弹窗：允许 / 拒绝，默认拒绝，超时也拒绝，可勾选“输入控制”；常驻指示上可随时撤销 | **配对 ≠ 授权。** 授权在认证通过之后才弹，所以未认证的节点连弹窗都看不到（防骚扰） |
| **秘密** | address 含 256-bit PSK，是不记名凭据；另有节点私钥和 TLS 私钥 | 只存在 Keychain 里，只经 stdin 交给 helper。不进 argv、env、日志、剪贴板，不进 UI（诊断模式除外）。因为白名单非空，address 泄露本身不能连入，但仍要按秘密处理（PSK 还要防 DERP 运营者） |

### 六位短码在无信令服务时

- **能做**：
  - 作为**短校验串（SAS）**，在两台机器上对比，用来确认配对（必须配合下文的“先承诺、后揭示”）；
  - 作为本地已配对记录的人类可读标签。
- **不能做**：
  - 发现（没有“码 → 地址”的目录）；
  - 身份（约 20 bit，可以穷举）；
  - 承载 address（≥140 B）；
  - 当 PAKE 口令用（需要会合 / 邮箱服务器，magic-wormhole 那种，被排除）；
  - 对单条消息做 SAS 也不行：攻击者生成约 10⁶ 把密钥就能磨出同码。
- **扩展（P2）**：同一局域网内可以用 Bonjour 发现 + 六位码做 SPAKE2，不需要服务器。只适合同网段，MVP 不做。

---

## 4. 交互方案对比（避免手输长串）

| 方式 | 定位 | 评估 |
|---|---|---|
| 直接粘贴长串 | **仅诊断**：debug 构建或隐藏开关 | 需要双向交换（address ←、控制端公钥 →）两个串。macOS 通用剪贴板会同步到 iCloud 设备，还会进剪贴板历史。不进正式 UI |
| **可信渠道两步一次性邀请文件（AirDrop）** | **推荐的配对方式** | 见下面的流程。二维码作为扩展：被控端显示 I，MBP 用连续互通相机扫码；屏幕上会露出秘密，需要短时显示 |
| **已配对设备一键重连** | **推荐的日常使用方式** | 不是配对手段，是配对之后的常态。被控端每个会话仍然弹窗授权；授权记忆默认关（沿用 spec OQ-25 的取向） |

### 两步配对流程（设想；单对，单个未完成请求）

1. **MBP 端，“配对新的被控端…”**：
   - 生成或载入控制端身份：tailcat 节点密钥、TLS 客户端证书；
   - 生成 32 B 随机数 `Nc`，导出请求文件 `R = {v, ctrl_node_pub, ctrl_tls_fp, C=H(Nc), req_id, expires(10min), 自报名}`；
   - R **不含秘密**，经 AirDrop 发给 Mac mini。
2. **Mac mini 端，“导入配对请求…”**：
   - 弹出确认窗：自报名标注“不可信”，显示公钥指纹摘要；按钮为“允许配对”或“拒绝”，默认拒绝；
   - 允许后，生成**该配对专用**的 tailcat 服务端密钥 + PSK 和被控端 TLS 证书，以及 `Nh`；
   - 导出邀请文件 `I = {v, tailcat_addr, host_tls_fp, Nh, H(R), expires}`，I **含秘密**；
   - 记录状态设为 `pending`，同一个 `req_id` 只能用一次。
3. **MBP 导入 I**：校验 `H(R)` 等于本机未完成的请求，并且没过期，然后**消费**该请求（防重放），写入 Keychain，再删掉 I 文件（提示用户）。
4. **首次连接**：
   - 建立 mTLS QUIC 后，控制端在隧道内揭示 `Nc`；
   - 两端各自显示 `SAS = trunc6(H(Nc‖Nh‖R‖I))`；
   - 被控端用户确认“两边一致”后，记录变为 `paired`，否则销毁；
   - 因为 Nc 事先已承诺，中途替换 R 的人每次尝试只有 10⁻⁶ 的成功率。
5. **撤销**：被控端“移除已配对设备” → `Close` 该 tailcat Server，删除密钥和 PSK（v0.7.0 没有 Remove API，白名单也不能为空）。控制端同样提供“忘记此设备”。

> **产品待决（不擅定）**：一键重连要求被控端为该配对**持久化**服务端密钥和 PSK，并在“允许被连接”时常驻 DERP。spec 原本是“每 epoch 一次性”，那是为有中转的场景设计的。另一个选择是每次都重新走邀请，安全性更强，但体验差。MVP 建议持久化，以白名单为主防线，撤销时轮换。

---

## 5. 关键设计点（标注：已有代码 / 设想 / 需实测）

### 5.1 QUIC 通道与控制、音视频的绑定

- 控制端是 QUIC 客户端：在一条连接上 `open_bi` 三条流，带 `SessionHeader`。被控端新增 `accept_session` 逐条 `accept_bi` 并读头。规则：
  - 三条流的 `session_id` 必须相同、通道不能重复；
  - 10 s 内收齐三条，否则关闭连接；
  - 第四条流直接关闭连接；
  - 每条连接只允许一个会话。
  
  这套逻辑参照 `broker::attach_stream`（已有代码可借鉴）。
- 用独立的 ALPN（如 `airhand-direct/1`）和自己的版本号，不触碰中转版的 `PROTOCOL_VERSION`。
- 被控端 quinn 端点只接受来自 helper 回环 4 元组的连接（一个 tailcat Server 对应一个配对，所以 4 元组 ⇒ 节点公钥）。mTLS 的证书指纹必须和该配对记录一致，这样把“隧道身份”和“QUIC 身份”交叉绑定起来。
- 直连专用的 `TransportConfig`（需实测）：`initial_mtu=1200`、`min_mtu=1200`，MTU 探测关闭或上限 ≤1232；keep-alive ≤250 ms，让入站静默检测可用；idle 超时沿用 10 s。

### 5.2 路径闸门与故障关闭

- helper 闸门：只有 path=direct 才双向转发（设想；spec §2 已有推导）。
  - 被控端：`Server.Status()` 的 `CurAddr` 非空，并且 1000 ms 内有入站；
  - 控制端：每 250 ms 流水线式 `DiscoPing`，返回 `Endpoint` 非空才算 direct。
  - helper 不从端点字符串推断 DERP（已有代码 `EncodeEvent` 的取向）。
- helper 经 stdout 发 JSON 事件，只带路径类别和计数。Rust 收到非 direct、helper 退出或写超时（`writeWatchdog` 已有代码）时：
  - **被控端**立即 `injector.release_all()`，之后的 `Input` 全部丢弃；
  - **控制端**停止发送输入，UI 显示“直连丢失，输入已停止”。
- 闸门恢复后是否续用，在 MVP 里先做“断开会话”（最简单，也不冒称直连）。
- 已知界限：判定有 1–1.5 s 窗口，窗口内可能有 0–10 个包经 DERP（spec Phase 0 数据，云 VM 上测的）。**macOS 双机需实测。**
- 首包：被控端闸门打开晚于控制端时，QUIC Initial 会被丢弃，要等约 1 s 的 PTO 重传。MVP 接受这一点并记录耗时；无中转时的 `DirectGateOpen` 替代方案列为 P1。

### 5.3 helper 与 App 的生命周期

- 用 `tokio::process` 启动，`kill_on_drop(true)`；stdin 管道传 JSON 配置（密钥、PSK、白名单、端口）。`HelperArgv` 只返回固定标志，不带秘密。
- helper 在 stdin EOF 时退出。macOS 没有 `PDEATHSIG`，所以再每秒检查一次 `getppid()==1`，是则退出。
- helper 的 stderr 不原样记录，只记退出码和类别（已有测试 `TestHelperEventsAndErrorsContainNoEndpoint` 等）。`Logf` 默认丢弃（已有测试）。
- 被控端菜单栏“允许被连接”开关：开 → 起 tailcat Server；关 → `Close`，会话结束，DERP 上不再在线。
- App 退出、helper 崩溃、会话任务被取消，都按路径丢失处理：先 `ReleaseAll`。

### 5.4 macOS 隔离配置（都需实测）

- fork 构建使用独立的 bundle identifier（如 `app.airhand.client.tailcatdev`），与正式版的 TCC、Keychain、配置目录隔离。
- 用固定的开发签名，否则每次重编译 TCC 授权都会失效。
- 加入 macOS 包目标（`--bundles app`）；helper 作为 `bundle.externalBin` sidecar，同一签名，开启 hardened runtime。
- `Info.plist` 加 `NSLocalNetworkUsageDescription`：macOS 15+ 有本地网络隐私，同一局域网下的 direct 很可能走 LAN 地址，权限归属到负责进程，要实测弹窗和拒绝后的行为。
- 确认应用防火墙对 helper UDP 监听的提示。
- helper 不读写磁盘（tailcat 用内存 `mem.Store`，需核对），不访问 Keychain（由 Rust 侧读）。
- 屏幕录制和辅助功能只授予主 App。

---

## 6. 优先级

**P0（MVP 必须有）**
1. helper 实装：`RunControl`/`HelperArgv`、真实的 Server/Client、UDP 桥、闸门、事件；新增 `cmd/airhand-p2p`。现状：已有接口、测试接缝和先红测试 → 待实装。
2. 去 broker 的直连会话：`accept_session`、`HostSession::accept`，把 `ControllerService::connect` 拆成“拿授权”和“attach”两段，再加 `connect_direct`。现状：设想。
3. 双向认证：WG 白名单和 address 固定（tailcat 已有，已实测）+ QUIC mTLS（服务端固定已有代码，客户端证书是设想）。
4. 白名单非空，否则 fail-closed；撤销时 `Close` 后重建或停止。现状：`StartServer` 已有校验代码；v0.7.0 行为已核对。
5. 两步配对、绑定 `H(R)`、一次性、过期、首会话 SAS 确认，加 Keychain 存储。现状：设想。
6. 被控端每会话授权弹窗和常驻撤销；有带输入权限的在途会话时，新请求直接拒绝。现状：设想；桌面基线没有确认步骤。
7. 路径丢失 → `ReleaseAll` + 停止输入 + UI 不冒称直连。现状：写看门狗已有代码，其余是设想，时限需实测。
8. quinn MTU ≤1232、keep-alive。现状：需实测。
9. macOS 签名 sidecar、TCC、本地网络权限、identifier 隔离。现状：需实测。
10. 秘密只在 Keychain 和 stdin；Rust 侧用 `Secret<T>` 包装，`Debug` 脱敏。现状：helper 侧已有测试，Rust 侧是设想。

**P1**
- socketpair 数据报通道（方案 B）；
- 首包 gate-open 优化；
- 闸门恢复后续用会话；
- 授权记忆；
- 多配对时 helper 给数据报标注来源公钥；
- 吞吐、CPU、码率上限；
- 长期 DERP 兜底（等产品决定）；
- 二维码、文件关联、Bonjour；
- 双机实测通过后再上 CI。

---

## 7. 最小改动文件（`server/` 不动）

- `p2p/helper.go`：实装逻辑，保留 `Backend` 等接缝；新增 `p2p/cmd/airhand-p2p/main.go`；扩充 `p2p/helper_test.go`。
- `crates/airhand-link/src/transport.rs`：加 `accept_session`（含绑定校验）。
- `crates/airhand-link/src/host.rs`：加 `HostSession::accept`；直连监听循环复用现有 `session` 闭包签名。
- `crates/airhand-link/src/controller/mod.rs`：把 `connect` 里 grant 之后的部分抽成 `attach`；加 `connect_direct`；`SessionInfo.resume` 改为 `Option`。
- `crates/airhand-link/src/net.rs`：加 `direct_transport_config()` 和客户端证书校验器（复用 `PinnedVerifier` 的写法）。
- 新增 `crates/airhand-link/src/direct/{helper.rs, pairing.rs, tls.rs}`：子进程与事件、R/I 格式与 SAS、自签证书。
- `client/src-tauri/src/host/session.rs`：在 `HostMsg::Hello` 之前加授权闸门；路径事件触发 `release_all` 并丢弃输入。
- `client/src-tauri/src/host/mod.rs` 和 `lib.rs`：菜单栏“允许被连接”“导入配对请求…”“已配对设备”，以及 Tauri 命令。
- `client/src/`（前端）：控制端的“配对新的被控端…”、已配对列表加“连接”按钮、路径徽标（引导中 / 直连 / 直连丢失）、SAS 显示。
- `client/src-tauri/tauri.conf.json`、`Info.plist`：`externalBin`、macOS 目标、fork 用的 identifier、`NSLocalNetworkUsageDescription`。
- 文档：新增 `docs/specs/tailcat-direct-mvp.md`，写明与 spec 的差异和信任假设；同步 `docs/security.md`。

### 最小 macOS UI 改动

- **被控端（菜单栏）**：
  - “允许被连接”开关；
  - “导入配对请求…”；
  - “已配对设备 ▸ 移除”；
  - 会话授权弹窗（允许 / 拒绝 + 输入勾选 + 倒计时）；
  - 会话中的常驻指示（设备名 + 路径 + “断开”）；
  - SAS 确认窗。
- **控制端**：
  - “配对新的被控端…”（导出 R 并调起 AirDrop 分享面板）；
  - “导入邀请…”；
  - 已配对列表的“连接”按钮；
  - 会话里的路径徽标和“直连丢失，输入已停止”提示；
  - SAS 显示。

---

## 8. 开发步骤（每步有门禁，实测通过后才进入 CI）

0. **准备**：从 SHA 拉分支；准备开发签名和独立 identifier；记录 macOS、Go、Rust、tailcat 版本。
1. **helper 实装**：先红测试转绿（`go test ./p2p/...`）。双机上用**helper 二进制**（不是 CLI）复现已有证据：建立 direct、白名单外节点被拒、DERP data 不增加。
2. **QUIC 探针**：一个小的 Rust 示例程序，经 helper 在双机间建立 mTLS QUIC，开三条流灌流量。测 MTU 黑洞、吞吐、CPU、DERP 计数（方法与已有证据相同；helper 计数的指标名需对照 `tailscale.com` 的固定版本核对）。
3. **直连会话**：`accept_session`、`attach`、`connect_direct`。本机单元测试用进程内 quinn，不经 tailcat，覆盖：重复通道、他会话流、第四条流、超时、mTLS 不匹配。
4. **配对**：R/I、Keychain、SAS。单元测试覆盖：篡改、过期、重放、未知 `req_id`、空白名单拒绝启动。
5. **Tauri 集成**：授权弹窗、指示、路径事件触发 `ReleaseAll`、UI。
6. **双机验收**（见下）。全部通过后才写 CI（Go 单元测试、Rust 单元测试、macOS 构建）。

## 9. 双机通过标准（Mac mini 被控端，MBP 控制端）

**正向**
- 全程用 App 完成配对：AirDrop R → 允许 → AirDrop I → 首连 SAS 确认，全程不手输长串；
- 之后一键重连，被控端弹窗允许；
- MBP 的 Tauri 窗口渲染 Mac mini 的真实桌面，键鼠可用；
- 持续 ≥10 分钟：两端 helper 报告 direct（被控端 `CurAddr` 非空，控制端 `Endpoint` 非空），稳态期间 DERP 应用数据计数不增加（引导期允许）；
- **同一局域网**和**跨网络**（MBP 用手机热点）各做一次，分别记录。

**负向**
- 第三个节点即使拿到 address 也连不上，被控端没有任何弹窗；
- Mac mini 本机进程向回环 QUIC 端口发包，无法建立会话（mTLS 拒绝）；
- 已用过的 I 重放、篡改过的 R/I 被拒绝；
- 被控端点拒绝或超时 → 不出画面；
- 会话中撤销 → 2 s 内停止画面和输入；
- 移除配对后，旧密钥连不上。

**故障**
- 控制端按住 Shift 和鼠标键，然后用 `pfctl` 或断网切断直连：
  - 被控端在 X s 内释放所有按键（X 先记录，目标参考 spec 的 1–1.5 s）；
  - UI 不显示“直连”；
  - 窗口期内经 DERP 的包数记录下来。
- 杀掉 App → helper 1 s 内退出；杀掉 helper → 会话结束并释放按键；睡眠 / 唤醒后不残留会话。

**秘密**
- 在 App 日志、helper stderr、`ps` argv、崩溃日志、剪贴板里，grep 不到 address、PSK、私钥和对端 IP。

**记录**
- 拓扑、DERP 区域、首连耗时（含 PTO）、延迟、码率、两端 CPU、TCC 和本地网络弹窗的行为。
