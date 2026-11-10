# tailcat 临时点对点直连（Direct P2P transport）

> 状态：**规格 r3.1（已按第 1、2 轮意见修订；第 3 轮复审结论：无 S1，可提交批准 (A)；第 3 轮的 S2/S3 列入下文“r4 待办”），未实现**。本文件只描述设计；不含功能代码。评审记录见 [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（三轮报告全文在 [`review/`](review/)）。
> 基线：上游 `sqlpxc/airhand` `main` @ `a317551`（协议版本 v9，工作区版本 0.2.2，MIT）。
> 集成对象：`https://github.com/tailscale/tailcat`，**固定到 tag `v0.7.0`（commit `15ab9e6`；`546a07d` 是该附注 tag 对象的哈希，不是 commit）**，`go.mod` 要求 Go 1.27.1，BSD-3-Clause。`main`（调研时 `e00cdde`）上的未发布改动（`AllowClient` 钩子、`perf`、`libtailcat`）**不作为依赖**。

## 背景/目标

现状（见 [architecture](../architecture.md)、`crates/airhand-link/src/net.rs`、`server/src/broker.rs`）：

- 被控端与控制端都**主动连出**到自建中转服务器（QUIC/UDP 4433，证书指纹固定）。
- 信令流（第一条双向流）完成注册 / 配对；每个会话的控制 / 画面 / 声音三条流由服务器按会话 ID 配对后**零拷贝透传**。
- 因此画面、声音、输入的全部流量都经过中转服务器：延迟多一跳、占中转带宽、服务器能看到会话内容（`docs/security.md` 已说明）。

目标：在**不改变使用方式**（仍用 6 位控制码连接）的前提下，让两端在条件允许时建立**临时的点对点 UDP 直连**，
媒体与输入改走直连，中转服务器只保留**信令 / 配对 / 兜底**职责，从而降低对中转的带宽与信任依赖。

选用 tailcat 作为 NAT 穿透与加密隧道：它复用 Tailscale 的数据面（userspace WireGuard + magicsock 打洞；DERP 用于引导，也是打洞失败时的兜底数据通道），
**不需要 Tailscale 账号、不需要 root / 管理员、不改路由与 DNS**；连接元数据由调用方带外交换（这里由 airhand 的会话控制流承载）。DERP 在 tailcat 里既是引导通道、也是打洞失败时的兜底数据通道。

### 已核实的 tailcat 事实

核对范围：下表凡未标注者均已对 `v0.7.0` 源码核对（`Server.AllowedClients` / `AddAllowedClient`、`ServedTCPPorts` / `ServedUDPPorts`、`MaxUDPPayload = 1232`、`Client.DiscoPing`、`Server.Status`、`Client.PublicKey`）；标注 `main` 的条目不属于 `v0.7.0`，不得依赖；**未证实（v0.7.0）**：`Client.Ping` 内部超时、超 1232 字节写入的库内行为、magicsock 何时开始经 DERP 双发。

| 项目 | 结论 |
|---|---|
| LICENSE | BSD-3-Clause（允许使用、修改、再分发二进制与源码；须保留版权声明与免责声明，二进制再分发须在文档 / 材料中复现声明；不得借用贡献者名义背书）。传递依赖（`tailscale.com` BSD-3、`wireguard-go` MIT、gVisor Apache-2.0 等）须在实现阶段做完整许可审计并写入第三方声明 |
| 形态 | Go 库（`github.com/tailscale/tailcat`）+ CLI（`cmd/tailcat`）。`go.mod` 要求 Go 1.27.x |
| 机制 | 服务端得到一个 `tc…` 地址（含服务端 WireGuard 公钥、独立的路径发现公钥、默认的 256 bit 预共享密钥、DERP 区域）；客户端凭地址经 DERP 发 “Meow” 引导，随后 magicsock 用 STUN + disco 打洞，**成功则升级为直连 UDP**，失败则继续走 DERP 兜底 |
| 是否需账号 | 否（无登录、无控制面） |
| 是否真正直连 | 通常是（“usually”）；双方都在困难 NAT 且无端口映射时只能走 DERP。**路径检测 API 两端不同**：客户端用 `Client.DiscoPing`（`tailcat ping --until-direct` 用的就是它；`Client.Ping` 只测 DERP 往返，不能区分路径）；服务端没有 ping，只能轮询 `Server.Status()` 里对应对端的当前地址 / 中继字段 |
| DERP | 默认使用 Tailscale 托管的**限速**免费 DERP（`https://tailcat.dev/derpmap.json`）；官方声明**无 SLA、可随时撤销**；可自建 `derper` 并把主机名嵌入地址 |
| 安全属性 | 默认地址是**不记名凭据**（含 PSK）；`v0.7.0` 用 `Server.AllowedClients`（`Start` 之前给定的公钥列表，对应 CLI `--allow`）限制可连接的客户端节点公钥；另有运行时的 `AddAllowedClient`；**列表为空时放行所有客户端**，`AddAllowedClient` 在加入第一个公钥之前同样放行所有人（所以本设计在 `Start` 时即给定非空列表，并在公钥无效 / 为空时拒绝 Start）；`AllowClient` 钩子仅在 `main`。威胁模型文档明确“早期实验性、原本面向同一人的两端”，对互不信任双方的使用仍在加固，历史上出过经 DERP 可触发的崩溃 |
| 包过滤默认值 | `Server.ServedTCPPorts == nil` 时对本机地址放行**全部 TCP 端口**（再由 `OnTCP == nil` 回 RST）；`ServedUDPPorts == nil` 时全部 UDP 端口交给 `OnUDP` 判断。**必须显式设置为空非 nil 的 TCP 范围与单端口 UDP 范围**（见“安全模型”） |
| 不可禁止 DERP 承载数据 | `Server` / `Client` 没有“禁止 DERP 数据面”的选项；magicsock 路径退化时自动改走 DERP 且不通知调用方 |
| UDP | 库提供 `OnUDP` / `DialUDPPort`（`ConnPacketConn`），**单包载荷上限 `MaxUDPPayload = 1232` 字节**（README：超过“不保证送达”，库内是丢弃 / 报错 / 分片**未证实**，Phase 0 实测）；**CLI 的 `serve` / `forward` 只转发 TCP**（UDP 只在 `exit-node`、socks UDP ASSOCIATE、`main` 上未发布的 `perf` 里出现） |
| 稳定性 | README 明确：Go API、CLI 参数、线格式**都可能变化**，无稳定性承诺。`main` 上还有 `cmd/libtailcat`（C ABI，支持 UDP / 禁用路径探测 / 固定允许列表，**不在 `v0.7.0`**），见下“方案 C′” |

### 集成形态（本规格的设计选择，待评审确认）

airhand 的传输是 QUIC（UDP）。只用 tailcat CLI 只能得到 TCP 转发，等于要把三条流改成 TCP 多路复用，
放弃 QUIC 的分流队列、BBR 与无队头阻塞，不可接受；而把 Go 运行时 cgo 嵌进 Tauri / Android 进程构建与体积代价过大。
因此选择：

- **方案 B（推荐）**：仓库内新增一个很小的 Go 模块 `p2p/`，产出独立可执行文件 `airhand-p2p`（“helper”），
  以 **tailcat 库（固定到某个已核对的 tag/commit）** 为依赖；Rust 侧以**子进程**方式拉起，通过 **stdin/stdout JSON 行**控制。
  helper 只做一件事：把 tailcat 隧道里的**一个 UDP 端口**与本机**回环 UDP 套接字**之间做数据报转发。
  QUIC 仍然是 Rust 的 quinn；helper 不解析 QUIC。Rust 与 helper 之间的数据报通道有两个候选，**由 Phase 0 实测后二选一**：
  - (i) 回环 UDP：helper 与 quinn 各绑定回环随机端口，经 stdin 互告确切 4 元组，helper 只收发该 4 元组；被控端直连监听只接受 1 个连接，接受后立即关闭，并使用独立 ALPN；Windows 上两端都用 `SO_EXCLUSIVEADDRUSE` 绑定，防止同机其他进程抢绑同一回环端口；缺点：本机其他用户 / 会话可探测该回环端口（只能靠令牌与 ALPN 挡住，存在升级被抢占的 DoS 面）。
  - (ii，首选，若吞吐 / CPU 达标)：为 quinn 实现自定义 `AsyncUdpSocket`，数据报经 helper 的**独立管道**（Unix 用额外继承的 fd；Windows 用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 继承额外句柄，带长度前缀）收发，**完全不开本机端口**；stdin / stdout 只用于 JSON 行控制协议，不混传数据报。
- 方案 A（否决）：直接子进程调用 `tailcat` CLI —— 只有 TCP，且地址会出现在命令行参数（`ps` 可见）。
- 方案 C（否决，桌面）：cgo / `c-shared` 把 tailcat 嵌入 Rust —— 体积、交叉编译与崩溃隔离问题；helper 子进程还便于降权与崩溃隔离。
- 方案 C′（Phase 1.5 候选，Android）：`main` 上的官方 C ABI `libtailcat`（不在 `v0.7.0`，其允许列表在创建时固定，同样要求“公钥先行”流程）；是否采用留到 Android 阶段另行评审。
- 方案 D（否决）：用 Rust 重写 magicsock / disco / WireGuard 数据面。

方案 B 的代价与前提（Phase 0 验证项，见下文）：QUIC 数据报必须 ≤ 1232 字节；多一层用户态 WireGuard + gVisor 的 CPU 与延迟需实测。

## 总体设计

### 1. 会话先走中转，再就地升级到直连（make-before-break，被控端裁决）

1. 连接流程不变：控制端用控制码经中转建立会话，三条流经服务器配对，**画面立即可用**。
2. 双方在 `Hello` 里声明是否启用直连能力。仅当**双方都启用**时才进入升级流程；否则与未启用时完全一致。
3. **公钥先行**（`v0.7.0` 的 `AllowedClients` 为空时**放行所有客户端**，`AddAllowedClient` 在加入第一个公钥之前同样如此，所以必须在 `Start` 时就给定非空列表）：控制端先创建自己的 helper dial 实例（本 `epoch` 新的客户端密钥），用 `Client.PublicKey()`（拨号前即可取得）取得客户端节点公钥，经控制流发 `ControllerMsg::DirectRequest { epoch, client_key }`。`client_key` 为空或无法解析时，Rust 侧与 helper 都**拒绝 Start**（I14）。
4. 被控端只在 **`HostHello` 已发出之后**才处理 `DirectRequest`（此前收到的一律丢弃，I12）。**桌面被控端没有“请本机用户同意”这一步**（持码人连上后立刻收到 `Hello`），所以 Phase 1 里该条件即“`HostHello` 之后”；在有同意流程的平台（Android，Phase 1.5）还须在同意之后。被控端为该 `epoch` 在 helper 进程内**新建**一个 tailcat `Server`（新的密钥与 PSK，`AllowedClients = [client_key]`），并生成一次性令牌与该会话的临时自签证书；经控制流回 `HostMsg::DirectOffer { epoch, kind, body, quic_fp, token, ttl }`。
5. 控制端 helper 以 offer 中的地址拨号，并按 §2 的规则检测路径；**仅当路径为 `direct`** 才继续。超时（默认值由 Phase 0 实测后确定，不小于 12 秒，需覆盖 DERP 图拉取与引导）或只有 DERP 则放弃，会话继续走中转，不报错。
6. 控制端经 helper 的数据报通道用 quinn 连接被控端的**直连 QUIC 监听**，出示一次性令牌并校验 `quic_fp`；被控端校验令牌 / 会话 / `epoch` / 有效期通过后，等待控制端在**直连连接**上开三条流（`SessionHeader` 与中转上相同）。
7. **提交（commit）由被控端裁决，顺序固定**：三条直连流就绪后，被控端 **先在中转控制流上发 `DirectSwitch { epoch }`（被控端→控制端方向的栅栏）**，**再**把 `DirectCommit { epoch }` 作为**直连控制流的第一条消息**发出。被控端若在自身超时前没有收到完整的三条流，则放弃该 `epoch`（迟到的 `DirectRequest` / 连接一律忽略，I13）。**控制端打开三条流后设 T（默认 2 秒，Phase 0 校准）超时：收不到 `DirectCommit` 就经中转发 `DirectFallback { epoch }`**；被控端对已提交的 `epoch` 也必须接受它。
8. **切换规则**：
   - **画面 / 声音**：被控端提交后把发送端切到直连流并立即发一个关键帧；接收端**始终同时读取中转流与直连流**，新通道的首个关键帧到达后丢弃旧通道的画面包（按帧序号），声音丢弃序号 ≤ 已交付序号的包（24 字节媒体头没有 epoch 字段，所以规则以“来自哪条通道 + 序号”表达）。
   - **控制流（升级方向，栅栏）**：控制消息没有序号、跨流无顺序保证。每个发送端在**旧通道（中转）**上发出的**最后一条**消息是 `DirectSwitch { epoch }`，之后才在直连上发新消息；接收端读到该栅栏之前只处理旧通道，新通道的消息先缓存，读到栅栏后再处理缓存。控制端在收到 `DirectCommit` 后用同样方式切换自己的发送端。`Ack` / `Ping` / `Pong` 与顺序无关，缓存期间照常处理（避免在途窗口停住）。**栅栏等待上限**不写死 500 ms，由 Phase 0 按中转 RTT 校准（可配置或自适应）；**超时时丢弃缓存的直连控制消息并执行与回退相同的流程**（丢失由状态重同步补偿），不照常处理（会乱序）。
   - **终止消息**：`Bye` 一律经**中转**控制流发送（中转是会话生命周期的权威，I10），可同时在直连上多发一份；不存在“回退后重发 `Bye`”。
9. **中转流保持打开作为热备**（空闲，不发媒体，不 `finish`）。

### 2. 路径检测、存活检测与回退（对 DERP 的有界承诺）

**路径检测规则（helper 内的“闸门”）**：tailcat 无法禁止 DERP 承载数据，所以规格承诺**有界**而非“零”（I3）。闸门关闭时 helper 立即丢弃来自 Rust 一侧的数据报，并报告 `path = derp|none`。
- 控制端（`Client` 没有 `Status`，只能用 `DiscoPing`）：pong 经 DERP 返回 ⇒ 立即判非 direct；连续 M 次（默认 2）在 `max(750 ms, 3×RTT)` 内没有收到 direct pong ⇒ 判非 direct（避免跨洲 RTT 被误判）。
- 被控端：轮询 `Server.Status()` 中该对端的当前地址 / 中继字段，当前地址为空 ⇒ 判非 direct。
- 升级前双方的闸门**各自**关闭，直到本端检测为 direct 才打开；直连 QUIC 握手在升级超时内可重试，容忍两端判定时间差。
- 窗口期（从路径退化到闸门关闭）内仍可能有少量数据报经共享 DERP 发出；严格为零的前提只能是**自建 DERP**。magicsock 在直连信任期内是否会双发 DERP、何时开始双发，列为 Phase 0 实测项（未证实）。

**存活检测（不依赖画面）**：桌面被控端只在画面变化时编码、无声音不发包，静止桌面没有帧也没有 Ack，因此**不能**用画面包或 Ack 判存活。改用直连控制流上的 Ping/Pong：控制端本来就每秒发一次 `Ping`，被控端回 `Pong`；控制端在直连上 K 秒（默认 2，须小于现有的 2 秒命令队列过期与写入失败判定，Phase 0 校准）收不到 `Pong` ⇒ 经中转发 `DirectFallback`；被控端在直连上 K 秒收不到 `Ping` ⇒ 回退。该规则同时覆盖两个方向的非对称故障。直连 QUIC 连接关闭 / 空闲同样视为失效。

**回退（反向栅栏）**：
- 失效侧立即把自己的发送端切回中转流并请求关键帧，并且它在**中转**控制流上发出的**第一条**消息必须是 `DirectFallback { epoch }`。
- 对端无论从哪里得知（收到 `DirectFallback`、或自己检测到故障）都**立即回退**；收到 `DirectFallback` 的一方此后**丢弃**该 `epoch` 的所有直连控制消息（含已缓存的），只处理中转上的消息。
- **状态重同步**：被控端在本地调用 `release_all()` 释放按键；控制端清空本地记录的按键 / 鼠标按钮状态（不需要再发 `ReleaseAll` 输入事件）；控制端重发当前的 `Settings`、`Audio` 启停与选中的显示器；剪贴板以最新状态为准。
- **写入失败不结束会话**：直连通道的写入失败 / 卡住（含现有 2 秒命令队列过期、写任务失败判定）一律转为回退并把排队消息改投中转；**直连通道失败只触发回退，绝不结束会话**；仅中转连接断开才结束会话（与 I10 一致）。helper 进程退出 / 崩溃视同直连失效，由 Rust 侧监督，不自动无限重启。
- 每次重新升级都使用**新的 `epoch`、新的 tailcat 密钥 / 地址 / 令牌**（旧的立即销毁）；回退后每隔 30 秒、最多 3 次重试。
- 并发与进程模型：**每个会话一个 helper 进程；每个 `epoch` 在进程内新建 `Server`（控制端新建 `Client` 与密钥）并 `Close` 旧的**。同一进程内多个 `Server` 是否安全（例如进程级全局设置）**未证实**，Phase 0 验证；不安全则改为每个 `epoch` 一个进程。并发上限默认 2（可配，不超过 `AIRHAND_MAX_SESSIONS`），超出的会话保持中转。

### 3. 协议改动（`PROTOCOL_VERSION` 9 → 10）

- **服务器源码不改，但必须用 v10 重新构建并部署**：服务器直接引用 `airhand_proto::PROTOCOL_VERSION` 并要求**严格相等**，升级后所有 v9 客户端（桌面、Android、网页控制台）都会被拒绝；见“发布与回滚”。
- `ClientMsg::Hello` / `ServerMsg::Welcome` / `Incompatible` 与 ALPN **编码不得变化**（`version_handshake_wire_is_stable` 保护；该测试用字面量版本号，需另加对 `PROTOCOL_VERSION == 10` 的断言）。
- `crates/airhand-proto/src/session.rs`：在 `HostMsg` / `ControllerMsg` 枚举**末尾**追加变体；`HostHello` / `ControllerHello` 增加能力位 `direct: bool`（这会改变 Hello 的 postcard 编码——因此 v10 与 v9 本来就不互通）。下列为占位示意，字段细节在实现阶段定稿：
  - `ControllerMsg::DirectRequest { epoch: u32, client_key: String /* nodekey:… */ }`
  - `HostMsg::DirectOffer { epoch: u32, kind: u8, body: Vec<u8> /* kind=1：tailcat 地址，不记名凭据 */, quic_fp: [u8; 32], token: [u8; 16], ttl_secs: u16 }`
  - `HostMsg::DirectDecline { epoch, reason }` / `ControllerMsg::DirectDecline { epoch, reason }`
  - `HostMsg::DirectCommit { epoch }`（直连控制流上的第一条消息；之前先在中转上发 `DirectSwitch`）
  - `HostMsg::DirectSwitch { epoch }` / `ControllerMsg::DirectSwitch { epoch }`（升级方向：旧通道上的最后一条消息）
  - `HostMsg::DirectFallback { epoch }` / `ControllerMsg::DirectFallback { epoch }`（回退方向：中转上的第一条消息）
- 前向兼容的实际情况（已对照源码）：控制流上**任何**解码错误都会结束会话，postcard 对未知枚举变体会报错，所以“预留扩展”**不**提供前向兼容；`DirectOffer` 因此用 `kind: u8` + 不透明 `body`，接收端遇到不认识的 `kind` 回 `DirectDecline`，不依赖解码器容忍未知变体。Kotlin / TypeScript 不解析 postcard，只处理 Rust 转出的 JSON 事件与媒体包，本特性 Phase 1 不改它们的行为；但 `android/native`、`web/` 的 Rust 代码要为新字段 / 新变体通过编译。
- 媒体包头（24 字节）与 `tests/protocol.json` 样例**不变**。
- 新增依赖：直连临时证书需要 `rcgen`（目前仅 `server` / `web` 依赖），加到需要的 crate。

### 发布与回滚

- 上线顺序：用 v10 重新构建中转服务器镜像与全部客户端，**同时**部署 / 分发（v9 与 v10 互不相通，这与现状“同版本才可用”一致）。
- 未启用直连（默认）时：不发 `Direct*` 消息、不起子进程、不联系 tailcat / DERP。
- 回滚：关闭 `[direct] enabled` 即可停用功能；回退到 v9 需整体回滚服务器与客户端。

### 4. 配置（默认关闭）

`config.toml` 新增 `[direct]`（桌面；网页控制台与 Android 为后续阶段）：

| 键 | 默认 | 说明 |
|---|---|---|
| `enabled` | `false` | 总开关；关闭时**不会启动 helper、不会联系任何 tailcat / DERP 主机** |
| `derp_map_url` | tailcat 默认 | 必须 `https://`；可指向自建 DERP 图；自建 DERP 时主机名可嵌入地址，不再访问 Tailscale 托管服务 |
| `upgrade_timeout_secs` | 由 Phase 0 实测定，≥ 12 | 等待直连路径的时间 |
| 其它阈值 | 由 Phase 0 实测定 | 栅栏等待上限、Commit 等待 T、存活检测 K、路径判定的 M / RTT 系数（均须小于 2 秒的现有命令队列过期） |
| `max_sessions` | `2` | 同时升级的会话数上限 |

helper 可执行文件的路径**不可配置**：固定为主程序同目录下的文件，启动前校验构建时记录的哈希（或签名），不一致则不启用直连。该校验只防误放 / 损坏文件；在用户可写的安装位置（如 AppImage、用户目录安装）防不了同一用户下的攻击者（对方本来就能替换主程序），文档如实说明。

### 5. 安全模型

- **信任假设（如实表述）**：地址（含 PSK）、令牌、`quic_fp`、`client_key` 经中转的控制流**明文**转发（TLS 在中转终止）。**本特性不提供对中转的端到端保密：主动作恶的中转可以替换 `DirectOffer` / `DirectRequest`，让两端连到自己并在直连路径上做中间人**，与今天中转可读写会话内容等价；控制码由服务器分配，该限制无法靠带外密钥消除，需在 `docs/security.md` 说明。收益仅限：诚实但好奇的中转运营者（升级完成后媒体与输入不再经过它）与路径上的被动观察者。
- tailcat 地址是不记名凭据 → **每个会话、每个 `epoch` 一次性**：新建、用完即毁；永不使用 `tailcat genkey` 的保存密钥；地址只经会话控制流下发，**不写日志、不进 `config.toml`、不放命令行参数、不进崩溃报告**；在 Rust 里用对 `Debug` / `Display` 脱敏的 `Secret<T>` 包装（`HostMsg` 派生 `Debug`，Windows 日志写入 `OutputDebugString`，本机其他用户可读，所以 I7 尤其重要）。
- **双重限定**：helper 的 `AllowedClients` 只含控制端上报的那一个节点公钥；直连 QUIC 还要出示一次性令牌，并按被控端为该会话生成的临时自签证书指纹做固定校验（沿用现有证书固定逻辑）。
- **最小暴露面（必须显式配置，不能依赖默认值）**：被控端 helper 的 tailcat `Server` 设置 `ServedTCPPorts` 为**空但非 nil**、`ServedUDPPorts` 只含一个端口，`OnTCP` / `OnTCPForward` / `OnUDPForward` / `AllowProxy` 全部为 nil，`AllowedClients` 在 `Start` 前给定；会话 / `epoch` 结束时 `Close`。该端口的数据报只转发到 Rust 侧的直连监听。
- **本机暴露面**：见方案 B 中“数据报通道”的两个候选；选 (i) 时必须叠加 4 元组锁定、单连接、独立 ALPN 与令牌；选 (ii) 不开任何本机端口。
- **同意与可见性**：直连会话与中转会话共用同样的“托盘显示谁在控制 / 一键断开 / 更换控制码”路径（Android 的“请求同意”在 Phase 1.5，同样适用）；**桌面被控端本来就没有请求同意这一步**，`DirectOffer` 在 `HostHello` 之后发送（I12）。
- **IP 暴露（新增）**：magicsock 会通告公网地址与本机所有网卡地址（含局域网、VPN），升级后对端能获知被控端 / 控制端的公网与内网地址；现状下对端拿不到这些。**持控制码者在桌面被控端上会立即（无需任何同意）获知这些地址**，与“持码即可完全控制”的现有信任模型一致，但须如实写明。因此默认关闭，首次启用时提示并写入 `docs/security.md`。
- **日志**：helper 默认把 tailcat / magicsock 的 `Logf` 设为丢弃（其内部日志含节点公钥简写与对端 IP，而 Windows 日志写入 `OutputDebugString`，本机其他用户可读）；诊断模式须脱敏。
- **Windows SYSTEM 被控端（Phase 0 必须验证的具体方案）**：被控端 agent 以 SYSTEM 在控制台会话运行（含登录界面，没有用户令牌可借）。helper 由被控端以**受限令牌**启动：以服务自身 SYSTEM 令牌为基础 `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)`，把用户 SID 与 Administrators 设为 **deny-only**（注意：令牌的用户 SID 仍是 SYSTEM，只是带 deny-only 属性），特权仅保留 `SeChangeNotifyPrivilege`，**低完整性级别**，并套 **Job 对象**（`KILL_ON_JOB_CLOSE`、禁止创建子进程、内存上限）；helper 不读写磁盘、不加载保存的密钥。Phase 0 须证实：该令牌下 netmon / magicsock 能绑定 UDP 并枚举网卡；默认 DACL 只授予 SYSTEM / Administrators 时进程能否打开自己新建的对象、Go 运行时是否正常（Chromium 沙箱需专门处理）；是否需要 restricting SIDs 及其对 Winsock 的影响；**AppContainer** 作为对照备选（默认禁止回环，配合候选 (ii) 的独立管道）；Windows 防火墙对该进程的行为（是否弹窗 / 是否需要安装器加规则）。**退路：若证实不可行，Windows SYSTEM agent 作为被控端时禁用直连；控制端不受影响。**
- 隐私：启用时两端会向 tailcat 的 DERP 与 DERP 图服务暴露自己的公网 IP 与连接时间；默认关闭，且可改用自建 DERP。

## 非目标

- 不修改中转服务器**源码**（`server/`）；但需要用 v10 重新构建并部署（见“发布与回滚”）。不改变 6 位控制码与重连凭证机制。
- Phase 1 **不做** Android 与网页控制台（`web/`）的直连行为；它们列为 Phase 1.5 TODO（它们的 Rust 代码仅需为新协议字段通过编译）。Phase 1 只覆盖桌面客户端（Windows / macOS / Linux）两端互连。
- **不实现 Tailscale 与 ZeroTier**（只在“后续阶段”写扩展点）；不接入任何 Tailscale 控制面 / 账号 / tailnet。
- 不使用 tailcat 的 ssh / files / exec / exit-node / socks 功能；不提供“把 airhand 当通用隧道”的能力。
- 不实现多路径聚合 / 同时多条直连；不做额外端口映射逻辑（magicsock 自带的除外）。
- 不上游提交；不发布；不写 `main`。

## 不变量

- **I1** 默认关闭；关闭时：**不发任何 `Direct*` 消息、不启动子进程、不联系 tailcat / DERP**（线上消息与“v10 且未启用”一致；与 v9 不互通）。
- **I2** 直连失败、超时、对端未启用、helper 缺失 / 校验失败 / 崩溃，一律**静默回退**中转，不中断会话、不弹错误（只在诊断面板留痕）。**静止桌面（无画面、无声音）不得触发回退**。
- **I3**（有界）路径一旦按 §2 规则判为非 `direct`，helper 闸门关闭，停止把数据报写入隧道；升级前不为“尚未 direct”的路径切换发送端。窗口期内少量数据报可能经共享 DERP；严格为零仅在自建 DERP 下成立。具体时限由路径判定规则与 Phase 0 实测给出（含 RTT 相关项），不写死 250 ms。
- **I4** 握手消息（`Hello` / `Welcome` / `Incompatible`）与 ALPN 的编码不变；`PROTOCOL_VERSION` 升到 10。
- **I5** 令牌一次性：只对一个会话、一个 `epoch`、有效期内有效；重放、过期、错会话、错 `epoch` 一律拒绝。
- **I6** 同一时刻每条逻辑流只有一个**发送**通道。升级方向：栅栏保证控制消息不乱序、不丢失，画面 / 声音按序号去重且不倒退。回退方向：以 `DirectFallback` 反向栅栏切断直连控制流，迟到的直连控制消息被丢弃；直连上未确认的数据可能丢失，以按键释放与状态重同步补偿。
- **I7** tailcat 地址、节点密钥、令牌不出现在日志、`Debug` / `Display` 输出、命令行参数、`config.toml`、错误文案与崩溃信息里（`Secret<T>`）；helper 默认丢弃 tailcat 内部日志。
- **I8** 不依赖解码器对未知变体的容忍：新增 `kind` 一律由 `DirectDecline` 回应；协议扩展以版本号为界。
- **I9** helper 只服务一个 UDP 端口、无 TCP 服务；对其他端口 / TCP 不回 RST（探测超时）；回环通道的暴露面按所选候选 (i)/(ii) 的约束。
- **I10** 托盘显示 / 断开 / 更换控制码对直连会话的语义与中转会话相同；升级过程中被踢 / 更换控制码，两条通道一并结束；`Bye` 一律经中转发送。
- **I11** 直连 QUIC 单个数据报载荷 ≤ 1232 字节（直连专用 `Endpoint` 与 `direct_transport_config()`：`initial_mtu` ≤ 1232、`min_mtu` 1200、关闭 MTU 探测、`EndpointConfig` 最大 UDP 载荷 1232；中转用的 `transport_config()` 不变）。具体 quinn API 名称在 Phase 0 对照 0.11 源码确认。
- **I12** `DirectOffer` 只在 `HostHello` 已发之后发送；此前收到的 `DirectRequest` 丢弃。在有“请求同意”流程的平台（Android，Phase 1.5）还须在同意之后。
- **I13** 提交由被控端裁决；迟到的 `DirectRequest` / 接受 / 连接（`epoch` 已过期或放弃）一律忽略；任一端经中转收到对端的 `DirectFallback` 立即回退；控制端等不到 `DirectCommit` 超时即回退。
- **I14** `client_key` 为空或无法解析时，Rust 侧与 helper 都拒绝 Start（不得起一个放行所有客户端的 `Server`）；helper **不调用** `AddAllowedClient`（运行时追加会重新引入“列表为空即放行”的窗口）。

## 验收测试清单

测试命名采用 Rust `#[test]` / `#[tokio::test]` 函数名与 Go `Test…` 名；文件为**将要创建**的路径，实现阶段按“先红后绿”提交。
验证面均为**本机自动化**（回环 + 假 helper / 可注入路径源）；真实 NAT / 跨公网表现只能在**线上手动验证**。托盘 / 被控端会话逻辑位于 `client/src-tauri`，相关测试放在该 crate，不放在 `airhand-link`。

- [ ] I4：`crates/airhand-proto/src/framing.rs` →「`version_handshake_wire_is_stable`」（既有，必须仍通过）；`crates/airhand-proto/src/lib.rs` →「`protocol_version_is_10`」；`crates/airhand-proto/src/session.rs` →「`direct_messages_wire_is_stable`」。
- [ ] I1：`crates/airhand-link/tests/direct.rs` →「`disabled_by_default_never_spawns_helper`」；预期：假 helper 工厂 0 次调用、无任何 `Direct*` 消息、无到 tailcat/DERP 的连接尝试。
- [ ] 配置：`client/src-tauri/src/config.rs` 单测 →「`direct_config_defaults_off`」「`direct_config_rejects_non_https_derpmap`」「`direct_config_has_no_helper_path_option`」。
- [ ] 升级：`crates/airhand-link/tests/direct.rs` →「`session_upgrades_to_direct_when_both_sides_enable`」；预期：升级后画面包经直连流到达、中转流字节计数不再增长。「`peer_without_direct_enabled_stays_on_relay`」。
- [ ] I8：「`unknown_offer_kind_is_declined`」。
- [ ] I12：「`no_offer_before_host_hello`」「`request_before_host_hello_is_dropped`」。
- [ ] I13：「`late_request_after_host_timeout_is_ignored`」「`host_fence_then_commit_is_first_direct_message`」（被控端先在中转发 `DirectSwitch`，再在直连发 `DirectCommit` 作为第一条）「`commit_lost_controller_falls_back_within_bound`」「`fallback_message_forces_peer_to_fall_back`」「`asymmetric_direct_failure_recovers_within_bound`」。
- [ ] I2 / 存活：「`static_screen_does_not_trigger_fallback`」（无画面无声音数十秒，Ping/Pong 正常 ⇒ 不回退）「`missing_pong_triggers_fallback`」「`direct_failure_falls_back_to_relay`」「`helper_crash_does_not_end_session`」「`direct_stream_failure_does_not_end_session`」「`relay_loss_ends_session_even_when_direct`」「`direct_write_failure_requeues_on_relay`」。
- [ ] I3（Rust 侧）：「`upgrade_timeout_keeps_relay`」「`no_switch_before_both_gates_open`」。I3（Go 侧，helper 本身）：`p2p/gate_test.go` →「`TestGateDropsWhenPathNotDirect`」（可注入路径源）「`TestControllerGateIgnoresSlowButDirectPath`」（RTT 大于 250 ms 的直连不被误判）「`TestHostGateClosedWhenCurAddrEmpty`」；另附**手动 / 可选 CI**：用本地自建 `derper` 验证真实 DERP 退化行为与窗口时长。
- [ ] I5：「`direct_token_is_one_time`」「`direct_token_wrong_session_rejected`」「`direct_token_expired_rejected`」「`direct_token_wrong_epoch_rejected`」；证书固定：「`direct_cert_pin_mismatch_aborts_upgrade`」。
- [ ] I6：「`input_order_preserved_across_switch`」「`fence_timeout_drops_buffer_and_releases_keys`」「`late_direct_input_after_fallback_is_discarded`」「`keys_released_after_fallback`」「`settings_resynced_after_fallback`」「`bye_always_sent_on_relay`」「`duplicate_frames_across_lanes_are_dropped`」「`stale_lane_frames_before_keyframe_are_dropped`」「`audio_not_replayed_after_switch`」。
- [ ] I11：「`direct_quic_never_sends_datagram_over_1232`」（行为测试：假 helper 记录并丢弃超限数据报，断言观测到的最大数据报 ≤ 1232）。
- [ ] I7：「`direct_secrets_are_redacted_in_debug_and_logs`」（`Secret<T>`）；Go「`TestTailcatLogfDiscardedByDefault`」。
- [ ] I9 / I14：Go `p2p/helper_test.go` →「`TestServesExactlyOneUDPPort`」「`TestNoTCPServiceAndNoRST`」（SYN 超时而非 RST）「`TestOnlyAllowedClientKeyConnects`」「`TestEmptyOrInvalidAllowlistRefusesToStart`」「`TestMultipleServersInOneProcessAreIndependent`」（不通过则改为每 epoch 一进程）；Rust：「`empty_client_key_refuses_start`」；若采用候选 (i) →「`direct_listener_binds_loopback_only`」「`direct_listener_accepts_single_connection`」；若采用 (ii) →「`direct_path_opens_no_local_port`」。
- [ ] helper 协议：Go「`TestControlProtocolNeverEchoesAddressToArgvOrStderr`」「`TestDatagramPipeIsSeparateFromControlStdio`」（候选 (ii)）。
- [ ] 并发：「`two_concurrent_sessions_upgrade_independently`」「`session_over_cap_stays_on_relay`」。
- [ ] I10（`client/src-tauri` 内）：「`kick_all_ends_direct_and_relay_lanes`」「`new_code_ends_direct_session`」「`kick_during_upgrade_ends_both_paths`」。（“同意”相关用例属于 Android，Phase 1.5。）
- [ ] Windows（Windows CI / 真机）：「`helper_token_is_restricted_on_windows`」：断言用户 SID 与 Administrators 为 deny-only、特权只剩 `SeChangeNotifyPrivilege`、完整性级别 Low、位于带 `KILL_ON_JOB_CLOSE` 与进程数限制的 Job 内（若采用 restricting SIDs 则断言其存在）。
- [ ] 完整性：「`helper_hash_mismatch_disables_direct`」（只验证防误放 / 损坏，不声称防同用户攻击）。
- [ ] 回归：`server/` 源码无 diff（`git diff --stat` 校验）；`cargo test`（根工作区）、`cargo test`（`web/`）、`npm test`（`client/`）、`cargo test`（`android/native`）全部仍通过。
- [ ] 手动（不计入自动通过数，须单独记录）：两台不同网络的真机开启后确认路径为 direct；记录升级耗时、码率、延迟、CPU；对称 NAT 下确认回退；Windows 防火墙行为；macOS 嵌套 helper 签名 / 公证 / 防火墙弹窗；杀毒软件误报情况；Linux 各分发形态（deb / AppImage 等）带上 helper 且卸载干净。

## 阶段与门禁（对应 workflow v2）

| 阶段 | 内容 | 门禁 / 证据 |
|---|---|---|
| 调研 | 上游机制、tailcat 核实 | 已完成：见本文与 feature map |
| 规格 | 本文 + feature map，走分支 + PR | PR #1（Draft） |
| 设计评审 | 需求与架构评审（独立于实现），分级 findings，修订后复审至无严重问题 | [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（三轮报告全文在 [`review/`](review/)） |
| 项目负责人批准 (A) | 本变更属重大改动；spec 与评审都不代替批准。(A)：**仅批准 Phase 0 spike**，范围限定在独立 spike 分支，**不得合并，spike 代码不得直接复用进 Phase 1**（Phase 1 按先红后绿重写） | PR 评论记录批准结论 |
| Phase 0 spike | 实测并产出数据，见下表清单与 go/no-go 阈值 | 结果写入规格 r4 |
| 复审 | **Phase 0 结果 → 规格 r4 → 再做一轮设计复审**直到无 S1 | 追加到评审文档 |
| 项目负责人批准 (B) | 批准 Phase 1 全量实现；“降级为仅低码率档”属范围变更，须在 (B) 中单独确认 | PR 评论记录批准结论 |
| 先红后绿 | 先提交行为断言失败的测试，再最小实现 | 实现 PR（另开分支） |
| 验收 | 本地全部套件真实通过数 + 手动线上验证记录 | PR 描述 `## 验收清单` / `## 风险分级` |
| 合并 | 仅在本 fork，不向上游提 PR；需批准 | — |

### Phase 0 清单（实测项）

1. 吞吐 / 延迟 / CPU：QUIC 经 helper + tailcat 直连 UDP（回环候选 (i) 与独立管道候选 (ii) 各测一次）；对照经中转的同场景。
2. 超 1232 字节写入的库内行为；quinn 0.11 直连配置项的确切名称与默认值。
3. `DiscoPing` 超时语义、RTT 对判定的影响；`Server.Status()` 的 `CurAddr` 变化时序；两端判定为 direct 的时间差；magicsock 何时开始经 DERP 双发。
4. `Client.Ping` 内部超时、DERP 图拉取耗时 ⇒ 校准升级超时。
5. v0.7.0 下 `AllowedClients` 为空 / 无效公钥的行为（预期：放行所有，故需 I14）。
6. 同一进程内多个 `Server` 是否安全（进程级全局设置）。
7. Windows：受限令牌（含默认 DACL、restricting SIDs、Go 运行时）与 AppContainer 对照；UDP 绑定 / 网卡枚举；防火墙行为。
8. helper 体积与第三方许可清单；macOS 嵌套签名 / 公证可行性。
9. 中转 RTT 下栅栏等待上限、Commit 等待 T、存活检测 K 的校准（均须小于现有 2 秒命令队列过期）。

### 建议的 go/no-go 阈值（供项目负责人在批准 (A) 前确认或调整）

| 指标 | 阈值 |
|---|---|
| helper 额外 CPU（被控端，1080p60、约 20 Mbps 单会话；参考硬件同上） | ≤ 单核 30% |
| 局域网直连额外往返延迟（相对不经 helper 的同路径） | ≤ 3 ms |
| 单会话持续吞吐（5 分钟，在 QUIC 层统计） | ≥ 60 Mbps，QUIC 层丢包 < 0.1%；产品允许的最高码率为 100 Mbps，直连会话的码率上限策略（限制码率 / 回退）在 r4 定 |
| 路径判定误判（直连稳定且 RTT ≤ 400 ms，30 分钟；与 Go 用例覆盖的 RTT 范围对齐） | 0 次 |
| 存活检测误回退（直连稳定；静止桌面 + Wi-Fi 场景，30 分钟） | 0 次 |
| I3 窗口（自建 `derper`，路径被人为切断后） | 闸门关闭 ≤ 2 s；经 DERP 发出的数据报数量如实记录（上限值在首轮数据后与项目负责人确认） |
| 升级完成时间（p50；起点：控制端发出 `DirectRequest`；参考硬件：近三年主流桌面 / 笔记本，在 spike 报告中写明型号级别） | ≤ 12 s（含 DERP 引导与打洞） |
| Windows 受限令牌 / AppContainer | 至少一种能绑定 UDP 并正常运行；否则走退路（Windows SYSTEM 被控端禁用直连） |
| 任一不达标（Windows 一行除外，Windows 一行有自己的退路，不触发整体放弃） | 降级为“仅低码率档可用”或放弃；该决定属范围变更，须项目负责人单独确认 |

> 工作流适配说明：`docs/workflow-v2.md` 与 `AGENTS.md` 来自另一个仓库（passkey 演示），其中 `tests/*.mjs`、CI 检查名（`spec-requires-tests`、`agent-trailer`、`privacy-scan`）是那个仓库的实现。
> 本 fork 目前没有这些脚本与 CI（上游也没有 `.github/`）。本规格把“验收映射到测试用例名”改写为 Rust / Go 用例名；是否把检查脚本与 CI 移植进本 fork，作为开放问题交项目负责人决定。

## r4 待办（第 3 轮复审的 S2 / S3，Phase 0 之后、批准 (B) 之前落实）

评审结论：无 S1，可提交批准 (A)。下列条目**不阻断 (A)**，但批准 (B) 前必须关闭（详见 [第 3 轮报告](review/tailcat-p2p-round3.md)）：

1. **S2-1 回退发起方**：写明发起方也丢弃该 epoch 后续的直连控制消息（丢失靠 `release_all()` 与状态重同步补偿），或改为带确认的回退；列出栅栏之后仍须处理的中转消息（`Bye`、`DirectFallback`、对向 `DirectSwitch`）；“改投中转”只限未写出且未过期的消息，过期输入不补发，正在写的一条视为丢失；补用例 `host_initiated_fallback_discards_inflight_direct_input`、`bye_on_relay_processed_after_fence`、`expired_input_not_replayed_on_relay_after_fallback`。
2. **S2-2 存活超时**：去掉“须小于 2 秒”的约束（直连通道的写超时 / 队列过期按通道区分，一律转为回退）；直连期间使用比 1 秒更短的 Ping 间隔（`Ping` 是现有消息，不改协议），K 取 `max(1.5 s, N×间隔 + 2×RTT)` 一类形式，由 Phase 0 校准；被控端写卡住判定是 10 秒而非 2 秒，措辞写准确。
3. **S3-1** 被控端 `Server.Status()` 轮询间隔（≤ 250 ms）与 `CurAddr` 失效延迟计入 I3 窗口；**S3-2** Commit 计时以“读到 `DirectCommit`（不论是否在缓存中）”为准，回退后迟到的 Commit 一律忽略（`commit_after_own_fallback_is_ignored`）；**S3-3** 删除“控制端清空本地按键状态”，改为 `airhand-link` 控制端按会话缓存最近一次 `Settings` / `Audio` / `SelectDisplay` 并在回退后重发；**S3-4** Windows 验收改为“手动 / 真机，以 SYSTEM 服务身份运行”，补功能验证 `helper_under_restricted_token_completes_direct_session` 与反向验证 `restricted_helper_cannot_open_parent_or_write_install_dir`；**S3-6** 选 (i) 时补 `direct_listener_uses_exclusive_addr_on_windows`；**S3-7** 验收映射整理（`bye_always_sent_on_relay` 归 I10，I14 单列）。

## 后续阶段（TODO，仅扩展点，本次不实现）

- **Phase 1.5**：网页控制台（`web/` 容器内置 helper）与 Android（无法像桌面一样拉子进程；候选方案 C′ `libtailcat`，需另行评审）。
- **Phase 2 — Tailscale**：已在 tailnet 内的两端不需要 tailcat：被控端把本机 tailnet 地址（Tailscale 使用的 CGNAT 段与 ULA 段）作为新的 `DirectOffer` `kind`（候选 UDP 地址列表）；直连 QUIC 监听绑定到该地址（需要非回环监听，安全模型要单独评审），仍用一次性令牌 + 证书固定。地址发现只枚举本机网卡；**airhand 不接触 Tailscale 登录 / auth key / API**。新增 `kind` 需升协议版本或用 `DirectDecline` 协商。
- **Phase 3 — ZeroTier**：同上；候选地址来自 ZeroTier 虚拟网卡（按网卡名前缀 / 用户配置的 CIDR 白名单枚举）。**网络 ID 与 token 不入库，airhand 不调用 ZeroTier 本地服务 API**。
- 共同点：升级 / 回退状态机复用；对应的新规格与评审另开。

## 风险

- **tailcat 无稳定性承诺**：API / 线格式可能变；对策：固定 `v0.7.0`（commit `15ab9e6`）并记录 `go.sum` 哈希；升级走单独 PR + 回归；两端 helper 版本不一致时握手失败即回退中转。
- **不能禁止 DERP 承载数据**：窗口期内少量数据报可能经 Tailscale 共享 DERP（有界，见 I3）；共享 DERP 限速、可撤销、可见元数据；默认关闭 + 可自建 DERP。
- **性能未知**：QUIC 经回环 / 管道 + 用户态 WireGuard + gVisor 的额外 CPU / 延迟 / 吞吐上限（4K、120 fps 码率较高）；Phase 0 先量，不达标则降级或放弃。
- **1232 字节上限与库行为**：超限时库内是丢弃 / 报错 / 分片未证实；QUIC 载荷被迫 ≤ 1232，开销略增。
- **打洞失败率**：对称 NAT / 严格网络下只能走 DERP；本设计在窗口期之后退回自建中转，因此这类网络上**没有收益但也没有长期损失**。
- **控制流切换复杂度**：栅栏 + 回退重同步是整个设计中最容易出错的部分；复审与测试重点。
- **Windows SYSTEM 被控端**：见安全模型的具体方案；Phase 0 前不得假设可行。
- **新增运行时依赖与供应链**：Go 工具链、tailscale.com / gVisor / wireguard-go 体积（待实测）与许可声明；安装包体积增长；杀毒软件对 Go / WireGuard 二进制的误报。
- **打包与平台**：Windows 防火墙、macOS 嵌套 helper 的签名 / 公证 / hardened runtime / 应用防火墙弹窗、卸载流程（`uninstall.rs`）清理 helper、Linux 各分发形态带上 helper——均为 F12 子项，各自需验收。
- **协议版本升级**：v9 ↔ v10 不互通；服务器与所有客户端须同时升级（见“发布与回滚”）。
- **仍需线上验证**：真实跨网络打洞成功率、码率与延迟、DERP 引导可用性——自动测试无法覆盖。

## 开放问题（需项目负责人 / 评审结论）

1. 集成形态：确认方案 B（自带 Go helper 子进程）。备选 A、C、D 已否决；C′ 留给 Android。是否接受新增 Go 构建与第三方许可声明？
2. DERP 策略：默认使用 Tailscale 托管的限速 DERP 做引导（以及路径退化窗口期内有界地承载少量数据）是否可接受？还是要求先自建 `derper`？
3. 控制流是否迁移：本设计迁移并用栅栏保证顺序；若觉得复杂度过高，可退为“只迁移画面与声音”（输入延迟不受益）。
4. 批准节奏：是否接受“分两段批准”（先批准 Phase 0 spike，不合并；Phase 0 结果 → 规格 r4 → 复审 → 再批准 Phase 1 全量）？go/no-go 阈值是否按上表或另行调整？
5. Phase 1 范围：仅桌面（Windows / macOS / Linux）；网页控制台与 Android 是否确认后置？
6. 本 fork 是否移植 workflow v2 的检查脚本与 CI（隐私扫描、署名 trailer、spec-requires-tests），还是沿用人工 + 一次性扫描？
7. fork 保持 MIT；helper 引入的第三方许可（BSD-3 / MIT / Apache-2.0 等）是否接受？是否在 README 加声明？
8. 发布：服务器镜像与全部客户端须同步升级到 v10，是否接受？
9. 直连是否追加 mTLS（让控制端出示临时客户端证书指纹）：本设计暂不采纳——已有 `AllowedClients`（单公钥）+ 一次性令牌 + 证书指纹固定三层，客户端证书会增加 `rcgen` / rustls 配置复杂度；如评审认为必要可在 r4 加入。
10. 本特性对桌面被控端没有“请求同意”环节：持码人会立即获知被控端公网与内网地址（见安全模型）；是否接受，或要求直连默认仅对“已保存的设备”开放？

## 是否重大改动

**是**。理由：新增监听 / 子进程（权限与安全边界变更）、跨 `proto` / `link` / `client` / 新增 `p2p` 多个子系统、新增第三方运行时依赖、协议版本升级、新增 IP 暴露面。
按 workflow v2「角色与硬约束」：重大改动必须等待项目负责人批准；spec 与设计评审都不代替批准。批准结论记录在 PR 评论中。
