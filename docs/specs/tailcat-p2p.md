# tailcat 临时点对点直连（Direct P2P transport）

> 状态：**规格 r5（2026-11-10 项目负责人对开放问题 11–14 的决定后修订：改写 never 模式下的同意与 IP 暴露不变量；补直连码率上限策略；第 5 轮复审见评审记录），未实现**。本文件只描述设计；不含功能代码。本版**仍未含 Phase 0 实测数据**（超时 / 阈值 / 候选通道 / Windows 结论 / 码率上限 C 的确定值待 spike）；spike 完成后出 r6 再复审、再批准 (B)。评审记录见 [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（各轮报告全文在 [`review/`](review/)）。
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
2. 双方在 `Hello` 里声明是否启用直连能力。仅当**双方都启用**、**且已取得直连同意**（见 I12：`approve = "ask"` 时由被控端用户在确认窗授予“直连”；`approve = "never"` 且 `[direct] enabled` 时由运营者显式配置视为同意）才进入升级流程；否则与未启用时完全一致。**取得直连同意之前不交换任何地址、不启动 helper、不联系 tailcat / DERP。**
3. **公钥先行**（`v0.7.0` 的 `AllowedClients` 为空时**放行所有客户端**，`AddAllowedClient` 在加入第一个公钥之前同样如此，所以必须在 `Start` 时就给定非空列表）：控制端先创建自己的 helper dial 实例（本 `epoch` 新的客户端密钥），用 `Client.PublicKey()`（拨号前即可取得）取得客户端节点公钥，经控制流发 `ControllerMsg::DirectRequest { epoch, client_key }`。`client_key` 为空或无法解析时，Rust 侧与 helper 都**拒绝 Start**（I14）。
4. 被控端只在**已取得直连同意且 `HostHello` 已发出之后**才处理 `DirectRequest`（此前收到的一律丢弃，未取得直连同意的回 `DirectDecline`，I12 / I16）。桌面被控端默认 `approve = "ask"`，每个新会话都要确认（此前桌面被控端不请求同意）；`approve = "never"` 的路径见 I21。被控端为该 `epoch` 在 helper 进程内**新建**一个 tailcat `Server`（新的密钥与 PSK，`AllowedClients = [client_key]`），并生成一次性令牌与该会话的临时自签证书；经控制流回 `HostMsg::DirectOffer { epoch, kind, body, quic_fp, token, ttl }`。
5. 控制端 helper 以 offer 中的地址拨号，并按 §2 的规则检测路径；**仅当路径为 `direct`** 才继续。超时（默认值由 Phase 0 实测后确定，不小于 12 秒，需覆盖 DERP 图拉取与引导）或只有 DERP 则放弃，会话继续走中转，不报错。
6. 控制端经 helper 的数据报通道用 quinn 连接被控端的**直连 QUIC 监听**，出示一次性令牌并校验 `quic_fp`；被控端校验令牌 / 会话 / `epoch` / 有效期通过后，等待控制端在**直连连接**上开三条流（`SessionHeader` 与中转上相同）。
7. **提交（commit）由被控端裁决，顺序固定**：三条直连流就绪后，被控端 **先在中转控制流上发 `DirectSwitch { epoch }`（被控端→控制端方向的栅栏）**，**再**把 `DirectCommit { epoch }` 作为**直连控制流的第一条消息**发出。被控端若在自身超时前没有收到完整的三条流，则放弃该 `epoch`（迟到的 `DirectRequest` / 连接一律忽略，I13）。**控制端打开三条流后设 T（默认 2 秒，Phase 0 校准）超时，以“读到 `DirectCommit`（不论它是否因栅栏而暂存在缓存里）”为准：收不到就经中转发 `DirectFallback { epoch }`**；被控端对已提交的 `epoch` 也必须接受它。
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
- 被控端 `Server.Status()` 的轮询间隔（默认 ≤ 250 ms）与 `CurAddr` 失效延迟都计入 I3 窗口，窗口上限 = 轮询间隔 + 判定延迟（Phase 0 实测）。
- 窗口期（从路径退化到闸门关闭）内仍可能有少量数据报经共享 DERP 发出；严格为零的前提只能是**自建 DERP**。magicsock 在直连信任期内是否会双发 DERP、何时开始双发，列为 Phase 0 实测项（未证实）。

**存活检测（不依赖画面）**：桌面被控端只在画面变化时编码、无声音不发包，静止桌面没有帧也没有 Ack，因此**不能**用画面包或 Ack 判存活。改用直连控制流上的 Ping/Pong：直连期间控制端以比现有 1 秒更短的间隔（默认 250 ms，Phase 0 校准；`Ping` 是现有消息，不改协议）发 `Ping`，被控端回 `Pong`；控制端在直连上 K 秒收不到 `Pong` ⇒ 经中转发 `DirectFallback`；被控端在直连上 K 秒收不到 `Ping` ⇒ 回退。K 取 `max(1.5 s, N×间隔 + 2×RTT)` 一类形式（N、系数由 Phase 0 校准），**不再要求小于 2 秒的命令队列过期**：直连通道的写超时 / 队列过期按通道区分，直连通道上的任何超时、写失败都只转为回退，不影响中转通道的既有判定（被控端对中转的写卡住判定现状为 10 秒，措辞以源码为准）。K 中的 RTT：控制端取自 Ping/Pong 往返；被控端用本会话直连 QUIC 连接自身的 RTT 估计（`Flow` 的 srtt 在通道切换后 `min_rtt` 约 10 秒才更新，切换 / 回退时重置 `min_rtt`，Phase 0 观察是否造成码率抖动）。该规则同时覆盖两个方向的非对称故障。直连 QUIC 连接关闭 / 空闲同样视为失效。

**回退（反向栅栏）**：
- 失效侧立即把自己的发送端切回中转流并请求关键帧，并且它在**中转**控制流上发出的**第一条**消息必须是 `DirectFallback { epoch }`。**发起方也丢弃该 `epoch` 此后收到的所有直连控制消息**（含已缓存的）；直连上未确认的输入丢失，靠 `release_all()` 与状态重同步补偿（不改为“带确认的回退”，避免多一轮往返）。
- 对端无论从哪里得知（收到 `DirectFallback`、或自己检测到故障）都**立即回退**；收到 `DirectFallback` 的一方此后**丢弃**该 `epoch` 的所有直连控制消息（含已缓存的），只处理中转上的消息。
- **栅栏之后中转上仍须处理的消息**：`Bye`（I10）、对端的 `DirectFallback`（幂等）、对向未完成的 `DirectSwitch`（该 `epoch` 已放弃，忽略）。回退后迟到的 `DirectCommit` 一律忽略。
- **“改投中转”的范围**：只限**尚未写出且未过期**的消息；已过期的输入**不补发**；正在直连上写的那一条视为丢失。
- **状态重同步**：被控端在本地调用 `release_all()` 释放按键；控制端的 `airhand-link` **按会话缓存最近一次 `Settings` / `Audio` / `SelectDisplay`，回退后重发**（`Settings` 与 `Audio` 的重发是幂等的；缓存的 `SelectDisplay` 在收到被控端的 `HostMsg::Displays { current }` 时同步更新，避免旧序号把画面切回去，被控端已 `follow_display` 切换的以被控端为准）（不依赖“清空本地按键状态”，也不新增 `ReleaseAll` 输入事件）；剪贴板以最新状态为准。
- **写入失败不结束会话**：直连通道的写入失败 / 卡住一律转为回退并把符合上条范围的排队消息改投中转；**直连通道失败只触发回退，绝不结束会话**；仅中转连接断开才结束会话（与 I10 一致）。helper 进程退出 / 崩溃视同直连失效，由 Rust 侧监督，不自动无限重启。
- 每次重新升级都使用**新的 `epoch`、新的 tailcat 密钥 / 地址 / 令牌**（旧的立即销毁）；因路径 / 存活检测而回退后，每隔 30 秒、最多 3 次重试。**若本会话已由被控端收回“直连”权限，则本会话不再重试升级**（`revoked_direct_not_retried_in_session`）。
- 并发与进程模型：**每个会话一个 helper 进程；每个 `epoch` 在进程内新建 `Server`（控制端新建 `Client` 与密钥）并 `Close` 旧的**。同一进程内多个 `Server` 是否安全（例如进程级全局设置）**未证实**，Phase 0 验证；不安全则改为每个 `epoch` 一个进程。并发上限默认 2（可配，须小于服务器为每台被控端预留的会话槽位，该槽位默认 4；客户端拿不到服务器环境变量，不得引用 `AIRHAND_MAX_SESSIONS`），超出的会话保持中转。

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
  - 确认与断开（r4）：沿用现有 `HostMsg::AwaitingApproval`、`ByeReason::{Declined, Unanswered, Kicked, CodeChanged}`；`HostHello` 新增 `granted: Permissions { input, clipboard, audio, direct }`；新增 `HostMsg::PermissionsChanged { granted }`、无字段的 `ControllerMsg::Bye`、`ByeReason::Busy`（均追加在枚举末尾）。
- 前向兼容的实际情况（已对照源码）：控制流上**任何**解码错误都会结束会话，postcard 对未知枚举变体会报错，所以“预留扩展”**不**提供前向兼容；`DirectOffer` 因此用 `kind: u8` + 不透明 `body`，接收端遇到不认识的 `kind` 回 `DirectDecline`，不依赖解码器容忍未知变体。Kotlin / TypeScript 不解析 postcard，只处理 Rust 转出的 JSON 事件与媒体包；**r4 起桌面前端（`client/` 的 TypeScript）需要新增确认窗、指示条、`PermissionsChanged`、路径显示与 SAS 的界面**（列入 F21 / F23 / F26），Android / 网页控制端不改行为但须容忍新的事件与 `Problem` 类型（例如 `Busy`，也可复用现有 `Problem::HostBusy`），并有“旧前端忽略未知事件”的用例；但 `android/native`、`web/` 的 Rust 代码要为新字段 / 新变体通过编译。
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
| 其它阈值 | 由 Phase 0 实测定 | 栅栏等待上限、Commit 等待 T、Ping 间隔与存活检测 K、路径判定的 M / RTT 系数、`Server.Status()` 轮询间隔 |
| `max_sessions` | `2` | 同时升级的会话数上限 |

被控端确认相关的 `[host] approve` / `approval_timeout_secs` / `reconfirm_minutes` / `max_pending_approvals` 见“被控端确认与主动断开”（这些**不属于** `[direct]`，且 `approve = "ask"` 对中转会话同样生效）。

helper 可执行文件的路径**不可配置**：固定为主程序同目录下的文件，启动前校验构建时记录的哈希（或签名），不一致则不启用直连。该校验只防误放 / 损坏文件；在用户可写的安装位置（如 AppImage、用户目录安装）防不了同一用户下的攻击者（对方本来就能替换主程序），文档如实说明。

### 5. 安全模型

- **信任假设（如实表述）**：地址（含 PSK）、令牌、`quic_fp`、`client_key` 经中转的控制流**明文**转发（TLS 在中转终止）。**本特性不提供对中转的端到端保密：主动作恶的中转可以替换 `DirectOffer` / `DirectRequest`，让两端连到自己并在直连路径上做中间人**，与今天中转可读写会话内容等价；控制码由服务器分配，该限制无法靠带外密钥消除，需在 `docs/security.md` 说明。收益仅限：诚实但好奇的中转运营者（升级完成后媒体与输入不再经过它）与路径上的被动观察者。
- tailcat 地址是不记名凭据 → **每个会话、每个 `epoch` 一次性**：新建、用完即毁；永不使用 `tailcat genkey` 的保存密钥；地址只经会话控制流下发，**不写日志、不进 `config.toml`、不放命令行参数、不进崩溃报告**；在 Rust 里用对 `Debug` / `Display` 脱敏的 `Secret<T>` 包装（`HostMsg` 派生 `Debug`，Windows 日志写入 `OutputDebugString`，本机其他用户可读，所以 I7 尤其重要）。
- **双重限定**：helper 的 `AllowedClients` 只含控制端上报的那一个节点公钥；直连 QUIC 还要出示一次性令牌，并按被控端为该会话生成的临时自签证书指纹做固定校验（沿用现有证书固定逻辑）。
- **最小暴露面（必须显式配置，不能依赖默认值）**：被控端 helper 的 tailcat `Server` 设置 `ServedTCPPorts` 为**空但非 nil**、`ServedUDPPorts` 只含一个端口，`OnTCP` / `OnTCPForward` / `OnUDPForward` / `AllowProxy` 全部为 nil，`AllowedClients` 在 `Start` 前给定；会话 / `epoch` 结束时 `Close`。该端口的数据报只转发到 Rust 侧的直连监听。
- **本机暴露面**：见方案 B 中“数据报通道”的两个候选；选 (i) 时必须叠加 4 元组锁定、单连接、独立 ALPN 与令牌；选 (ii) 不开任何本机端口。
- **同意与可见性**：见下一节“被控端确认与主动断开”。桌面被控端默认 `approve = "ask"`，每个新会话都要确认；`DirectOffer` 只在已取得直连同意之后发送（I12 / I16 / I21）；直连会话与中转会话共用同样的托盘 / 常驻指示 / 一键断开路径。
- **IP 暴露**：magicsock 会通告公网地址与本机所有网卡地址（含局域网、VPN），升级后对端能获知被控端 / 控制端的公网与内网地址；现状下对端拿不到这些。**未经同意不得暴露这些地址**，持码人本人不能单方面触发。同意的来源（I12 / I21）：
  - 被控端 `approve = "ask"`（默认）：必须由被控端用户在该会话确认窗口里明确勾选“允许直连（对方将获知本机公网 / 内网地址）”。
  - 被控端 `approve = "never"`：运营者显式把被控端设为免确认，**且**显式打开 `[direct] enabled`，这两项配置一并视为同意；此模式下允许直连与地址交换，不再每会话弹窗勾选。只开 `never` 而不开 `[direct] enabled` 时，不交换地址、不启动 helper。配置何时生效、如何防止远端改配置，见“配置的读取与生效”。
  - 控制端：本端 `[direct] enabled = true` 视为对“暴露本机公网 / 内网地址并经第三方 DERP 引导”的同意；首次启用时在本机提示。控制端不会在未启用直连时启动 helper 或交换地址。
  因此 `[direct] enabled` 默认关闭，并写入 `docs/security.md`。已获输入权限的控制端本来就能借本机会话获知被控端 IP，直连同意主要防止未获输入权限的对端、以及未来未经同意的会话在升级路径上暴露地址。
- **日志**：helper 默认把 tailcat / magicsock 的 `Logf` 设为丢弃（其内部日志含节点公钥简写与对端 IP，而 Windows 日志写入 `OutputDebugString`，本机其他用户可读）；诊断模式须脱敏。
- **Windows SYSTEM 被控端（Phase 0 必须验证的具体方案）**：被控端 agent 以 SYSTEM 在控制台会话运行（含登录界面，没有用户令牌可借）。helper 由被控端以**受限令牌**启动：以服务自身 SYSTEM 令牌为基础 `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)`，把用户 SID 与 Administrators 设为 **deny-only**（注意：令牌的用户 SID 仍是 SYSTEM，只是带 deny-only 属性），特权仅保留 `SeChangeNotifyPrivilege`，**低完整性级别**，并套 **Job 对象**（`KILL_ON_JOB_CLOSE`、禁止创建子进程、内存上限）；helper 不读写磁盘、不加载保存的密钥。Phase 0 须证实：该令牌下 netmon / magicsock 能绑定 UDP 并枚举网卡；默认 DACL 只授予 SYSTEM / Administrators 时进程能否打开自己新建的对象、Go 运行时是否正常（Chromium 沙箱需专门处理）；是否需要 restricting SIDs 及其对 Winsock 的影响；**AppContainer** 作为对照备选（默认禁止回环，配合候选 (ii) 的独立管道）；Windows 防火墙对该进程的行为（是否弹窗 / 是否需要安装器加规则）。**退路：若证实不可行，Windows SYSTEM agent 作为被控端时禁用直连；控制端不受影响。**
- 隐私：启用时两端会向 tailcat 的 DERP 与 DERP 图服务暴露自己的公网 IP 与连接时间；默认关闭，且可改用自建 DERP。

## 被控端确认与主动断开（2026-11-10 项目负责人批复；同日 13:08 Asia/Taipei 就开放问题 11–14 作出决定）

> 决定（项目负责人 2026-11-10 批复，并经同日对开放问题 11–14 的决定补充）：默认情况下**控制码不等于自动接受**——`approve = "ask"` 时，被控端用户必须先确认，才会交换任何地址 / 升级直连。无人值守机器由运营者显式设 `approve = "never"` 免确认，且该模式在同时显式开启 `[direct] enabled` 时**允许直连**（运营者的显式配置视为同意，见 I21）。被控端要有随时可用的主动断开。握手参考现成方案。

### 现成方案调研（来源均为官方文档 / 源码；许可单列）

| 方案 | 许可 | 被控端确认 | 断开 / 持续提示 | 对本设计的启示 |
|---|---|---|---|---|
| **RustDesk**（同品类、开源；[源码 `src/server/connection.rs`](https://github.com/rustdesk/rustdesk/blob/master/src/server/connection.rs)、[文档 `approve-mode`](https://rustdesk.com/docs/en/self-host/client-configuration/advanced-settings/)） | **AGPL-3.0** | `approve-mode` = `password` / `click` / `password-click`（默认 `password-click`；源码 `ApproveMode::Both`：口令与点击二选一——有效口令存在时可凭口令接入，否则需点击；核对的源码为 master @ `95cca3f66`）；`click` 时连接先处于“未授权”，由连接管理窗口点“接受”才发 `LoginResponse`（含主机信息与显示器信息），之前不发画面 | 连接管理窗口可“断开”（`Close`）并可逐连接切换权限（键鼠 / 剪贴板 / 音频 / 文件 / 重启等）；未授权连接有宽限期上限（`LOGIN_GRACE` = 180 s，超时即关闭）、未授权连接总数上限（64）与单地址上限（16）、授权前消息大小上限（`MAX_UNAUTHORIZED_MESSAGE` = 128 KiB）；可选无操作自动断开 | 同品类基线：授权前不发主机信息 / 画面；超时即关；授权前资源与消息大小受限；逐连接权限开关与随时断开 |
| **Chrome Remote Desktop（远程协助）**（[官方帮助](https://support.google.com/chrome/answer/1649523)；Chromium 的 `remoting/` 为 BSD-3-Clause，服务本身为专有） | 客户端部分 BSD-3-Clause，服务专有 | 一次性 12 位访问码（5 分钟过期、只能用一次）；对方输入后，被控端弹出**显示对方身份**的对话框，点“共享”才开始 | 共享期间常驻提示条，随时“停止共享”；每 30 分钟弹出“是否继续共享”，该提示的默认按钮是“停止共享”，避免回车误延长 | 一次性码、显示请求者身份的确认、常驻指示 + 一键停止、周期性续期（默认按钮为停止） |
| **AnyDesk**（[官方文档：接受窗口](https://support.anydesk.com/docs/accepting-the-connection)、[交互式访问](https://support.anydesk.com/docs/interactive-access)） | 专有 | 接受窗口显示请求者身份与可勾选的权限 / 权限配置文件，点“接受”或“拒绝”；可设“始终 / 仅窗口打开时 / 从不显示请求”（“从不”则只能走无人值守密码） | 接受窗口在会话期间始终显示，随时“断开”；全部权限关闭即只读 | 请求者身份 + 权限勾选的接受窗口；会话期间常驻的断开入口；无人值守是单独、需显式开启的模式 |
| **magic-wormhole**（[文档](https://github.com/magic-wormhole/magic-wormhole)） | MIT | 无“被控端确认”概念；用 SPAKE2（PAKE）让**服务器不知道的**短码派生共享密钥，码一次性使用，中间人只有一次猜测机会；可选 `--verify` 让双方比对校验串 | — | PAKE 对 airhand 不适用（见下“未采纳项”）；但“一次性码”与“双方比对校验串（SAS）”的思路可借用 |

**许可注意**：RustDesk 是 AGPL-3.0，而本 fork 是 MIT：**只参考其设计思路与文档化行为，不复制、不改写其源码**；实现以 airhand 自身协议与代码为准。

### 选定基线：RustDesk 的 `click` 确认模型，落在 airhand 已有的 Android 确认机制上

理由：RustDesk 与 airhand 同品类（开源、中转 / 打洞 + 控制码 / 密码、被控端常驻）；其“授权前不发主机信息 / 画面、逐连接确认、连接管理窗口随时断开、未授权阶段资源受限”正是本需求要的。而且 airhand **已有同构机制**：`HostMsg::AwaitingApproval`、`ByeReason::Declined` / `Unanswered`、Android 被控端 60 秒超时（`android/native/src/host/session.rs` 的 `APPROVAL_TIMEOUT`）、`Problem::Declined` / `Unanswered` 为致命（控制端不自动重连）。所以 Phase 1 是把这套机制**扩展到桌面被控端**（现状：桌面被控端不请求同意，`e2e.rs` 里甚至断言不会出现 `AwaitingApproval`），并让它成为直连升级的前置条件，而不是另造协议。

**沿用**：被控端确认模型与超时即拒；授权前不发 `Hello` / 画面；确认窗口显示请求者身份与权限勾选（AnyDesk / RustDesk）；会话期间常驻指示与一键断开；一次性语义（CRD）；周期性续期提示（CRD，可选，默认按钮为“停止”）。
**差异**：
1. RustDesk 在登录**之前**就做打洞与地址交换；本设计**确认之前不交换任何地址**，也不启动 helper、不联系 tailcat / DERP（见 I16）。
2. 确认发生在**中转的会话控制流**上（会话三条流已配对，但只有 `AwaitingApproval`），而不是在直连上。
3. 权限勾选只做 Phase 1 最小集：输入控制、剪贴板、声音、**允许直连**（含“对方将获知本机公网 / 内网地址”的明示）。
4. 无人值守（RustDesk 的 `password` 模式、AnyDesk 的 Unattended Access）**不是**本特性的默认；须运营者显式设 `approve = "never"`。与 r4 草案不同：该模式**允许直连**，条件是同时显式开启 `[direct] enabled`（两项配置一并视为对暴露地址的同意，见 I21）。

### 握手（确认发生在升级之前，且以确认结果为准）

1. 控制端用控制码经中转建立会话，打开三条流，发 `ControllerMsg::Hello`（现有）。此时**不发 `DirectRequest`，不启动 / 联网 helper**。控制端会紧接着发 `Settings` 等消息；被控端在等待确认期间的处理见 I15（记下最新值、确认后生效，与 Android 一致）。
2. 桌面被控端按 `approve` 分流：
   - **`approve = "ask"`（新默认，对每个新控制端会话生效，含纯中转）**：收到后**不发 `HostMsg::Hello`、不发画面**，先发 `HostMsg::AwaitingApproval`。为取到设备标识与直连能力位，先等待 `ControllerMsg::Hello`，默认 2 秒（与 Android 的 `HELLO_WAIT` 相同；超时仍弹出确认窗，把对端当作陌生设备，且不显示“允许直连”）。确认窗口显示控制端**自报**的设备名（明确标注“自报，不可信”）与**经中转看到的**来源 IP（现有 `Peer` 只有这两项，没有 OS 字段，不显示 OS），带勾选项：输入控制（默认开）、剪贴板（默认关）、声音（默认关）、**允许直连（默认关；说明：对方将获知本机公网 / 内网地址，并经第三方 DERP 引导）**，以及按钮“允许”“拒绝”。“允许直连”勾选**仅当本端 `[direct] enabled` 且已收到 `ControllerHello.direct` 为真时才显示**。**当存在已授权且带“输入控制”的在途会话时，不显示“允许直连”勾选与“允许”按钮，新请求按拒绝处理**（防止远端用注入输入替用户点确认，见下“防注入”）。
   - **`approve = "never"`（须显式开启）**：不弹确认窗、不发 `AwaitingApproval`，立即按 I21 发出带 `granted` 的 `HostMsg::Hello` 并开始画面（`granted.direct` 还须对端 `ControllerHello.direct` 为真，见 I21）。托盘可显示时给出醒目警告。
3. **超时默认拒绝**：`approval_timeout_secs` 默认 60（与 Android 现状一致），超时发 `Bye(Unanswered)`；点拒绝发 `Bye(Declined)`；这两种原因对控制端是**终止性**的（代码里的 `Problem::is_final()`，注意不是 `is_fatal()`：后者只对 `Incompatible` 为真；控制端不自动重连，现有行为；但这只是客户端自觉遵守，恶意客户端可无视，所以仍需下面的滥用防护）。排队中的请求，60 秒计时从**入队时**开始；控制端在等待期间离开时，确认窗 / 排队项立即撤下（`prompt_withdrawn_when_controller_leaves`）。等待期间被控端照常回 `Pong`（现有）。
4. 点“允许”后：被控端发 `HostMsg::Hello`（新增字段：本次授予的权限集），开始画面。**授权记忆（桌面为新设计）**：现有语义只在 Android 上（`trusted` 只记设备、不记权限，任何 `HostCmd` 都清空）。桌面端按设备保存**当前**权限集，同一次运行内同设备断线重连可沿用（设备标识是控制端自报、经中转明文转发，中转本来就能重放，沿用即承认这一点）。**清除规则**：任何被控端主动断开（停止共享、断开此控制端、KickAll）、权限被收回到全无、会话结束超过 N 分钟（默认 10，可配）都清除该设备记忆；沿用的是收回后的权限而非最初授予的。**以上授权记忆仅适用于 `approve = "ask"`**。在 ask 模式下，**“直连”权限不随记忆复用**，每次新会话都要重新确认。`approve = "never"` 不使用授权记忆：每个新会话按 I21 赋予 `granted`（控制码仍然有效时，被“断开此控制端”的持码人可以再连回来，并再次获得直连）。never 模式下要阻止持码人再接入，只能：停止共享（更换控制码）、关闭 `approve = "never"`，或关闭 `[direct] enabled`（后者只阻止直连，不阻止中转接入）。本规格不引入“本次运行的拒绝表”；若以后要加，属会话策略变更，须项目负责人另行决定。
5. **仅当已取得直连同意**（ask：用户授予“直连”；never：运营者已同时开启 `never` 与 `[direct] enabled`，且对端 `ControllerHello.direct` 为真）：控制端在收到带 `granted.direct = true` 的 `Hello` 后才创建 helper dial 实例并发 `DirectRequest`；被控端对未取得直连同意的 `DirectRequest` 一律丢弃并回 `DirectDecline`（I12 / I16）。helper 的任何网络活动（拉 DERP 图、连 DERP）都在**取得直连同意之后、且（对控制端）`DirectOffer` 到达之后**才发生。控制端在 Offer 之前为取 `Client.PublicKey` 而创建 helper 实例时，是否已启动 netmon / 拉取 DERP 图，列为 Phase 0 核实项（`controller_helper_no_network_before_offer`）。
6. **权限可随时收回**：被控端窗口随时取消某项勾选，立即生效并通知控制端（`HostMsg::PermissionsChanged`）：关输入 ⇒ 被控端丢弃后续输入；关直连 ⇒ 立即回退中转并销毁 helper / tailcat 实例（不换码），**且本会话不再按 §2 自动重试升级**。在 `approve = "ask"` 下，收回直连后的新会话须重新勾选；在 `approve = "never"` 下，收回只约束本会话，对端以新会话重连时仍按 I21 授予直连（见上，阻止再接入须换码或改配置）。
7. **校验串（SAS，可选借用 magic-wormhole 的 `--verify` 思路；项目负责人 2026-11-10 决定：只显示、不强制比对）**：升级时双方各自从 `(epoch, tailcat 地址, quic_fp, token, client_key)` 的 transcript 哈希派生一个短校验串，同时显示在被控端**指示条**与控制端界面（Offer 产生于确认之后，确认窗里无法显示）。用户可口头比对，**不一致就点断开**。它能发现“主动作恶的中转替换 Offer”，但**不是强制步骤**（只显示，不阻塞升级，也不要求双方输入或确认一致）。只显示哈希截断后的短串，计算过程与中间值不进日志、不进 `Debug` 输出（I7）。登录屏 / 锁屏没有指示条时，本机一侧无法显示 SAS，控制端仍显示。

### 主动断开（被控端与控制端都随时可用）

- **被控端的常驻指示**：会话期间显示常驻的、不可被远端画面遮盖 / 关闭的指示（托盘 + 置顶小条，类似 CRD 的共享提示条与 AnyDesk 常驻窗口），写明控制端设备名、当前路径（中转 / 直连）、已授予的权限，并提供两个一键按钮：
  - **“停止共享”（默认主按钮）**：结束**所有**会话，**同时关闭中转与直连两条路径**（经中转发 `Bye(CodeChanged)`，即现有 `HostCmd::NewCode` 的实际行为与控制端文案；销毁每个会话的 tailcat `Server`、令牌与直连连接；结束 helper 进程），并**更换控制码使旧码立即失效**（`NewCode` 语义 + 直连清理）。
  - “断开此控制端（保留控制码）”：只结束选中的会话（两条路径，发 `Bye(Kicked)`），控制码不变——供固定控制码的常驻主机。**现有代码没有按会话踢人的命令**（`KickAll` 是对所有会话广播），需新增 `HostCmd::Kick { session_id }`，Windows 上对应新增 `ToAgent` 消息；现有托盘的“断开所有控制端”（`KickAll`）保留。在 `approve = "ask"` 下，被断开的设备记忆同时清除，不能免确认回来。在 `approve = "never"` 下没有确认步骤，持码人可以再连回来（见握手第 4 步）。
  - **指示条只能收回、不能扩大权限**；在 `approve = "ask"` 下，要增加权限只能在**新会话的确认窗**里勾选。
- **控制端的主动断开**：新增 `ControllerMsg::Bye`（经中转控制流发送，I10），随后关闭直连连接并结束 helper；被控端收到后清理该会话的全部直连资源。`ControllerMsg::Bye` 定为**无字段**（避免把站在被控端角度的 `ByeReason` 套给控制端）。界面上的“断开”沿用现有 `Command::Disconnect`（现状：先发 `Input(ReleaseAll)`、等 150 ms、再结束任务，**不发任何 Bye**），顺序改为：`ReleaseAll`（当前通道）→ 经中转发 `Bye` → 关闭直连 → 结束 helper；被控端收到 `Bye` 时本地调用 `release_all()`。
- **周期性续期（可选；项目负责人 2026-11-10 决定：默认关闭）**：`reconfirm_minutes`（0 = 关，默认 0）；开启后每 N 分钟在被控端弹出“是否继续共享”，**默认按钮为“停止共享”**，超时无应答视为停止（参照 CRD）。
- 断开必须在界限内完成（默认 2 秒内两条路径都不再收发媒体，由 Phase 0 / 实现阶段校准）；断开后任何迟到的直连连接 / 令牌一律拒绝（I5、I13）。

- **取消安全**：会话任务可能在任意 await 处被取消（例如 `NewCode` 返回时丢弃 `JoinSet`）。helper 清理必须放在 `Drop` 一类取消安全路径上：helper 用 `kill_on_drop`，并在 stdin 读到 EOF 时自行退出；Windows 用已有的 Job `KILL_ON_JOB_CLOSE`，Linux 用 `PR_SET_PDEATHSIG`（`aborted_session_task_kills_helper`）。

### 防注入（确认不能被远端代点）

控制端拿到“输入控制”后能向被控端注入键鼠，**不能让它替用户点自己的确认窗 / 续期提示 / 指示条，也不能靠注入输入改配置，把自己变成“运营者”**。措施：（1）指示条只能收回权限（上节）；（2）确认窗与续期提示在有已授权且带输入权限的在途会话期间，不显示“允许直连”与“允许”（新请求按拒绝处理）；（3）Windows 上确认窗 / 配置确认可过滤注入输入（低层钩子的 `LLMHF_INJECTED` / `LLKHF_INJECTED`，行为需在 Phase 0 / 实现阶段验证）；macOS / Linux 能否区分注入输入**未证实**，做不到就只靠（2）与下节“配置的读取与生效”；（4）放宽 `approve` / `[direct] enabled` 的配置改动，在有带输入权限的在途会话时不生效（见下节）。

### Windows 安装版的确认窗位置与授权通道

Windows 安装版的被控端是控制台会话里的 SYSTEM agent，界面在托盘进程，二者经命名管道通信（`winsvc/`；管道允许同会话交互用户进程连接，`ToAgent` 现仅有 `KickAll` / `NewCode` / `SetAutostart` / `SaveDevices`）。r4 定：确认窗放在托盘进程；新增 `ToAgent::Approve { session_id, granted }` / `ToAgent::Decline { session_id }`；agent 在收到前校验管道客户端进程为本安装目录下的 `airhand.exe`（如实说明：同用户攻击者不在防护范围内，其本来就能控制该用户会话）。**`approve = "ask"` 时，登录屏 / 锁屏没有托盘，现架构下没有可显示确认窗的路径 ⇒ 确认超时拒绝**（结论，不再是“未证实”）；是否能在 agent 内于安全桌面弹窗，列为可选的后续验证项，不在 Phase 1 范围。**`approve = "never"` 时不依赖确认窗**，登录屏 / 锁屏下会话仍可按 I21 建立；若同时开启了 `[direct] enabled`，直连升级也可进行。**此期间没有本机指示条、不能在本机一键停止共享或收回权限、也不能显示 SAS**（I22 的例外）：本机侧的停止 / 换码须等出现可显示托盘的会话，或运营者在本机用其他途径改配置 / 结束进程；远程侧仍可通过控制端断开结束该会话。不采用“无法显示指示条就不升级直连”（那会改变无人值守直连的默认可用性，须项目负责人另行决定）。若 Phase 0 证实 Windows SYSTEM 被控端无法运行受限 helper，则按已有退路禁用该被控端上的直连，never + 直连在 Windows 安装版被控端上同样不可用（写入 r6）。

### 配置

| 键 | 默认 | 说明 |
|---|---|---|
| `[host] approve` | `"ask"` | `ask`：每个新控制端会话都要被控端确认（超时即拒），含纯中转会话；`never`：**无人值守**（不弹确认），须显式开启，托盘显示醒目警告。该模式**允许直连**：若同时显式开启 `[direct] enabled`，则 `granted.direct` 为真，运营者这两项配置视为对暴露地址的同意（I21）；只开 `never` 而不开 `[direct] enabled` 时不交换地址 |
| `[host] approval_timeout_secs` | `60` | 与 Android 现状一致 |
| `[host] reconfirm_minutes` | `0` | 周期性“是否继续共享”，0 = 关 |
| `[host] max_pending_approvals` | `2`（服务器按被控端每次的会话槽位默认 4，客户端拿不到服务器的上限；该值须小于槽位数，避免待确认的会话挤掉已授权的会话） | 同时等待确认的会话数上限；超出的立即回 `Bye(Busy)`（`ByeReason` 末尾新增 `Busy`）；同一时刻只弹一个确认窗口，其余排队 |

### 配置的读取与生效

`[host] approve` 与 `[direct]` 在进程启动时读取，并在配置文件变更时重新读取（现有桌面端会跟踪 `config.toml`）。文件位置：Windows 在安装目录（被控端是 SYSTEM 服务，托盘进程没有写该目录的权限，改配置须管理员）；macOS / Linux 在用户配置目录。

对在途会话与防注入的规则：

- **收紧立即生效**：关闭 `[direct] enabled`，对已有直连会话立即回退并销毁 helper（与“发布与回滚”一致）；把 `approve` 从 `never` 改为 `ask`，已建立的会话继续到结束，但新会话须确认。
- **放宽延后生效**：把 `approve` 改为 `never`，或在 `never` 下打开 `[direct] enabled`，若当时存在已授权且带“输入控制”的会话，**改动不立即生效**（避免远端用注入输入改配置、免确认并取得直连）。待这些会话全部结束后，须本机再确认一次（提示受“防注入”约束）才生效；没有在途输入会话时，仍须本机确认后生效。用例：`config_change_to_never_during_input_session_not_applied`；手动：`enable_never_prompt_rejects_injected_input`。

### 滥用与资源（参照 RustDesk 的未授权连接限制）

- 控制码是 6 位数字且猜码会引发弹窗；RustDesk 同样限制未授权连接的总数 / 单地址数。airhand：被控端**本地**按来源 IP 限速并设**全局**弹窗速率上限（设备标识可被随意更换，不能只按设备；服务器不改，**不能**“计入服务器猜码限速”——服务器只在控制码不存在时按 IP 记失败，带重连凭证的请求不限速），超限后**立即**发 `Bye(Declined)` 并关闭会话、不弹窗（不得“不回应等超时”，那会长时间占用会话槽位）。注意：**拒绝不会让控制码失效**，被拒的控制端仍持有重连凭证，能阻止持码人的办法是“停止共享 / 更换控制码”（写入 `docs/security.md`）。
- 授权前阶段（`AwaitingApproval`）见 I15：只处理 `Hello` / `Ping` / `Bye`，`Settings` / `Audio` / `SelectDisplay` 只记下最新值、确认后生效，其余一律丢弃；授权前消息大小设上限（参照 RustDesk 的 128 KiB 预授权上限，airhand 具体值实现时定）。
- 确认窗口里显示的对端自报文字（设备名等）必须做长度限制与控制字符 / 双向字符清理，且标注“自报”。

### 未采纳项（及理由）

- **PAKE（SPAKE2）绑定控制码**：magic-wormhole 的前提是短码**不为服务器所知**；airhand 的控制码是服务器分配并保存的标识，中转本来就知道它，因此对中转无法提供 PAKE 的保护。若将来要对恶意中转提供密码学保证，需要“服务器不知道的第二段口令”（例如 6 位公开码 + 控制端额外输入的口令词），这属于单独的特性与评审（列为后续扩展点）。
- **RustDesk 的 `password-click`（口令与点击二选一）**：默认路径要求**每次都点击确认**（控制码相当于“口令”，不能替代点击），所以不采用“有口令即免点击”的分支；无人值守须显式设 `approve = "never"`，且仅在同时开启 `[direct] enabled` 时允许直连（I21）。
- **在登录屏 / UAC 安全桌面上的无人值守确认**：见风险。

## 非目标

- 不修改中转服务器**源码**（`server/`）；但需要用 v10 重新构建并部署（见“发布与回滚”）。不改变 6 位控制码与重连凭证机制。
- Phase 1 **不做** Android 与网页控制台（`web/`）的直连行为；它们列为 Phase 1.5 TODO（它们的 Rust 代码仅需为新协议字段通过编译）。Phase 1 只覆盖桌面客户端（Windows / macOS / Linux）两端互连。
- **不实现 Tailscale 与 ZeroTier**（只在“后续阶段”写扩展点）；不接入任何 Tailscale 控制面 / 账号 / tailnet。
- 不使用 tailcat 的 ssh / files / exec / exit-node / socks 功能；不提供“把 airhand 当通用隧道”的能力。
- 不实现多路径聚合 / 同时多条直连；不做额外端口映射逻辑（magicsock 自带的除外）。
- 默认开启的无人值守直连不在本特性内（须运营者显式同时设置 `approve = "never"` 与 `[direct] enabled`）。“已信任设备”的跨运行持久记忆、PAKE / 服务器不知道的第二口令（见“未采纳项”）不在本特性内。同一次运行内同设备断线重连的授权记忆仅适用于 `approve = "ask"`，且直连权限不随记忆复用（见握手第 4 步）。
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
- **I12** `DirectOffer` 只在**已取得直连同意**且 `HostHello` 已发之后发送。被控端的直连同意来源只有两种，且都必须对端 `ControllerHello.direct` 为真：（a）`approve = "ask"`：被控端用户在该会话确认窗中授予“直连”权限；（b）`approve = "never"` 且 `[direct] enabled = true`：运营者显式配置视为同意（I21）。控制端以本端 `[direct] enabled` 为对暴露自身地址的同意。此前收到的、以及未取得直连同意时收到的 `DirectRequest` 一律丢弃（后者回 `DirectDecline`）。
- **I13** 提交由被控端裁决；迟到的 `DirectRequest` / 接受 / 连接（`epoch` 已过期或放弃）一律忽略；任一端经中转收到对端的 `DirectFallback` 立即回退；控制端等不到 `DirectCommit` 超时即回退。
- **I14** `client_key` 为空或无法解析时，Rust 侧与 helper 都拒绝 Start（不得起一个放行所有客户端的 `Server`）；helper **不调用** `AddAllowedClient`（运行时追加会重新引入“列表为空即放行”的窗口）。
- **I15** 桌面被控端 `approve = "ask"`（默认）时，每个新控制端会话在用户确认前：**不发 `HostMsg::Hello`、不发画面 / 声音**，只发 `AwaitingApproval`；只**处理** `ControllerMsg::Hello`（取设备标识与直连能力位）/ `Ping` / `Bye`；`Settings` / `Audio` / `SelectDisplay` **只记下最新值，确认后生效**（与 Android 一致，避免回归）；输入、剪贴板、`Ack`、`Direct*` 一律丢弃。**超时（默认 60 秒）即拒绝**（`Bye(Unanswered)`），点拒绝为 `Bye(Declined)`；二者对控制端是终止性的（`Problem::is_final()`，不自动重连）。`approve = "never"` 不适用本条，见 I21。
- **I16** 取得直连同意之前：不交换任何地址 / 令牌 / 密钥，被控端不启动 helper，控制端 helper 不联网（不拉 DERP 图、不连 DERP）；未取得直连同意的会话永不启动 helper。同意的判定与 I12 相同。
- **I17** 确认窗口显示的对端自报文字（设备名等）有长度上限并清理控制 / 双向字符，且标注“自报”；同时等待确认的会话数有上限；同一来源被拒绝 / 无应答超限后静默拒绝，不再弹窗。
- **I18** **停止共享**：结束所有会话的**中转与直连两条路径**（`Bye(CodeChanged)` 经中转；销毁每个会话的 tailcat `Server`、令牌、直连连接；结束 helper 进程）并**更换控制码使旧码立即失效**；之后迟到的直连连接 / 令牌 / 旧码一律拒绝。被控端“断开此控制端”结束选中会话的两条路径（`Bye(Kicked)`），不换码；在 `approve = "ask"` 下被断开的设备须重新确认才能回来，在 `approve = "never"` 下持码人可以再连回来。
- **I19** 控制端主动断开：经中转发无字段的 `ControllerMsg::Bye`，随后关闭直连连接并结束 helper；被控端收到后清理该会话的全部直连资源。控制端的断开入口在会话期间始终可用（含升级进行中与直连状态）。被控端的本机断开入口在指示条 / 托盘可显示时始终可用；登录屏 / 锁屏等没有指示条时，本机侧不适用，见 I22。
- **I20** 权限由被控端强制执行（每项在两个方向上都要有执行点）：关闭“输入控制”后丢弃后续输入；关闭“剪贴板”后既不应用控制端发来的内容、也不再向控制端推送（桌面端现状这两处都不受条件限制，需新增）；关闭“声音”后取消订阅；收回“直连”权限立即回退中转并销毁 helper / tailcat 实例（不换码），且本会话不再自动重试升级；权限变化经 `HostMsg::PermissionsChanged` 通知控制端。
- **I21** `approve = "never"` 是运营者显式开启的无人值守模式：不弹确认窗、不发 `AwaitingApproval`，会话立即发 `HostHello` 并开始画面。`HostHello.granted` 取值：输入 / 剪贴板 / 声音沿用现有行为（全部为真）；**`direct` 当且仅当同时显式开启 `[direct] enabled` 且对端 `ControllerHello.direct` 为真时为真**。运营者显式同时开启这两项配置，即视为对“暴露本机公网 / 内网地址并经第三方 DERP 引导”的同意，无需再逐会话勾选（此为 r4「never 禁用直连」的修订）。只开 `never` 而不开 `[direct] enabled`，或对端未声明直连能力时，`granted.direct` 为假，不发 `DirectOffer`、不启动 helper。该模式须显式开启，托盘可显示时给出醒目警告（说明：持码人可在无人确认的情况下接入；若直连已启用，还将获知本机地址）。放宽这两项配置的生效规则见“配置的读取与生效”。
- **I22** 被控端在指示条 / 托盘可显示时，会话期间的常驻指示不能被远端画面关闭或遮盖，显示控制端设备名、当前路径（中转 / 直连）与已授予权限，并可一键停止共享或收回权限；指示条只能收回权限；确认窗 / 续期提示 / 配置确认不能被远端注入输入代点（见“防注入”）。**例外**：登录屏 / 锁屏等没有托盘与指示条时，本机无法显示指示、SAS 与一键停止（never 模式下会话与直连仍可按 I21 进行）；此时只能由控制端断开，或等本机出现可显示的会话 / 运营者在本机用其他途径换码或改配置。**显示层行为无法自动化，验收归入手动项。**
- **I23**（实现位置）确认闸门状态机（`ask`：等 `Hello` → 询问 → 放行 / 拒绝、授权前缓存、授权记忆；`never`：跳过询问、按 I21 赋予 `granted`；权限集）下沉到 `airhand-link`，桌面与 Android 共用；Android 现有实现迁移为共用逻辑，避免两份逻辑走样。

## 验收测试清单

测试命名采用 Rust `#[test]` / `#[tokio::test]` 函数名与 Go `Test…` 名；文件为**将要创建**的路径，实现阶段按“先红后绿”提交。
验证面均为**本机自动化**（回环 + 假 helper / 可注入路径源）；真实 NAT / 跨公网表现只能在**线上手动验证**。托盘 / 被控端会话逻辑位于 `client/src-tauri`，相关测试放在该 crate，不放在 `airhand-link`。

- [ ] I4：`crates/airhand-proto/src/framing.rs` →「`version_handshake_wire_is_stable`」（既有，必须仍通过）；`crates/airhand-proto/src/lib.rs` →「`protocol_version_is_10`」；`crates/airhand-proto/src/session.rs` →「`direct_messages_wire_is_stable`」。
- [ ] I1：`crates/airhand-link/tests/direct.rs` →「`disabled_by_default_never_spawns_helper`」；预期：假 helper 工厂 0 次调用、无任何 `Direct*` 消息、无到 tailcat/DERP 的连接尝试。
- [ ] 配置：`client/src-tauri/src/config.rs` 单测 →「`direct_config_defaults_off`」「`direct_config_rejects_non_https_derpmap`」「`direct_config_has_no_helper_path_option`」。
- [ ] 升级：`crates/airhand-link/tests/direct.rs` →「`session_upgrades_to_direct_when_both_sides_enable`」；预期：升级后画面包经直连流到达、中转流字节计数不再增长。「`peer_without_direct_enabled_stays_on_relay`」。
- [ ] I8：「`unknown_offer_kind_is_declined`」。
- [ ] I12 / I16：「`no_offer_before_host_hello`」「`request_before_host_hello_is_dropped`」「`no_offer_before_approval`」「`direct_request_without_direct_grant_is_declined`」「`helper_not_spawned_before_approval`」（假 helper 工厂：取得直连同意前 0 次调用，且控制端 helper 无网络活动）「`declined_session_never_spawns_helper`」「`ask_mode_no_offer_without_direct_checkbox`」「`controller_helper_no_network_before_offer`」。
- [ ] I15（确认 / 拒绝 / 超时）：`client/src-tauri` 内「`ask_mode_sends_awaiting_approval_and_no_hello`」「`no_frames_or_audio_before_approval`」「`non_hello_ping_bye_messages_cached_or_dropped_while_awaiting`」（原名 `non_ping_messages_dropped_while_awaiting`，与 I15 对齐：`Settings` 等记下最新值，输入 / 剪贴板 / `Direct*` 丢弃）「`approve_then_hello_with_granted_permissions`」「`decline_sends_bye_declined`」「`approval_timeout_sends_bye_unanswered`」（注入时钟，默认 60 秒）「`pong_still_answered_while_awaiting_approval`」「`declined_and_unanswered_are_fatal_no_auto_reconnect`」（`airhand-link`）「`same_device_reconnect_reuses_grant_in_same_run`」「`prompt_waits_for_controller_hello_then_hides_direct_if_missing`」；并更新 `client/src-tauri/src/e2e.rs` 里“桌面被控端不会请本机用户同意”的断言（改为按 `approve` 模式断言）。
- [ ] I15 补充：「`pre_approval_settings_applied_after_approval`」「`pre_approval_input_clipboard_direct_dropped`」「`controller_never_requests_without_direct_grant`」「`approved_without_direct_never_spawns_helper_either_side`」「`prompt_hides_direct_option_when_direct_not_enabled`」「`prompt_withdrawn_when_controller_leaves`」「`queued_request_timer_starts_at_enqueue`」。授权记忆（仅 ask）：「`kicked_device_must_reapprove`」「`reused_grant_reflects_revocations`」「`direct_grant_not_reused_on_reconnect`」「`grant_memory_cleared_on_host_initiated_disconnect`」。
- [ ] 防注入 / Windows：「`indicator_cannot_grant_permissions`」「`prompt_hides_direct_option_while_remote_input_active`」「`config_change_to_never_during_input_session_not_applied`」；手动：「`injected_input_cannot_approve_prompt`」（Windows，低层钩子过滤）；「`enable_never_prompt_rejects_injected_input`」「`tray_ipc_approve_requires_installed_airhand_exe`」「`no_tray_at_lock_screen_times_out_to_reject`」（ask）；「`never_direct_at_lock_screen_has_no_local_indicator`」（never + 直连，本机无指示条，控制端仍可断开）。
- [ ] I17（滥用与资源）：「`approval_prompt_text_is_sanitized_and_length_limited`」「`pending_approvals_over_cap_get_busy`」「`repeated_declines_from_same_source_are_silently_rejected`」「`unauthorized_message_over_limit_closes_session`」「`over_rate_limit_replies_declined_immediately`」「`declined_does_not_invalidate_code`」。
- [ ] I18（停止共享）：「`stop_sharing_ends_relay_and_direct_paths`」「`stop_sharing_rotates_code_old_code_rejected`」「`stop_sharing_destroys_tailcat_server_token_and_helper`」「`late_direct_connection_after_stop_is_refused`」「`disconnect_one_controller_keeps_code_and_closes_both_paths`」「`stop_sharing_during_upgrade_ends_both_paths`」「`disconnect_completes_within_bound`」「`aborted_session_task_kills_helper`」「`kick_session_command_ends_only_that_session`」「`stop_sharing_sends_code_changed`」。
- [ ] I19（控制端主动断开）：「`controller_bye_closes_direct_and_kills_helper`」「`controller_bye_sent_on_relay`」「`controller_disconnect_during_upgrade_cleans_up`」。
- [ ] I20 / I21 / I22：「`revoking_input_permission_drops_input`」「`revoking_direct_permission_falls_back_and_destroys_helper`」「`permissions_changed_notifies_controller`」「`revoking_clipboard_stops_both_directions`」「`revoking_audio_unsubscribes`」「`approve_never_skips_prompt_and_sends_hello`」「`approve_never_allows_direct_when_enabled`」「`approve_never_direct_false_when_direct_disabled`」「`approve_never_operator_config_counts_as_consent`」（`never` + `[direct] enabled` 且对端声明直连时不弹窗即可发 `DirectOffer` / 启动 helper）「`approve_never_direct_false_when_peer_lacks_direct`」「`approve_never_shows_warning_state`」「`never_mode_kicked_device_can_return_until_code_changed`」「`revoked_direct_not_retried_in_session`」「`disabling_direct_falls_back_inflight_sessions`」「`session_indicator_shows_path_and_permissions`」「`reconfirm_defaults_off`」「`reconfirm_prompt_defaults_to_stop_and_times_out_to_stop`」（`reconfirm_minutes > 0`）。
- [ ] 协议兼容：「`old_frontend_ignores_unknown_events`」「`approval_new_variants_wire_is_stable`」。
- [ ] SAS（只显示、不强制比对）：「`verification_string_matches_on_both_ends`」「`verification_string_changes_if_offer_is_substituted`」「`verification_string_not_logged`」「`verification_string_is_displayed_not_required`」（升级不因用户未比对而阻塞或失败）。
- [ ] 手动：Windows 登录屏 / UAC 安全桌面下确认窗口能否显示与点击（不能则按超时拒绝）；macOS / Linux 的置顶指示与托盘行为。
- [ ] I13：「`late_request_after_host_timeout_is_ignored`」「`host_fence_then_commit_is_first_direct_message`」（被控端先在中转发 `DirectSwitch`，再在直连发 `DirectCommit` 作为第一条）「`commit_lost_controller_falls_back_within_bound`」「`fallback_message_forces_peer_to_fall_back`」「`asymmetric_direct_failure_recovers_within_bound`」。
- [ ] I2 / 存活：「`static_screen_does_not_trigger_fallback`」（无画面无声音数十秒，Ping/Pong 正常 ⇒ 不回退）「`missing_pong_triggers_fallback`」「`direct_failure_falls_back_to_relay`」「`helper_crash_does_not_end_session`」「`direct_stream_failure_does_not_end_session`」「`relay_loss_ends_session_even_when_direct`」「`direct_write_failure_requeues_on_relay`」。
- [ ] I3（Rust 侧）：「`upgrade_timeout_keeps_relay`」「`no_switch_before_both_gates_open`」。I3（Go 侧，helper 本身）：`p2p/gate_test.go` →「`TestGateDropsWhenPathNotDirect`」（可注入路径源）「`TestControllerGateIgnoresSlowButDirectPath`」（RTT 大于 250 ms 且 ≤ 400 ms 的直连不被误判，与阈值表对齐）「`TestHostGateClosedWhenCurAddrEmpty`」「`TestHostGateWindowIncludesStatusPollInterval`」；另附**手动 / 可选 CI**：用本地自建 `derper` 验证真实 DERP 退化行为与窗口时长。
- [ ] I5：「`direct_token_is_one_time`」「`direct_token_wrong_session_rejected`」「`direct_token_expired_rejected`」「`direct_token_wrong_epoch_rejected`」；证书固定：「`direct_cert_pin_mismatch_aborts_upgrade`」。
- [ ] I6：「`input_order_preserved_across_switch`」「`fence_timeout_drops_buffer_and_releases_keys`」「`late_direct_input_after_fallback_is_discarded`」「`keys_released_after_fallback`」「`settings_resynced_after_fallback`」「`host_initiated_fallback_discards_inflight_direct_input`」「`bye_on_relay_processed_after_fence`」「`expired_input_not_replayed_on_relay_after_fallback`」「`commit_after_own_fallback_is_ignored`」「`controller_resends_cached_settings_audio_display_after_fallback`」「`duplicate_frames_across_lanes_are_dropped`」「`stale_lane_frames_before_keyframe_are_dropped`」「`audio_not_replayed_after_switch`」。
- [ ] I11：「`direct_quic_never_sends_datagram_over_1232`」（行为测试：假 helper 记录并丢弃超限数据报，断言观测到的最大数据报 ≤ 1232）。
- [ ] I7：「`direct_secrets_are_redacted_in_debug_and_logs`」（`Secret<T>`）；Go「`TestTailcatLogfDiscardedByDefault`」。
- [ ] I9 / I14：Go `p2p/helper_test.go` →「`TestServesExactlyOneUDPPort`」「`TestNoTCPServiceAndNoRST`」（SYN 超时而非 RST）「`TestOnlyAllowedClientKeyConnects`」「`TestEmptyOrInvalidAllowlistRefusesToStart`」「`TestMultipleServersInOneProcessAreIndependent`」（不通过则改为每 epoch 一进程）；Rust：「`empty_client_key_refuses_start`」（I14 单列，不并入 I9）；若采用候选 (i) →「`direct_listener_binds_loopback_only`」「`direct_listener_accepts_single_connection`」「`direct_listener_uses_exclusive_addr_on_windows`」；若采用 (ii) →「`direct_path_opens_no_local_port`」。
- [ ] helper 协议：Go「`TestControlProtocolNeverEchoesAddressToArgvOrStderr`」「`TestDatagramPipeIsSeparateFromControlStdio`」（候选 (ii)）。
- [ ] 并发：「`two_concurrent_sessions_upgrade_independently`」「`session_over_cap_stays_on_relay`」。
- [ ] I10（`client/src-tauri` 内）：「`kick_all_ends_direct_and_relay_lanes`」「`new_code_ends_direct_session`」「`kick_during_upgrade_ends_both_paths`」「`bye_always_sent_on_relay`」。
- [ ] Windows（手动 / 真机，**以 SYSTEM 服务身份**运行，不计入自动通过数）：「`helper_token_is_restricted_on_windows`」「`helper_under_restricted_token_completes_direct_session`」（功能验证：受限令牌下能完成一次直连会话）「`restricted_helper_cannot_open_parent_or_write_install_dir`」（反向验证）：断言用户 SID 与 Administrators 为 deny-only、特权只剩 `SeChangeNotifyPrivilege`、完整性级别 Low、位于带 `KILL_ON_JOB_CLOSE` 与进程数限制的 Job 内（若采用 restricting SIDs 则断言其存在）。
- [ ] 完整性：「`helper_hash_mismatch_disables_direct`」（只验证防误放 / 损坏，不声称防同用户攻击）。
- [ ] 回归：`server/` 源码无 diff（`git diff --stat` 校验）；`cargo test`（根工作区）、`cargo test`（`web/`）、`npm test`（`client/`）、`cargo test`（`android/native`）全部仍通过。
- [ ] 手动（不计入自动通过数，须单独记录）：两台不同网络的真机开启后确认路径为 direct；记录升级耗时、码率、延迟、CPU；对称 NAT 下确认回退；Windows 防火墙行为；macOS 嵌套 helper 签名 / 公证 / 防火墙弹窗；杀毒软件误报情况；Linux 各分发形态（deb / AppImage 等）带上 helper 且卸载干净。

## 阶段与门禁（对应 workflow v2）

| 阶段 | 内容 | 门禁 / 证据 |
|---|---|---|
| 调研 | 上游机制、tailcat 核实 | 已完成：见本文与 feature map |
| 规格 | 本文 + feature map，走分支 + PR | PR #1（Draft） |
| 设计评审 | 需求与架构评审（独立于实现），分级 findings，修订后复审至无严重问题 | [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（各轮报告全文在 [`review/`](review/)） |
| 项目负责人批准 (A) | 本变更属重大改动；spec 与评审都不代替批准。(A)：**仅批准 Phase 0 spike**，范围限定在独立 spike 分支，**不得合并，spike 代码不得直接复用进 Phase 1**（Phase 1 按先红后绿重写） | **已批准（2026-11-10 项目负责人批复）**，阈值同批确认；结论记录在 PR 评论 |
| 工作流脚本 / CI 移植（独立 PR） | 把 workflow v2 的检查（隐私扫描、署名 trailer、spec-requires-tests）移植进本 fork，**单独一个 PR，在 Phase 1 之前完成**；本 PR 不含 | 独立 PR（后续） |
| Phase 0 spike | 实测并产出数据，见下表清单与 go/no-go 阈值 | 结果写入规格 r6 |
| 复审 | **Phase 0 结果 → 规格 r6 → 再做一轮设计复审**直到无 S1 | 追加到评审文档 |
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
9. 中转 RTT 下栅栏等待上限、Commit 等待 T、直连 Ping 间隔与存活检测 K 的校准（K 的形式见 §2；直连通道的写超时按通道区分）。
10. 控制端在发出 / 收到 `DirectOffer` 之前，为取 `Client.PublicKey` 而创建 helper 时，是否已启动 netmon、拉取 DERP 图或产生其他网络活动（规格要求无网络活动）。
11. 在最终选定通道上测 R，估计余量系数 k，给出建议 C，并在码率 C（及可达时接近 100 Mbps）下加测 helper CPU；记录 Windows 受限令牌结论对 never + 直连的影响。

### go/no-go 阈值（2026-11-10 项目负责人已批准；数值须在 Phase 0 报告里逐项对照）

| 指标 | 阈值 |
|---|---|
| helper 额外 CPU（被控端，1080p60、约 20 Mbps 单会话；参考硬件：近三年主流桌面 / 笔记本，在 spike 报告中写明型号级别。高码率 C 下的 CPU 见上节，另测） | ≤ 单核 30% |
| 局域网直连额外往返延迟（相对不经 helper 的同路径） | ≤ 3 ms |
| 单会话持续吞吐（5 分钟，在 QUIC 层统计） | ≥ 60 Mbps，QUIC 层丢包 < 0.1%；产品允许的最高码率为 100 Mbps（`QualitySettings.bitrate_kbps` 上限 `100_000`）。直连码率上限策略见下节（r5 已定策略，上限 C 的数值待 Phase 0 的 R 填入） |
| 路径判定误判（直连稳定且 RTT ≤ 400 ms，30 分钟；与 Go 用例覆盖的 RTT 范围对齐） | 0 次 |
| 存活检测误回退（直连稳定；静止桌面 + Wi-Fi 场景，30 分钟） | 0 次 |
| I3 窗口（自建 `derper`，路径被人为切断后） | 闸门关闭 ≤ 2 s；经 DERP 发出的数据报数量如实记录（上限值在首轮数据后与项目负责人确认） |
| 升级完成时间（p50；起点：控制端发出 `DirectRequest`；参考硬件：近三年主流桌面 / 笔记本，在 spike 报告中写明型号级别） | ≤ 12 s（含 DERP 引导与打洞） |
| Windows 受限令牌 / AppContainer | 至少一种能绑定 UDP 并正常运行；否则走退路（Windows SYSTEM 被控端禁用直连） |
| 任一不达标（Windows 一行除外，Windows 一行有自己的退路，不触发整体放弃） | 降级为“仅低码率档可用”或放弃；该决定属范围变更，须项目负责人单独确认 |

### 直连会话码率上限策略（r5；关闭第 3 轮 S2-3 的策略部分）

- 产品最高码率仍为 100 Mbps，与中转相同；直连**不**单独提高上限。
- **策略：限制码率，不因码率达到上限而回退中转。** 回退仍只由路径判定、存活检测、用户收回“直连”权限或主动断开触发。直连路径沿用现有自适应码率（拥塞下调、恢复上调），但其编码目标码率（`QualitySettings.bitrate_kbps`）不得超过直连上限 C。
- R 与 C 不是同一层的量：R 是 Phase 0 在**最终选定的数据报通道（(i) 或 (ii)）**上测得的 5 分钟 QUIC 层持续吞吐；C 约束的是编码器目标码率。C 必须为声音、包头与重传留出余量，即 C ≤ k×R，系数 k（k < 1）由 Phase 0 根据实测开销给出，不得把 R 直接当成 C。回退到中转后，解除直连上限 C，改回产品 / 中转既有上限（100 Mbps）与既有 ABR。
- 在 Phase 0 给出 R、k 与建议 C 之后写入 r6：
  - 若按上式可得的 C ≥ 100 Mbps：C = 100 Mbps（与产品上限相同，无额外限制）。
  - 若 60 Mbps ≤ C < 100 Mbps：取不超过该值的产品可用码率档，且不低于 60 Mbps；用户选择高于 C 的码率时被限制在 C，并在诊断 / 画质界面提示，不回退。
  - 若可得的 C < 60 Mbps，或 R 本身 < 60 Mbps：不在本策略内自动定 C；触发已批准的 go/no-go（降级为仅低码率档或放弃），须项目负责人单独确认。
- Phase 0 还须在目标码率为 C（以及，若 R 支持，接近 100 Mbps）的条件下加测 helper 额外 CPU，对照已批准的 CPU 阈值（阈值本身仍在约 20 Mbps 参考场景下定义，见上表；高码率下的 CPU 如实记录，超标时按 go/no-go 处理）。
- 本策略不在项目负责人 13:08 的四项决定之内，批准 (B) 时一并确认。验收：「`direct_session_caps_bitrate_instead_of_falling_back`」（在可注入的码率控制下，目标码率高于 C 时被限制在 C，且不发 `DirectFallback`）；「`fallback_to_relay_lifts_direct_bitrate_cap`」。

> 工作流适配说明：`docs/workflow-v2.md` 与 `AGENTS.md` 来自另一个仓库（passkey 演示），其中 `tests/*.mjs`、CI 检查名（`spec-requires-tests`、`agent-trailer`、`privacy-scan`）是那个仓库的实现。
> 本 fork 目前没有这些脚本与 CI（上游也没有 `.github/`；移植正在独立 PR 中进行）。本规格把“验收映射到测试用例名”改写为 Rust / Go 用例名。项目负责人已决定：把检查脚本与 CI 移植进本 fork，作为独立 PR，在 Phase 1 之前完成。

## r4 处理记录（第 3 轮复审的 S2 / S3 与项目负责人批复）

**已在 r4 落实（不依赖 spike 数据）**：S2-1（回退发起方丢弃、栅栏后仍处理的中转消息、“改投中转”范围、新增用例）；S2-2（去掉“< 2 秒”约束、直连 Ping 间隔与 K 的形式、按通道区分写超时、10 秒措辞）；S3-1（`Server.Status()` 轮询计入 I3 窗口）；S3-2（Commit 计时与回退后迟到 Commit）；S3-3（控制端缓存并重发 `Settings` / `Audio` / `SelectDisplay`）；S3-4（Windows 验收改为手动 / 真机 / SYSTEM，补功能与反向验证）；S3-6（选 (i) 的 Windows 独占地址用例）；S3-7（用例归属整理）；以及项目负责人批复新增的“被控端确认与主动断开”（I15–I22）。当时 I21 仍写 `never` 禁用直连，已由 r5 修订。

**当时仍待、现已分流**：码率上限策略的“限制码率还是回退”在 r5 定（见上节）；C 的数值与其余超时 / 阈值、候选通道、Windows 结论等仍待 Phase 0 数据，写入 r6。

## r5 处理记录（开放问题 11–14 与第 3 轮 S2-3 的策略部分）

**项目负责人 2026-11-10 13:08（Asia/Taipei）决定，已写入本版**：

1. 默认每个新会话确认（`approve = "ask"`，含纯中转）。无人值守机器显式设 `approve = "never"` 免确认，且免确认模式**允许直连**。运营者显式同时开启 `never` 与 `[direct] enabled`，视为对暴露地址的同意（I12 / I16 / I21）。这推翻了 r4「never 模式禁用直连」。
2. “停止共享”更换控制码；另提供“断开此控制端（保留控制码）”。
3. 周期性“是否继续共享”默认关闭（`reconfirm_minutes = 0`）。
4. 校验串（SAS）只显示、不强制比对。

**第 3 轮 S2-3 的剩余部分**：已补直连码率上限策略（限制码率、不因达上限而回退；C 由 Phase 0 的 R 填入 r6）。Go 用例 RTT 范围与阈值表对齐、参考硬件措辞已在 r4.1 处理。

**第 5 轮复审（无 S1）后的 r5.1**：已落实其 S2 / S3（never 路径上授权记忆 / 踢出 / 收回的例外、I22 在登录屏 / 锁屏的例外、放宽配置的延后生效与防注入、`granted.direct` 须对端也启用、控制端同意、SAS 显示位置、码率 R/C 换算与回退后解除上限、`ControllerMsg::Bye` 无字段、并发上限措辞、Phase 0 增补项、`HELLO_WAIT`）。未采纳须另行批准的方案：never 模式的“本次运行拒绝表”、以及“无法显示指示条就不升级直连”。

**仍待 Phase 0 实测数据（r6）**：各超时与阈值的取值（升级超时、栅栏上限、T、Ping 间隔、K、M、`Server.Status()` 轮询）、直连上限 C 的确定值（含余量系数 k 与 C 码率下的 CPU）、回环候选 (i) / (ii) 的选择、同进程多 `Server` 是否安全、Windows 受限令牌 / AppContainer 结论及其对 never + 直连的影响、控制端在 Offer 前是否已有网络活动、go/no-go 判定、helper 体积与许可清单。

## 项目负责人批复记录（2026-11-10）

- **批准**：Phase 0 spike（(A)，独立分支，不合并、代码不复用）；全部建议的 go/no-go 阈值；方案 B（Go helper 子进程）与第三方许可声明；默认使用 Tailscale 托管 DERP 做引导；v10 服务器与全部客户端同步升级；Phase 1 仅桌面。
- **新增要求**：被控端必须确认（控制码不等于自动接受，尤其在暴露公网 / 内网地址之前）；被控端要有随时可用的主动断开；握手参考现成方案（见“被控端确认与主动断开”）。
- **工作流脚本 / CI**：移植为独立 PR，在 Phase 1 之前完成（见阶段表）。
- **开放问题 11–14（2026-11-10 13:08 Asia/Taipei）**：默认每会话确认；`approve = "never"` 免确认且允许直连（显式配置视为同意）；停止共享换码、另配只断开此控制端；周期性续期默认关；校验串只显示、不强制比对。见上“r5 处理记录”。

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
- **确认步骤改变桌面被控端的既有行为**：默认 `approve = "ask"` 后，无人值守 / 登录屏主机在不改配置时无法被连接（确认超时即拒）。登录屏 / 锁屏要保持可连接，须显式设 `approve = "never"`；若还要直连，须同时开启 `[direct] enabled`。Windows SYSTEM agent 在登录屏 / 安全桌面上弹确认窗不在 Phase 1 范围（ask 模式超时拒绝）。同时 `e2e.rs` 等既有测试需随之更新。
- **无人值守直连的暴露面**：`approve = "never"` 且 `[direct] enabled` 时，持有控制码的人无需本机有人点确认即可接入，并获知被控端公网 / 内网地址、经第三方 DERP 引导。控制码泄露或被猜中的后果比纯中转无人值守更大。被“断开此控制端”的持码人可以再连回来并再次获得直连。对策：该组合必须两项都显式开启，且放宽配置须本机确认、不在有输入权限的在途会话期间生效；托盘可显示时醒目警告；写入 `docs/security.md`；拒绝不会使控制码失效，运营者须用“停止共享 / 更换控制码”作废旧码，或关闭 `never` / `[direct] enabled`。登录屏 / 锁屏期间没有本机指示条与一键停止（I22 例外）。Phase 0 / 实现不把控制码、地址或密钥写入仓库或日志（I7）。
- **协议版本升级**：v9 ↔ v10 不互通；服务器与所有客户端须同时升级（见“发布与回滚”）。
- **仍需线上验证**：真实跨网络打洞成功率、码率与延迟、DERP 引导可用性——自动测试无法覆盖。

## 开放问题

### 已决定（2026-11-10 项目负责人批复）

1. 集成形态方案 B（自带 Go helper 子进程）与第三方许可声明——**已批准**。
2. DERP 策略：默认使用 Tailscale 托管 DERP 做引导（以及路径退化窗口期内有界地承载少量数据，见 I3）——**已批准**（自建 `derper` 仍为可选配置）。
3. 控制流迁移（栅栏方案）——**随整体批准，按本规格实施**；复杂度由 Phase 0 与复审把关。
4. 批准节奏（分两段）与 go/no-go 阈值——**已批准**，阈值按上表。
5. Phase 1 范围仅桌面——**已批准**。
6. 工作流脚本 / CI——**移植，作为独立 PR 在 Phase 1 之前完成**。
7. fork 保持 MIT；helper 第三方许可（BSD-3 / MIT / Apache-2.0 等）与 README 声明——**已批准**。
8. v10 服务器与全部客户端同步升级——**已批准**。
9. mTLS：不采纳（保持三层限定）；评审未要求。
10. 桌面被控端请求同意：**不再接受默认“无确认”**——改为默认 `approve = "ask"`；`ask` 模式下取得直连同意之前不交换地址（I15 / I16）。
11. **默认确认范围**（2026-11-10 13:08 Asia/Taipei 项目负责人决定）：`approve = "ask"` 对桌面被控端的**每个新会话**（含纯中转）生效，接受由此改变“持码即连”的既有行为。无人值守机器须显式设 `approve = "never"` 免确认，**且免确认模式允许直连**（须同时显式开启 `[direct] enabled`；两项配置视为同意，见 I21）。
12. **“停止共享”与“断开此控制端”**（同日决定）：停止共享更换控制码；另配“断开此控制端（保留控制码）”。
13. **周期性“是否继续共享”**（同日决定）：默认关闭（`reconfirm_minutes = 0`）。
14. **校验串（SAS）**（同日决定）：只显示，不强制比对。

当前规格在开放问题上**没有仍待项目负责人确认的条目**。批准 (B) 与直连上限 C 的确定值仍待 Phase 0 数据与 r6 复审。

## 是否重大改动

**是**。理由：新增监听 / 子进程（权限与安全边界变更）、跨 `proto` / `link` / `client` / 新增 `p2p` 多个子系统、新增第三方运行时依赖、协议版本升级、新增 IP 暴露面。
按 workflow v2「角色与硬约束」：重大改动必须等待项目负责人批准；spec 与设计评审都不代替批准。批准结论记录在 PR 评论中。
