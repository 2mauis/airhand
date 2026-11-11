# tailcat 临时点对点直连：规格评审

## 结论

**不通过，需修订后复审。** 有 3 个严重问题（S1），编码前必须先改规格：

1. 不变量 I3"媒体永不走共享 DERP"，tailcat 在机制上做不到。
2. 控制流跨通道切换时，输入事件会乱序或丢失。
3. Windows 上被控端以 SYSTEM 身份运行，helper 子进程怎么降权，规格里还没有方案。

另有 9 条 S2：tailcat 版本断言错位、路径检测 API 写错、恶意中转的安全表述过度、版本升级的部署影响等，见下文。

---

## S1 严重（必须在编码前修订）

### S1-1 I3"媒体永不走共享 DERP"无法实现
- **位置**：规格 §2 健康检测、I3、验收项 `derp_only_path_never_carries_media`
- **问题**：tailcat 不提供"禁止 DERP 承载数据"的开关。magicsock 发现直连失效后会**自动**改走 DERP，不会通知调用方。helper 按 1 秒周期轮询路径，Rust 侧再加 3 秒超时，在这段时间里 QUIC 的媒体包、输入包都会经共享 DERP 发出。如果升级刚完成路径就退化，直连 QUIC 的握手和令牌也可能走 DERP。
- **依据**：
  - `tailcat.go:413-563`（`Server` 全部字段）、`tailcat.go:1926-1962`（`Client` 全部字段）里都没有禁用 DERP 数据面的选项。
  - README.md:18-19 写明 DERP 是 "ultimate relay-of-last-resort"；README.md:775-776 写明打洞失败时 "DERP continues as a fallback"。
  - tailcat 自己的 `perf` 也只能"先等直连再开始"，做不到"中途禁止 DERP"（README.md:328-333）。
- **建议修订**：
  - I3 改写为可度量的有界承诺，例如"检测到非直连后 ≤ X ms 内停止向隧道写入媒体"。
  - helper 内部加一道开关：以 ≤250 ms 的周期查询路径（被控端用 `Server.Status()`，控制端用 `DiscoPing`），路径不是 direct 时**直接丢弃**来自回环的数据报，不必等 Rust 侧切换。
  - 在风险里写明窗口期内仍有少量数据会经 DERP。如果要求严格为零，前提只能是自建 DERP。
  - 验收项改名为 `media_stops_within_bound_when_path_degrades`，并补一项用本地 `derper` 做的集成测试（手动或 CI 均可）。

### S1-2 控制流切换会让输入乱序、丢失
- **位置**：§1 第 7、8 步，I6，F9
- **问题**：规格要求控制流的发送端也切到直连，但 `HostMsg` / `ControllerMsg` 都**没有序号**，"按帧序号去重"只适用于画面和声音。
  - **乱序**：切换瞬间，中转上还在途的 `Input` 可能比直连上新发的 `Input` 晚到，造成按键抬起先于按下、鼠标按钮状态反转。
  - **丢失**：回退时直连 QUIC 连接关闭，连接上已写出但未确认的数据会丢掉，可能丢的有 `Input`（导致按键卡住）、`Clipboard`、`Settings`、`Audio{enabled}`、`Bye`。
  - **重复**：发送端本来就不会把同一帧发两次，问题在乱序，不在重复。
- **依据**：
  - `session.rs:26-49` 与 `210-235`：控制消息里没有 seq。
  - `media.rs:18-29`：只有画面和声音的包头带序号。
  - `transport.rs:22-30`：写入只按单条流保证有序。
  - `lib.rs:15` 明确写着"跨流没有顺序保证"。
- **建议修订**（二选一写进规格）：
  - (a) **栅栏协议**：发送端在旧通道上发出的最后一条消息是 `DirectSwitch{epoch}`；接收端读完旧通道、读到这条栅栏之后才处理新通道的消息，等待期间缓存新通道消息，超时上限如 500 ms。回退时同样处理。另外，回退后双方都执行 `ReleaseAll`，控制端重发 `Settings`、`Audio` 的当前状态。
  - (b) Phase 1 **控制流不迁移**，只迁移画面和声音。这样最简单，但输入延迟得不到改善，需要在目标里说明。
  - 补测试：`input_order_preserved_across_switch`、`keys_released_after_fallback`。

### S1-3 Windows SYSTEM 被控端的 helper 降权没有方案
- **位置**：§5 安全模型、风险"Windows SYSTEM 被控端"、开放问题 4
- **问题**：Windows 上的被控端 agent 以 SYSTEM 身份运行，**包括登录界面阶段**（没有可借用的用户令牌）。helper 要解析来自 DERP、disco、WireGuard 的不可信网络输入；tailcat 自己的威胁模型说明它"早期实验性"，历史上还出过"匿名陌生人经 DERP 发包即可触发 panic"的问题。规格把降权留到"实现阶段给出"，等于把安全边界设计推迟到批准之后。
- **依据**：
  - `client/src-tauri/src/winsvc/mod.rs:10-14`：agent = SYSTEM、控制台会话、需要操作 UAC 和登录界面。
  - `winsvc/service.rs:267, 328`：用 `CreateProcessAsUserW` 复制服务自己的 SYSTEM 令牌。
  - tailcat `SECURITY.md:14-19`（威胁模型）、`SECURITY.md:54-58`（经 DERP 可触发的 panic）。
- **建议修订**：批准前必须做 Phase 0 原型，并在规格里写定具体方案。候选方案：
  - `CreateRestrictedToken`（`DISABLE_MAX_PRIVILEGE`，去掉 Administrators / SYSTEM SID）+ 低完整性级别；
  - 或使用专用的虚拟服务账户；
  - Job 对象（禁止创建子进程、`KILL_ON_JOB_CLOSE`、限制内存）。
  
  同时要验证在该令牌下 netmon / magicsock 能否正常绑定 UDP、枚举网卡（目前未证实）。验收里要有可自动化的检查，例如 `helper_token_is_not_system_on_windows`。

---

## S2 重要

### S2-1 tailcat 版本断言错位：v0.7.0 里没有 `AllowClient`
- **位置**：§已核实的 tailcat 事实（标题写"`v0.7.0`"）、feature map 外部依赖表
- **问题**：`AllowClient` 钩子、`KeySet`、`DisconnectClient`、`perf` 都属于 v0.7.0 **之后**的 "Unreleased" 改动。v0.7.0 里对应的是 `Server.AllowedClients` / `AddAllowedClient`。本地调研目录也只有 `main@e00cdde`，没有 tag。
- **依据**：`CHANGELOG.md:3-31`（Unreleased，"Breaking Go API change: AllowedClients … replaced by AllowClient"）；`.git/packed-refs` 里只有 `refs/remotes/origin/main e00cdde`。
- **建议修订**：要么依赖固定到 commit `e00cdde`（并记录 go.sum 哈希），要么等 v0.8.0 发布；表格标题改为"`main@e00cdde`"，逐条重新核对。

### S2-2 "`Client.Ping` 能区分 direct/DERP"不成立；两端检测路径的 API 不同
- **位置**：事实表"是否真正直连"一行，§1 第 5 步，§2
- **问题**：`Client.Ping` 只测量 meow/meowed 经 DERP 的往返，结果里只有 `Latency`。能区分路径的是 `Client.DiscoPing`，CLI 的 `ping --until-direct` 用的也是它。被控端（tailcat Server）**没有** ping，只能轮询 `Server.Status()` 里对应 Peer 的 `CurAddr` / `Relay`。所以两端判断"direct"的依据和时机都不同。
- **依据**：`tailcat.go:2128-2133, 2186-2204`（Ping）；`2256-2303`（DiscoPing，并说明单纯 ping 会一直停在 DERP，要发 TSMP 推动路径发现）；`cmd/tailcat/tailcat.go:896`；`tailcat.go:2613-2617` 与 `CHANGELOG.md:62-64`（Status）。
- **建议修订**：写清楚两端的 helper 分别用什么 API、什么判定条件来报告 `direct`；状态机以被控端为准（见 S2-3）。

### S2-3 切换与回退没有单一决策者，非对称故障会长时间黑屏
- **位置**：§1 第 7、8 步，§2，`DirectActive` / `DirectFallback` 的语义
- **问题**：双方"各自单方面"切换和回退，`DirectFallback` 只用来"宣告"。
  - 假设只有被控端→控制端方向坏了：被控端仍收得到 Ping，于是继续在坏掉的直连上发画面；控制端只能干等，最长到 QUIC 空闲超时 10 秒（`net.rs:112`）。
  - 升级超时同样有竞态：被控端在 8 秒时放弃，控制端在 7.9 秒时连上。
- **建议修订**：
  - 由被控端做最终决定：令牌校验通过后，被控端在直连控制流上发 `DirectCommit{epoch}`，双方收到后才切换。
  - 任一方经中转收到对端的 `DirectFallback{epoch}` 时，**必须**立即回退。
  - 控制端加媒体看门狗：直连上 N ms 没有画面就经中转发 `DirectFallback`。
  - 补测试：`asymmetric_direct_failure_recovers_within_bound`、`late_accept_after_host_timeout_is_ignored`。

### S2-4 "对中转运营者更安全"的表述过度
- **位置**：§5 第 1 条
- **问题**：`addr`（含 PSK）、`token`、`quic_fp`、`client_key` 全部经中转**以明文**转发，因为 TLS 在中转服务器终止。主动作恶的中转只要替换 `DirectOffer` 和 `DirectAccept`，就能让两端都连到自己，在直连路径上做中间人。所以收益只对"诚实但好奇、且不记录"的中转运营者，以及路径上的被动观察者成立。
- **依据**：`docs/security.md:13`；`broker.rs:808-830`（字节透传）；`session.rs` 里新增消息都走控制流。
- **建议修订**：明确写"不提供对中转的端到端机密性；对主动作恶的中转零收益"。考虑到控制码本身由服务器分配和保存，这一限制无法靠带外密钥消除，应在文档里说明。

### S2-5 新增 IP 暴露面，且 Offer 的发送时机没有和"同意"挂钩
- **位置**：§1 第 3 步，§5"隐私"
- **问题**：
  - 现状下控制端拿不到被控端的 IP：`SessionGrant` 只有 `session_id`、`host_name`、`host_os`、`resume`。
  - magicsock 会通告**公网地址和本机所有网卡地址**（局域网、VPN），升级之后控制端能拿到被控端的公网 IP 和内网 IP。
  - 规格没有规定 `DirectOffer` 必须在本机用户同意、发出 `HostHello` 之后才发。如果先于同意，一个还没被同意（或最终被拒绝）的持码人也能拿到这些地址。
- **依据**：`signal.rs:111-116`；tailcat `README.md:769-772`；`session.rs:10-12`（同意之后才发 Hello）。
- **建议修订**：加一条不变量"`DirectOffer` 只在同意之后、`HostHello` 之后发送"，补测试 `no_offer_before_approval`；隐私说明里补充"对端会获知本机公网和局域网地址"。

### S2-6 协议升到 v10 的部署影响，与 I1 和"服务器不改"相矛盾
- **位置**：I1、§3 第 1 条、非目标第 1 条、回归项 `server_unchanged`
- **问题**：
  - 服务器直接使用 `airhand_proto::PROTOCOL_VERSION`，并且要求版本**严格相等**。升到 10 之后服务器必须重新构建、部署，所有 v9 客户端（包括 Android 和网页控制台）都会被拒绝。"`server/` 无 diff"字面上成立，但"不修改中转服务器与 Docker 部署"不成立：镜像必须重建。
  - `HostHello` / `ControllerHello` 加字段会改变 postcard 编码，Hello 里的 version 字节也会变，所以 I1"关闭时与 v9 字节级一致"不可能成立。
  - `android/native` 与 `web/` 要跟着改代码（构造新字段、穷举匹配新变体），不只是"cargo test 仍通过"。
- **依据**：`broker.rs:25, 300-305`；`lib.rs:30-33`；`session.rs:124-139, 237-242`。
- **建议修订**：
  - I1 改为"关闭时：不发新消息、不起子进程、不联系 tailcat/DERP；线上消息与 v10 关闭态一致"。
  - 增加发布计划：服务器与全部客户端同时升级，并说明如何回滚。
  - 非目标改为"服务器源码不改，但需要用 v10 重新构建和部署"。

### S2-7 I8 与"可扩展 `kind`"在 postcard 下不提供前向兼容
- **位置**：§3"可扩展枚举（kind）"、§3 末段、I8、验收项 `unknown_future_control_variant_*`
- **问题**：
  - **airhand 现状**：控制流上**任何**解码错误都会结束会话。被控端读任务里是 `read_msg(...).await?`；控制端读任务把 `Err` 转交出去，`recv` 返回错误，会话随之失败。
  - **postcard 行为**：postcard 不自描述，遇到未知变体下标时，serde derive 会报 invalid value。这一点本地没有 postcard 源码，依据是 serde/postcard 的公开行为，标为"按已知行为判断"。
  - **对本次的影响**：服务器按版本号把关，v9 和 v10 不会在同一会话里相遇，所以 I8 对这次改动基本无关。
  - **对后续阶段的影响**：Phase 2 往 `kind` 里加 `Udp` 变体时，同为 v10 的旧对端会直接断开会话，所谓"预留扩展"并不成立。
  - **Kotlin / TypeScript**：它们根本不解析 postcard，只解析 Rust 转出来的 JSON 事件和媒体包，对未知事件本来就会忽略（`client/src/lib/session.ts:389` 的 switch 没有 default；`RemoteSession.kt:358` 的 `when` 没有 else）。规格里"需要它们忽略未知变体"的说法对象搞错了。
- **依据**：`host.rs:305-306`；`transport.rs:150-166`；`framing.rs:45-47`。
- **建议修订**：offer 负载设计成 `kind: u8` + `#[serde(with="serde_bytes")] body`，接收端遇到不认识的 kind 就回 `DirectDecline`；或者明确"新增 kind 必须升版本号"。I8 改写为对上述现状的结论，删掉对 Kotlin/TS 的要求。

### S2-8 tailcat Server 默认放行范围比规格假设的宽；`AllowClient` 的设置时机与流程冲突
- **位置**：§5"最小暴露面""双重限定"，I9，F5
- **问题**：
  - **TCP 默认放行**：`ServedTCPPorts` 为 nil 时，包过滤对本机地址放行**全部 TCP 端口**，再由 `OnTCP == nil` 回 RST。这会暴露"这里有一个 tailcat 服务"。
  - **UDP 默认放行**：`ServedUDPPorts` 为 nil 时，全部 UDP 端口都会交给 `OnUDP` 判断。
  - **设置时机**：`AllowClient` "must be set before calling Start"；但按规格流程，控制端公钥要到 Start（拿到地址）**之后**才收到。
  - **已接入的客户端**：`AllowClient` 只在接入时问一次，之后不会再问。
- **依据**：`tailcat.go:474`；`536-557`；`711-713`；`816-852`；`544-545`（被过滤的包不回 RST）；`465-466`。
- **建议修订**：
  - 规格明确写死以下配置：`ServedTCPPorts = []filter.PortRange{}`（空但非 nil）；`ServedUDPPorts = {p}`；`OnTCP`、`OnTCPForward`、`OnUDPForward`、`AllowProxy` 全部为 nil；Start 之前设置 `AllowClient = keyset.Contains`，收到公钥后再往 keyset 里加；踢人或会话结束时调用 `DisconnectClient` 并 `Close`。
  - 或者把流程改为控制端**先**发公钥：`Client.PublicKey()` 可以在建连之前取得（`tailcat.go:2097-2103`）。
  - `TestNoTCPListeners` 应断言"不回 RST、SYN 超时"。

### S2-9 打包与平台需求遗漏
- **位置**：F12，风险
- **问题**：规格没有覆盖以下事项：
  - Windows Defender 防火墙对 helper 绑定 UDP 的处理（是否弹窗，或是否需要安装器加规则）；
  - macOS 嵌套 helper 的代码签名、公证、hardened runtime，以及应用防火墙弹窗；
  - Go/WireGuard 二进制常被杀毒软件误报；
  - 卸载流程（`client/src-tauri/src/uninstall.rs`）要能清理 helper；
  - Linux 各分发形态如何带上 helper。
- **建议修订**：作为 F12 的子项，各自配上验收。

---

## S3 一般

| 编号 | 位置 | 问题 | 依据 | 建议 |
|---|---|---|---|---|
| S3-1 | §4 `helper_path` | helper 路径可配置，被控端就会执行配置文件指定的程序；"存在且为普通文件"这项校验不够 | `config.rs:15-16`（Windows 上只有 SYSTEM/管理员可写，所以不构成提权，但扩大了攻击面） | 去掉这个配置项，固定为程序同目录下的文件，并校验构建时记录的哈希或签名 |
| S3-2 | §1 第 6 步，I9 | 回环 UDP 本机任何用户都能访问（包括 Windows 的其他会话 / RDP 用户）。控制端 helper 的回环收包口如果采用"锁定第一个发包来源"，会被本机其他进程抢先占用（升级被 DoS）。被控端 QUIC 监听也接受任意本机来源 | `net.rs:177-182`（现有绑定方式） | 首选：给 quinn 实现自定义 `AsyncUdpSocket`，经 helper 的管道或 stdio 收发数据报，完全不开端口。退而求其次：Rust 经 stdin 把 quinn 的确切端口告诉 helper，helper 只收发这个 4 元组；被控端监听只接受 1 个连接，接受后立即关闭；直连使用独立的 ALPN |
| S3-3 | 方案 C 否决理由，Phase 1.5 Android | 规格没有评估 tailcat 官方提供的 C ABI `cmd/libtailcat`。它支持 UDP、`disco_ping`、`allowed_clients`，并且"UDP writes above 1232 bytes are rejected"；Android 未必需要 gomobile | `cmd/libtailcat/README.md:9-12, 63-66, 109-133, 140-142` | 把它列为方案 C′ 写明取舍。桌面上 helper 仍然合理（崩溃隔离、便于降权）；Android 可考虑走 JNI。注意 libtailcat 的 `allowed_clients` 是创建时固定的，需要"公钥先行"的流程 |
| S3-4 | §1 | 被控端支持同时多个会话，规格没有说明：一个会话一个 helper，还是一个 helper 跑多个 Server？每个 Server 都有独立的 magicsock 和 DERP 连接，资源上限也没定 | `host.rs:158-171` | 写明"每会话一个 Server"和并发上限；补测试 `two_concurrent_sessions_upgrade_independently` |
| S3-5 | §1 第 7 步"epoch" | 24 字节媒体头里没有 epoch 字段，接收端无法按 epoch 过滤包，只能看包从哪条通道来 | `media.rs:18-29` | 规则写成：新通道的首个关键帧到达后，丢弃旧通道的画面；声音丢弃 seq ≤ 已交付的包。另外 Ack 是累积确认（`flow.rs:40-50`），丢掉的帧不会卡住在途窗口，可以写进规格作为依据 |
| S3-6 | F9 | 会话生命周期耦合：被控端三条写任务用 `try_join!` 绑在一起，任一失败整个会话结束；单条写入卡住 10 秒即判失败；中转侧任一条流关闭就拆掉整个会话。热备通道永远不能 `finish`。中转连接一断，直连会话也会结束 | `host.rs:294-300`；`transport.rs:22-30`；`broker.rs:789-790`；`host.rs:174-177, 209` | 规格写明：直连流失败只触发回退，不结束会话；中转断开即会话结束（与 I10 一致） |
| S3-7 | §2 重试 | 每次重新升级是新建 tailcat Server（新地址、新 PSK），还是复用原来的？这与"每个会话一次性"的说法冲突 | — | 写明每个 epoch 都新建 Server 和令牌，并关闭上一个 |
| S3-8 | 验收清单 | 有几项名不符实或做不到： | — | 见下面各条 |

S3-8 具体各项：

- `direct_session_requires_same_approval_*`、`kick_all_*`、`new_code_*` 放在 `airhand-link/tests`，但"同意"和托盘逻辑在 `client/src-tauri/src/host/session.rs` 与 `tray.rs`，从 link 层测不到。
- `direct_quic_payload_capped_to_p2p_mtu`：据我所知 quinn 的 `TransportConfig` 没有公开 getter（本地没有 crate 源码，未证实）。应改成行为测试：假 helper 丢弃超过 1232 字节的数据报，断言观测到的最大数据报 ≤ 1232。
- `version_handshake_wire_is_stable` 用的是字面量 version 7（`framing.rs:285-311`），并不断言 `PROTOCOL_VERSION == 10`，需要另加断言。
- `direct_config_has_no_shared_derp_media_override` 测的是"某个键不存在"，意义不大。
- 缺少的测试：S1-2、S2-3、S2-5、S3-4 里提到的各项，"升级过程中被踢 / 更换控制码"的竞态，以及 helper 崩溃时会话不中断。

---

## S4 建议

- **背景措辞**：背景第 18 行"DERP 仅用于引导"不准确，DERP 也是兜底的数据通道（事实表里已经写对，应统一）。
- **CLI 的 UDP 用法不全**：CLI 的 UDP 还出现在 `perf --udp`（main 分支，未发布，`cmd/tailcat/perf.go:51, 270`），不只 exit-node 和 socks。
- **I11 措辞**：tailcat README 说的是超过 1232 字节"not guaranteed to reach"，不是"会被丢弃"。Go 库实际是丢弃还是分片，本地未证实。
- **quinn 直连配置**：直连用独立的 `Endpoint`，再加一个 `direct_transport_config()`：
  - `initial_mtu(1200..=1232)`，默认就是 1200；
  - `min_mtu(1200)`；
  - `mtu_discovery_config(None)`；
  - `EndpointConfig::max_udp_payload_size(1232)`。

  这样不影响中转用的 `transport_config()`（`net.rs:107-119`）。以上 API 名称来自 quinn 0.11 的公开 API，本地没有源码，未逐行核对。
- **双向认证**：可以考虑用 mTLS 代替"只出示令牌"：`DirectAccept` 带上控制端临时证书的指纹，令牌作为第二道因子。
- **依赖**：`rcgen` 目前只有 `server` / `web` 依赖，`airhand-link` 或 `client` 需要新增。
- **I7 的重要性**：`HostMsg` 派生了 `Debug`（`session.rs:25`），而 Windows 的日志写到 `OutputDebugString`（`winsvc/mod.rs:21`），本机任何用户都能用 DebugView 读到。I7 因此尤其重要，建议用一个对 `Debug` / `Display` 脱敏的 `Secret<T>` 新类型。
- **超时与阶段顺序**：
  - `upgrade_timeout_secs = 8` 比 tailcat `Ping` 内部的 10 秒超时还短（`tailcat.go:2192-2193`），还要算上拉取 DERP 地图的时间，需要实测后再定默认值。
  - Phase 0 的实测（性能、Windows 令牌、防火墙）应在 Phase 3 批准**之前**完成，而不是之后。

---

## 已核实无误的关键断言

- 两端都主动连出到中转；客户端打开的第一条双向流是信令流；会话三条流按会话 ID 配对后透传；证书指纹固定（`lib.rs:5-23`，`broker.rs:722-830`，`net.rs:224-255`）。
- 媒体包头 24 字节，`protocol.json` 位于 `crates/airhand-link/tests/`（`media.rs:35`）。
- 文档已说明"中转能看到会话内容"（`docs/security.md:13`）。
- `version_handshake_wire_is_stable` 存在，并锁定了 Hello / Welcome / Incompatible 与 ALPN 的编码（`framing.rs:283-312`）。
- 当前 `PROTOCOL_VERSION = 9`、`ALPN = airhand/1`（`lib.rs:33, 36`）。
- Windows 被控端（agent）以 SYSTEM 身份运行（`winsvc/mod.rs:10`）。
- tailcat 的 LICENSE 是 BSD-3-Clause（`LICENSE:1-28`）；`go.mod` 要求 go 1.27.2，这是 main 分支的情况。
- `MaxUDPPayload = 1232`（`tailcat.go:574-578`）；有 `OnUDP`、`DialUDPPort`、`ConnPacketConn`。
- CLI 的 `serve` / `forward` 只转发 TCP，地址作为命令行参数传入。
- `AllowClient` 钩子存在（main 分支）。
- `Client.PublicKey()` 可以在建连之前取得。
- 用 `OnUDP` + `ServedUDPPorts` 可以做到只服务一个 UDP 端口。
- 默认是临时密钥；地址里含 PSK，是不记名凭据。
- 默认 DERP 地图为 tailcat.dev，无 SLA、可随时撤销；可以自建 DERP（README）。
- tailcat 的威胁模型明确写着"早期实验性"（`SECURITY.md:14-19`）。
- 没有稳定性承诺（README "Stability" 一节）。
- 调研时 main 分支为 `e00cdde`。

## 未能证实的断言

- v0.7.0 tag 的实际内容，以及它对应的 Go 版本：本地没有 tag，CHANGELOG 显示有几项与规格描述不一致。
- postcard 对未知变体报的具体错误类型：本地没有源码；但不论报哪种错，airhand 都会结束会话，这一点已核实。
- quinn 0.11 各配置项的确切名称与默认值：本地没有源码。
- Go 库收到超过 1232 字节的写入时，是丢弃、报错还是分片。
- `DiscoPing` 返回 direct 是否意味着 WireGuard 数据面也走直连；magicsock 重新确认路径期间是否会同时经 DERP 双发。
- Windows 受限令牌下 helper 的网络与网卡枚举是否可用。
- 经用户态 WireGuard 和 gVisor 之后的 CPU 开销与吞吐上限。
- `libtailcat` 是否包含在 v0.7.0 里。
- 传递依赖的完整许可清单（规格本身也列为待审计）。