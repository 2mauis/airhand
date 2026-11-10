# tailcat 临时点对点直连（Direct P2P transport）

> 状态：**规格 r6 / r6.1（2026-11-10 项目负责人 14:30 Asia/Taipei 决定：写入 Phase 0 五项前提、码率上限 C、I3 窗口包数上限，以及 never 模式隐私风险；真机测试列为批准 (B) 的前提。r6.1 闭合第 6 轮 S1，并落实针对性复审提出的 S2），未实现**。本文件只描述设计；不含功能代码。Phase 0 数据来自云 VM 报告（`spike/tailcat-p2p-phase0` 分支 `docs/specs/tailcat-p2p-phase0-report.md`，PR #3 head `065c6f7`，保持 Draft，spike 代码不进入 Phase 1），**不是**参考硬件，批准 (B) 前须用项目负责人指定的真机复核。评审记录见 [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（各轮报告全文在 [`review/`](review/)）。
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

核对范围：下表凡未标注者均已对 `v0.7.0` 源码核对（`Server.AllowedClients` / `AddAllowedClient`、`ServedTCPPorts` / `ServedUDPPorts`、`MaxUDPPayload = 1232`、`Client.DiscoPing`、`Server.Status`、`Client.PublicKey`）；标注 `main` 的条目不属于 `v0.7.0`，不得依赖。Phase 0 已实测：`Client.Ping` 对未授权客户端内部超时 10 s（路径判定必须用 `DiscoPing`）；超 1232 字节写入被静默丢弃；直连稳定期间自建 DERP 上数据包与 disco 包均为 0，只看 `CurAddr` 的闸门在静默断路后窗口内会有 0–10 个数据包经 DERP。

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
| UDP | 库提供 `OnUDP` / `DialUDPPort`（`ConnPacketConn`），**单包载荷上限 `MaxUDPPayload = 1232` 字节**（README：超过“不保证送达”；Phase 0 实测：写 1233–60000 字节返回 nil，数据报被静默丢弃，1200 与 1232 能送达）；**CLI 的 `serve` / `forward` 只转发 TCP**（UDP 只在 `exit-node`、socks UDP ASSOCIATE、`main` 上未发布的 `perf` 里出现） |
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
5. 控制端 helper 以 offer 中的地址拨号，并按 §2 的规则检测路径；**仅当路径为 `direct`** 才继续。超时（`upgrade_timeout_secs` 默认 15，下限 12，需覆盖 DERP 图拉取与引导）或只有 DERP 则放弃，会话继续走中转，不报错。
6. **先保证控制端首个直连 QUIC 包不会因被控端闸门未开而丢失**（I24）。Phase 0 实测被控端判定 direct 比控制端晚约 93–111 ms，若控制端在被控端闸门仍关闭时发出 QUIC Initial 且该包被丢弃，只能等约 1 s 的 PTO 重传，升级耗时分成约 0.44 s 与约 1.40 s 两档。本规格采用以下两种方案之一（不得只靠调小 `initial_rtt` / PTO 来掩盖首包被丢）：
   - **方案 (a)（首选）**：被控端闸门打开后经中转发 `HostMsg::DirectGateOpen { epoch }`（`epoch` 须为当前升级；迟到或 `epoch` 不匹配的提示按 I13 忽略）。控制端在本端闸门已打开**且**收到该提示之前，不得发出首个直连 QUIC 包。等不到（时限计入 `upgrade_timeout_secs`）则放弃本次升级、保持中转。
   - **方案 (b)（等效，不是 Phase 1 的默认实现）**：被控端 helper 在闸门关闭时短时缓存入站方向的数据包（而不是丢弃），使控制端可以在被控端闸门打开之前发出首包且该包不被丢失。缓存须有界限：最多 32 个数据包或 64 KiB（以先到者为准），最长存活 `min(rx_silence_ms, max(250 ms, 4 × 路径判定时差上界))`（时差上界默认 150 ms，故默认最长 600 ms，且不得超过静默阈值），只接受已在 `AllowedClients` 中的对端，本 `epoch` 结束或闸门打开并交付后立即清空。方案 (b) 下，入站静默的“首个入站”以**开闸并交付缓存**的时刻起算，不以缓存包的到达时刻起算，以免一开闸就因粘性静默而关上。协议仍保留 `DirectGateOpen` 变体，但方案 (b) 下被控端不发送、控制端不依赖。方案 (b) **不**要求控制端等待 `DirectGateOpen`，也不使用方案 (a) 那两条“等待 / 发送 DirectGateOpen”的用例。**Phase 1 按首选方案 (a) 编写先红用例与实现；改用方案 (b) 须先修订规格并替换用例。**
   两种方案都必须通过确定性用例：升级期间被控端因闸门关闭而丢弃的直连 QUIC 包数为 0，且不出现因 Initial 丢失而走 PTO 的重传（`first_quic_initial_not_lost_to_host_gate`）。随后控制端经 helper 的数据报通道用 quinn 连接被控端的**直连 QUIC 监听**，出示一次性令牌并校验 `quic_fp`；被控端校验令牌 / 会话 / `epoch` / 有效期通过后，等待控制端在**直连连接**上开三条流（`SessionHeader` 与中转上相同）。
7. **提交（commit）由被控端裁决，顺序固定**：三条直连流就绪后，被控端 **先在中转控制流上发 `DirectSwitch { epoch }`（被控端→控制端方向的栅栏）**，**再**把 `DirectCommit { epoch }` 作为**直连控制流的第一条消息**发出。被控端若在自身超时前没有收到完整的三条流，则放弃该 `epoch`（迟到的 `DirectRequest` / 连接一律忽略，I13）。**控制端打开三条流后设 T（`max(2 s, 4 × 直连 RTT)`，默认 2 秒；Phase 0 上 quic→commit 约 4 ms，2 s 足够）超时，以“读到 `DirectCommit`（不论它是否因栅栏而暂存在缓存里）”为准：收不到就经中转发 `DirectFallback { epoch }`**；被控端对已提交的 `epoch` 也必须接受它。
8. **切换规则**：
   - **画面 / 声音**：被控端提交后把发送端切到直连流并立即发一个关键帧；接收端**始终同时读取中转流与直连流**，新通道的首个关键帧到达后丢弃旧通道的画面包（按帧序号），声音丢弃序号 ≤ 已交付序号的包（24 字节媒体头没有 epoch 字段，所以规则以“来自哪条通道 + 序号”表达）。
   - **控制流（升级方向，栅栏）**：控制消息没有序号、跨流无顺序保证。每个发送端在**旧通道（中转）**上发出的**最后一条**消息是 `DirectSwitch { epoch }`，之后才在直连上发新消息；接收端读到该栅栏之前只处理旧通道，新通道的消息先缓存，读到栅栏后再处理缓存。控制端在收到 `DirectCommit` 后用同样方式切换自己的发送端。`Ack` / `Ping` / `Pong` 与顺序无关，缓存期间照常处理（避免在途窗口停住）。**栅栏等待上限**不写死 500 ms，由 Phase 0 按中转 RTT 校准（可配置或自适应）；**超时时丢弃缓存的直连控制消息并执行与回退相同的流程**（丢失由状态重同步补偿），不照常处理（会乱序）。
   - **终止消息**：`Bye` 一律经**中转**控制流发送（中转是会话生命周期的权威，I10），可同时在直连上多发一份；不存在“回退后重发 `Bye`”。
9. **中转流保持打开作为热备**（空闲，不发媒体，不 `finish`）。

### 2. 路径检测、存活检测与回退（对 DERP 的有界承诺）

**路径检测规则（helper 内的“闸门”）**：tailcat 无法禁止 DERP 承载数据，所以规格承诺**有界**而非“零”（I3）。闸门是**双向**的：关闭时，出站方向立即丢弃来自 Rust 一侧、本欲写入隧道的数据报；入站方向丢弃隧道来的、本欲交给 Rust 的数据报（方案 (b) 在关闭时改为短时缓存，见第 6 步与 I24）。并报告 `path = derp|none`。
- 控制端（`Client` 没有 `Status`，只能用 `DiscoPing`；不得用 `Client.Ping` 判定路径，其内部超时固定约 10 s）：pong 经 DERP 返回 ⇒ 立即判非 direct；连续 M = 2 次在 `max(750 ms, 3×RTT)` 内没有收到 direct pong ⇒ 判非 direct。Phase 0 在自建 DERP、静默断路后控制端闸门 **1.51 s** 关闭（6/6）。
- 被控端：**不能只看** `Server.Status()` 的 `CurAddr`。Phase 0 证实 magicsock 的 UDP 信任期为 6.5 s，只看 `CurAddr` 时静默断路后闸门要 **4.72 s** 才关（超过 2 s 阈值），窗口内有 0–10 个数据包经 DERP。因此被控端闸门在以下任一条件成立时关闭：（1）`CurAddr` 为空；（2）**入站静默**：该 flow 上连续 1000 ms 无入站数据报（默认 `rx_silence_ms = 1000`）。Phase 0 加上（2）后闸门 **1.02 s** 关闭（6/6），窗口内经 DERP 的数据包为 **0**。直连期间控制端每 250 ms 发 `Ping`，静止桌面也有入站流量，30 分钟 soak（含仿真 1% 丢包与抖动）下被控端闸门离开 direct 0 次。
- **入站静默的计时**：静默计时只在本 `epoch` 的该 flow 上、闸门**已经打开之后**观测到第一个入站数据报（方案 (b) 则为“开闸并交付缓存”的时刻）才开始。“从未有入站”、以及开闸之前到达并被缓存的包，都不得把已打开或即将打开的闸门关上。升级阶段还没有直连控制流上的 `Ping`（控制端要等读到中转上的 `DirectSwitch` 之后才在直连上发 Ping，见 I6），因此**不得用控制流 Ping 来证明开闸后会有入站**。改为：直连专用 `direct_transport_config()` 把 quinn 的 `keep_alive_interval` 设为 ≤ 250 ms（QUIC PING 帧走数据报通道，不经控制流，与 I6 的栅栏规则不冲突），保证开闸后、提交前后、静止桌面时都有入站，避免误关闸。验收：「`host_gate_not_closed_during_fence_wait_on_static_screen`」（栅栏等待期间、静止桌面、无控制流 Ping 时不得因静默关闸）。
- **静默关闭是粘性的**：粘性只针对**本 `epoch` 内已经打开过、之后又关闭**的闸门（因静默或因 `CurAddr` 变为空）。升级开始时闸门本来就是关着的，不在此列，仍可按路径判定打开。一旦已打开过的闸门关闭，本 `epoch` 内不再打开，也不再为该 `epoch` 发 `DirectGateOpen`；按路径失效走回退（控制端闸门在已打开后关闭同样触发 `DirectFallback`），换新 `epoch`（见下条重试规则）。
- **K 驱动闸门的等效方案**（可选，不是默认）：允许由 Rust 侧的存活判定 K 驱动 helper 在直连已提交之后关闭闸门，但仍须满足 I3（关闭 ≤ 2 s；自建 DERP 上窗口内数据包为 0）。**提交之前**（升级阶段还没有直连 Ping，K 不成立）必须保留入站静默或其它不依赖直连 Ping 的关闭条件，不得在升级阶段只看 `CurAddr`（Phase 0 只看 `CurAddr` 为 4.72 s，已否决）。用于驱动闸门的 K 加上 IPC 与轮询必须 ≤ 2 s，因此**不得**把驱动闸门的 K 取成 2 s（存活检测用于误回退判定的 K 实现值仍可以是 2 s，那是另一条路径）。该方案的验收必须覆盖升级阶段。
- 升级前双方的闸门**各自**关闭，直到本端检测为 direct 才打开。控制端发出首个直连 QUIC 包的时机见第 6 步与 I24，不得再依赖“升级超时内重试以容忍判定时间差”来掩盖首包被丢。
- 被控端 `Server.Status()` 的轮询间隔默认 **100 ms**（≤ 250 ms）。I3 窗口上限在采用入站静默时为静默阈值 + 轮询间隔（1000 + 100 ms）；Phase 0 实测 1.02 s。自建 DERP 上窗口内 0 包的验收，绑定到“关闭时刻早于 magicsock UDP 信任期（6.5 s）过期”这一机理：静默方案约 1.02 s 关闭，早于信任期结束，因此不会开始经 DERP 双发。不得把在 1.02 s 测得的 0 包直接外推到接近 2 s 才关闭的实现；若实际关闭时限明显长于 1.02 s，须重测包数。
- 采用入站静默（升级阶段必须保留）后，**自建 DERP 上 I3 窗口内的数据包上限为 0**（Phase 0 在约 1.02 s 关闭时实测，6/6）。对 Tailscale 托管 DERP 的窗口期与限速，Phase 0 未测，批准 (B) 前须真机复核（该项测试本身须项目负责人许可，因会联系第三方服务）；不把 0 包当成已在托管 DERP 上证实的结果。直连稳定期间，自建 DERP 上数据包与 disco 包均为 0。

**存活检测（不依赖画面）**：桌面被控端只在画面变化时编码、无声音不发包，静止桌面没有帧也没有 Ack，因此**不能**用画面包或 Ack 判存活。改用直连控制流上的 Ping/Pong：直连期间控制端以 250 ms 间隔（`Ping` 是现有消息，不改协议）发 `Ping`，被控端回 `Pong`；控制端在直连上 K 秒收不到 `Pong` ⇒ 经中转发 `DirectFallback`；被控端在直连上 K 秒收不到 `Ping` ⇒ 回退。K 取 `max(1.5 s, 4×间隔 + 2×RTT)`（RTT 400 ms 时为 1.8 s）；Phase 0 soak 使用 K = 2 s，误回退 0 次。**不再要求小于 2 秒的命令队列过期**：直连通道的写超时 / 队列过期按通道区分，直连通道上的任何超时、写失败都只转为回退，不影响中转通道的既有判定（被控端对中转的写卡住判定现状为 10 秒，措辞以源码为准）。K 中的 RTT：控制端取自 Ping/Pong 往返；被控端用本会话直连 QUIC 连接自身的 RTT 估计（`Flow` 的 srtt 在通道切换后 `min_rtt` 约 10 秒才更新，切换 / 回退时重置 `min_rtt`）。该规则同时覆盖两个方向的非对称故障。直连 QUIC 连接关闭 / 空闲同样视为失效。

**写超时看门狗（I25）**：Phase 0 发现，不限速打满（helper CPU > 100%，约 500–900 Mbps）时，被控端 helper 的写协程会永久阻塞在 gVisor `UDPConn.WriteTo`，直连吞吐降到 0，直到 QUIC 空闲超时；限速 20 / 60 / 200 / 400 Mbps 的运行未复现。根因在 tailcat 所依赖的 gVisor / netstack，不在本设计内修复（最小复现若要提交上游，不在本 fork 的 Phase 1 范围）。因此 helper 对写入隧道的 `Write` / `WriteTo`，以及候选 (ii) 下往 Rust 方向的管道写，都必须设写超时（默认 `write_timeout_ms = 500`，可配下限 100 ms、上限 2000 ms）；超时即上报直连失效，Rust 侧按回退流程切回中转，**结束当前 helper 进程并拉起新进程**开新 `epoch`，**不结束会话**。该次回退计入“30 秒、最多 3 次”的重试。Rust 侧的 K 判定仍作兜底。限速本身不能替代该看门狗。500 ms 在 200 / 400 Mbps 下会不会误触发，云 VM 未测，列入真机复核判据。

**回退（反向栅栏）**：
- 失效侧立即把自己的发送端切回中转流并请求关键帧，并且它在**中转**控制流上发出的**第一条**消息必须是 `DirectFallback { epoch }`。**发起方也丢弃该 `epoch` 此后收到的所有直连控制消息**（含已缓存的）；直连上未确认的输入丢失，靠 `release_all()` 与状态重同步补偿（不改为“带确认的回退”，避免多一轮往返）。
- 对端无论从哪里得知（收到 `DirectFallback`、或自己检测到故障）都**立即回退**；收到 `DirectFallback` 的一方此后**丢弃**该 `epoch` 的所有直连控制消息（含已缓存的），只处理中转上的消息。
- **栅栏之后中转上仍须处理的消息**：`Bye`（I10）、对端的 `DirectFallback`（幂等）、对向未完成的 `DirectSwitch`（该 `epoch` 已放弃，忽略）。回退后迟到的 `DirectCommit` 一律忽略。
- **“改投中转”的范围**：只限**尚未写出且未过期**的消息；已过期的输入**不补发**；正在直连上写的那一条视为丢失。
- **状态重同步**：被控端在本地调用 `release_all()` 释放按键；控制端的 `airhand-link` **按会话缓存最近一次 `Settings` / `Audio` / `SelectDisplay`，回退后重发**（`Settings` 与 `Audio` 的重发是幂等的；缓存的 `SelectDisplay` 在收到被控端的 `HostMsg::Displays { current }` 时同步更新，避免旧序号把画面切回去，被控端已 `follow_display` 切换的以被控端为准）（不依赖“清空本地按键状态”，也不新增 `ReleaseAll` 输入事件）；剪贴板以最新状态为准。
- **写入失败不结束会话**：直连通道的写入失败 / 卡住 / helper 写超时（I25）一律转为回退并把符合上条范围的排队消息改投中转；**直连通道失败只触发回退，绝不结束会话**；仅中转连接断开才结束会话（与 I10 一致）。helper 进程退出 / 崩溃视同直连失效，由 Rust 侧监督，不自动无限重启。
- 每次重新升级都使用**新的 `epoch`、新的 tailcat 密钥 / 地址 / 令牌**（旧的立即销毁）；因路径 / 存活检测 / 写超时（I25）而回退后，每隔 30 秒、最多 3 次重试（写超时回退**计入**这 3 次）。**若本会话已由被控端收回“直连”权限，则本会话不再重试升级**（`revoked_direct_not_retried_in_session`）。I25 触发时**结束当前 helper 进程并拉起新进程**再开新 `epoch`，不得在同一进程（同一 gVisor netstack）里新建 `Server` 来逃避已观察到的写卡死；同进程其他 `Server` 在饱和卡死时是否受影响仍标为未测。
- 并发与进程模型：**每个会话一个 helper 进程；每个 `epoch` 在进程内新建 `Server`（控制端新建 `Client` 与密钥）并 `Close` 旧的**。Phase 0 在同一进程内跑 3 个 `Server`，互相独立、无 goroutine 泄漏；设计仍保持每会话一进程。饱和卡死时同一进程内其他 `Server` 是否受影响未测。并发上限默认 2（可配，须小于服务器为每台被控端预留的会话槽位，该槽位默认 4；客户端拿不到服务器环境变量，不得引用 `AIRHAND_MAX_SESSIONS`），超出的会话保持中转。
- 栅栏等待上限：`max(1 s, 2 × 中转 srtt)`（不写死 500 ms）。Commit 等待 T：`max(2 s, 4 × 直连 RTT)`（Phase 0 上 quic→commit 约 4 ms，T = 2 s 足够）。

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
  - `HostMsg::DirectGateOpen { epoch }`（**方案 (a)**：被控端闸门打开后经中转发送；控制端收到前不得发首个直连 QUIC 包，I24。方案 (b) 不发送、不依赖此消息，但协议变体保留）
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
| `upgrade_timeout_secs` | `15` | 等待直连路径的时间，下限 12；Phase 0 在 VM 内最差约 2.2 s，保留余量覆盖未测的真实 NAT / DERP 冷启动 / 公网时延，真机复核后可再收紧 |
| `rx_silence_ms` | `1000` | 被控端闸门的入站静默阈值。可配范围：下限 **1000 ms**（与 Phase 0 soak 一致；更低的值须另行实测，因为下限若小于 Ping / QUIC keep-alive 间隔，静止桌面会误关闸，违反 I2）；上限须满足 `rx_silence_ms` + Status 轮询间隔 < 2000 ms（默认轮询 100 ms 时，上限 1899 ms）。若配置值使实际关闭时限明显长于 1.02 s，须按 §2 重测窗口包数。升级阶段必须使用该条件（或其它不依赖直连 Ping 的关闭条件），不能只靠 K 驱动 |
| `write_timeout_ms` | `500` | helper 写入隧道（及候选 (ii) 往 Rust 的管道写）的超时，范围 100–2000 ms；超时即按 I25 回退并重启 helper 进程 |
| `bitrate_cap_kbps` | `100000` | 直连会话编码目标码率上限 C（100 Mbps）；必须 ≤ 100000；回退中转后解除，见“直连会话码率上限”与 I26 |
| 其它阈值 | 见 §2 | 栅栏等待上限 `max(1 s, 2 × 中转 srtt)`、Commit 等待 T `max(2 s, 4 × 直连 RTT)`、Ping 间隔 250 ms、存活检测 K = `max(1.5 s, 4×间隔 + 2×RTT)`（误回退判定的实现值可用 2 s；**驱动闸门的 K 不得取 2 s**，须满足 K + IPC + 轮询 ≤ 2 s）、M = 2、`Server.Status()` 轮询 100 ms |
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

Windows 安装版的被控端是控制台会话里的 SYSTEM agent，界面在托盘进程，二者经命名管道通信（`winsvc/`；管道允许同会话交互用户进程连接，`ToAgent` 现仅有 `KickAll` / `NewCode` / `SetAutostart` / `SaveDevices`）。r4 定：确认窗放在托盘进程；新增 `ToAgent::Approve { session_id, granted }` / `ToAgent::Decline { session_id }`；agent 在收到前校验管道客户端进程为本安装目录下的 `airhand.exe`（如实说明：同用户攻击者不在防护范围内，其本来就能控制该用户会话）。**`approve = "ask"` 时，登录屏 / 锁屏没有托盘，现架构下没有可显示确认窗的路径 ⇒ 确认超时拒绝**（结论，不再是“未证实”）；是否能在 agent 内于安全桌面弹窗，列为可选的后续验证项，不在 Phase 1 范围。**`approve = "never"` 时不依赖确认窗**，登录屏 / 锁屏下会话仍可按 I21 建立；若同时开启了 `[direct] enabled`，直连升级也可进行。**此期间没有本机指示条、不能在本机一键停止共享或收回权限、也不能显示 SAS**（I22 的例外）：本机侧的停止 / 换码须等出现可显示托盘的会话，或运营者在本机用其他途径改配置 / 结束进程；远程侧仍可通过控制端断开结束该会话。不采用“无法显示指示条就不升级直连”（那会改变无人值守直连的默认可用性；该选项与其它 never 模式隐私缓解一并列为「待项目负责人决定」，见下节，不作为本规格默认设计）。Phase 0 **未测** Windows 受限令牌 / AppContainer（只验证了 Windows 交叉编译，helper 约 19 MB）。在项目负责人指定的真机复核之前，退路仍然有效：若证实不可行，Windows SYSTEM agent 作为被控端时禁用直连，never + 直连在 Windows 安装版被控端上同样不可用。

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
- **I2** 直连失败、超时、对端未启用、helper 缺失 / 校验失败 / 崩溃 / 写超时（I25），一律**静默回退**中转，不中断会话、不弹错误（只在诊断面板留痕）。**静止桌面（无画面、无声音）不得触发回退**。
- **I3** 路径一旦按 §2 规则判为非 `direct`，helper 闸门双向关闭（出站不再写入隧道，入站丢弃或按 I24 方案 (b) 短时缓存）；升级前不为“尚未 direct”的路径切换发送端。被控端闸门必须包含入站静默条件（默认 1000 ms 无入站即关；计时从本 `epoch` 该 flow 的首个入站数据报之后才开始；关闭在本 `epoch` 内粘性）。允许在直连已提交之后由 Rust 侧 K 驱动关闭，但升级阶段仍须保留静默或其它不依赖直连 Ping 的关闭条件；驱动闸门的 K + IPC + 轮询 ≤ 2 s。闸门须在路径退化后 **2 s 内**关闭。在自建 DERP 上，当关闭时刻早于 magicsock 信任期（6.5 s）过期时，窗口内经 DERP 发出的**数据包上限为 0**（Phase 0 在约 1.02 s 关闭时实测为 0；只看 `CurAddr` 时为 0–10 / 4.72 s，该方案已否决）。托管 DERP 上的窗口期须真机复核（须项目负责人许可），不得把 0 包写成已在托管 DERP 上证实。
- **I4** 握手消息（`Hello` / `Welcome` / `Incompatible`）与 ALPN 的编码不变；`PROTOCOL_VERSION` 升到 10。
- **I5** 令牌一次性：只对一个会话、一个 `epoch`、有效期内有效；重放、过期、错会话、错 `epoch` 一律拒绝。
- **I6** 同一时刻每条逻辑流只有一个**发送**通道。升级方向：栅栏保证控制消息不乱序、不丢失，画面 / 声音按序号去重且不倒退。回退方向：以 `DirectFallback` 反向栅栏切断直连控制流，迟到的直连控制消息被丢弃；直连上未确认的数据可能丢失，以按键释放与状态重同步补偿。
- **I7** tailcat 地址、节点密钥、令牌不出现在日志、`Debug` / `Display` 输出、命令行参数、`config.toml`、错误文案与崩溃信息里（`Secret<T>`）；helper 默认丢弃 tailcat 内部日志。
- **I8** 不依赖解码器对未知变体的容忍：新增 `kind` 一律由 `DirectDecline` 回应；协议扩展以版本号为界。
- **I9** helper 只服务一个 UDP 端口、无 TCP 服务；对其他端口 / TCP 不回 RST（探测超时）；回环通道的暴露面按所选候选 (i)/(ii) 的约束。
- **I10** 托盘显示 / 断开 / 更换控制码对直连会话的语义与中转会话相同；升级过程中被踢 / 更换控制码，两条通道一并结束；`Bye` 一律经中转发送。
- **I11** 直连 QUIC 单个数据报载荷 ≤ 1232 字节。不得依赖库报错：Phase 0 证实超限写入返回 nil 并被静默丢弃。直连专用 `Endpoint` 与 `direct_transport_config()` 使用 quinn 0.11.12 的 `initial_mtu(1232)`、`min_mtu(1200)`、`mtu_discovery_config(None)`、`EndpointConfig::max_udp_payload_size(1232)`；只设 `max_udp_payload_size` 而不提高 `initial_mtu` 时 MTU 会停在 1200。中转用的 `transport_config()` 不变。helper 的 Pump 对超限包计数丢弃。
- **I12** `DirectOffer` 只在**已取得直连同意**且 `HostHello` 已发之后发送。被控端的直连同意来源只有两种，且都必须对端 `ControllerHello.direct` 为真：（a）`approve = "ask"`：被控端用户在该会话确认窗中授予“直连”权限；（b）`approve = "never"` 且 `[direct] enabled = true`：运营者显式配置视为同意（I21）。控制端以本端 `[direct] enabled` 为对暴露自身地址的同意。此前收到的、以及未取得直连同意时收到的 `DirectRequest` 一律丢弃（后者回 `DirectDecline`）。
- **I13** 提交由被控端裁决；迟到的 `DirectRequest` / 接受 / 连接 / `DirectGateOpen`（`epoch` 已过期、不匹配或已放弃）一律忽略；任一端经中转收到对端的 `DirectFallback` 立即回退；控制端等不到 `DirectCommit` 超时即回退。方案 (a) 下等不到 `DirectGateOpen` 的时限计入 `upgrade_timeout_secs`，超时则放弃本次升级、保持中转。
- **I14** `client_key` 为空或无法解析时，Rust 侧与 helper 都拒绝 Start（不得起一个放行所有客户端的 `Server`）；helper **不调用** `AddAllowedClient`（运行时追加会重新引入“列表为空即放行”的窗口）。
- **I15** 桌面被控端 `approve = "ask"`（默认）时，每个新控制端会话在用户确认前：**不发 `HostMsg::Hello`、不发画面 / 声音**，只发 `AwaitingApproval`；只**处理** `ControllerMsg::Hello`（取设备标识与直连能力位）/ `Ping` / `Bye`；`Settings` / `Audio` / `SelectDisplay` **只记下最新值，确认后生效**（与 Android 一致，避免回归）；输入、剪贴板、`Ack`、`Direct*` 一律丢弃。**超时（默认 60 秒）即拒绝**（`Bye(Unanswered)`），点拒绝为 `Bye(Declined)`；二者对控制端是终止性的（`Problem::is_final()`，不自动重连）。`approve = "never"` 不适用本条，见 I21。
- **I16** 取得直连同意之前：不交换任何地址 / 令牌 / 密钥，被控端不启动 helper，控制端 helper 不联网（不拉 DERP 图、不连 DERP）；未取得直连同意的会话永不启动 helper。同意的判定与 I12 相同。
- **I17** 确认窗口显示的对端自报文字（设备名等）有长度上限并清理控制 / 双向字符，且标注“自报”；同时等待确认的会话数有上限；同一来源被拒绝 / 无应答超限后静默拒绝，不再弹窗。
- **I18** **停止共享**：结束所有会话的**中转与直连两条路径**（`Bye(CodeChanged)` 经中转；销毁每个会话的 tailcat `Server`、令牌、直连连接；结束 helper 进程）并**更换控制码使旧码立即失效**；之后迟到的直连连接 / 令牌 / 旧码一律拒绝。被控端“断开此控制端”结束选中会话的两条路径（`Bye(Kicked)`），不换码；在 `approve = "ask"` 下被断开的设备须重新确认才能回来，在 `approve = "never"` 下持码人可以再连回来。
- **I19** 控制端主动断开：经中转发无字段的 `ControllerMsg::Bye`，随后关闭直连连接并结束 helper；被控端收到后清理该会话的全部直连资源。控制端的断开入口在会话期间始终可用（含升级进行中与直连状态）。被控端的本机断开入口在指示条 / 托盘可显示时始终可用；登录屏 / 锁屏等没有指示条时，本机侧不适用，见 I22。
- **I20** 权限由被控端强制执行（每项在两个方向上都要有执行点）：关闭“输入控制”后丢弃后续输入；关闭“剪贴板”后既不应用控制端发来的内容、也不再向控制端推送（桌面端现状这两处都不受条件限制，需新增）；关闭“声音”后取消订阅；收回“直连”权限立即回退中转并销毁 helper / tailcat 实例（不换码），且本会话不再自动重试升级；权限变化经 `HostMsg::PermissionsChanged` 通知控制端。
- **I21** `approve = "never"` 是运营者显式开启的无人值守模式：不弹确认窗、不发 `AwaitingApproval`，会话立即发 `HostHello` 并开始画面。`HostHello.granted` 取值：输入 / 剪贴板 / 声音沿用现有行为（全部为真）；**`direct` 当且仅当同时显式开启 `[direct] enabled` 且对端 `ControllerHello.direct` 为真时为真**。运营者显式同时开启这两项配置，即视为对“暴露本机公网 / 内网地址并经第三方 DERP 引导”的同意，无需再逐会话勾选（此为 r4「never 禁用直连」的修订）。只开 `never` 而不开 `[direct] enabled`，或对端未声明直连能力时，`granted.direct` 为假，不发 `DirectOffer`、不启动 helper。该模式须显式开启，托盘可显示时给出醒目警告（说明：持码人可在无人确认的情况下接入；若直连已启用，还将获知本机地址）。**已知风险**（不是实现缺陷，见“never 模式的已知隐私风险”）：持码人可以连上来拿到地址后立即断开、并反复来访；被“断开此控制端”的持码人在控制码仍然有效时也可以再连回来。本条不变量不依赖任何尚未决定的缓解选项。放宽这两项配置的生效规则见“配置的读取与生效”。
- **I22** 被控端在指示条 / 托盘可显示时，会话期间的常驻指示不能被远端画面关闭或遮盖，显示控制端设备名、当前路径（中转 / 直连）与已授予权限，并可一键停止共享或收回权限；指示条只能收回权限；确认窗 / 续期提示 / 配置确认不能被远端注入输入代点（见“防注入”）。**例外**：登录屏 / 锁屏等没有托盘与指示条时，本机无法显示指示、SAS 与一键停止（never 模式下会话与直连仍可按 I21 进行）；此时只能由控制端断开，或等本机出现可显示的会话 / 运营者在本机用其他途径换码或改配置。**显示层行为无法自动化，验收归入手动项。**
- **I23**（实现位置）确认闸门状态机（`ask`：等 `Hello` → 询问 → 放行 / 拒绝、授权前缓存、授权记忆；`never`：跳过询问、按 I21 赋予 `granted`；权限集）下沉到 `airhand-link`，桌面与 Android 共用；Android 现有实现迁移为共用逻辑，避免两份逻辑走样。
- **I24** 控制端的首个直连 QUIC 包不得因被控端闸门尚未打开而丢失。方案 (a)：被控端闸门打开后经中转发 `DirectGateOpen`；控制端须在本端闸门已打开且收到该提示之后才发出首个直连 QUIC 包。方案 (b)：被控端 helper 在闸门关闭时按第 6 步的界限短时缓存入站包，控制端可以在开闸前发出首包。两种方案都必须通过 `first_quic_initial_not_lost_to_host_gate`（升级期间因闸门关闭而丢弃的直连 QUIC 包数为 0，且不出现因 Initial 丢失而走 PTO 的重传）。方案 (a) 另有“等待 / 发送 DirectGateOpen”用例；方案 (b) 不用这两条。不得只靠调小 PTO / `initial_rtt`。
- **I25** helper 写入隧道，以及候选 (ii) 下往 Rust 方向的管道写，必须有写超时（默认 500 ms）；超时或写操作卡住即视为直连失效，回退中转，结束当前 helper 进程并拉起新进程后开新 `epoch`，不结束会话。该次回退计入“30 秒、最多 3 次”重试。该看门狗独立于码率上限，限速不能替代它。Rust 侧 K 判定仍作兜底。
- **I26** 直连期间编码器目标码率不得超过上限 C（默认 C = 100 Mbps，且 C ≤ 产品上限 100 Mbps）；达到 C 不触发回退。回退到中转后解除直连上限 C，改回产品 / 中转既有上限与既有 ABR。

## 验收测试清单

测试命名采用 Rust `#[test]` / `#[tokio::test]` 函数名与 Go `Test…` 名；文件为**将要创建**的路径，实现阶段按“先红后绿”提交。
验证面均为**本机自动化**（回环 + 假 helper / 可注入路径源）；真实 NAT / 跨公网表现只能在**线上手动验证**。确认闸门状态机下沉到 `airhand-link` 之后（I23），会话 / 直连 / 停止共享 / never 路径等逻辑用例放在 `crates/airhand-link/tests/direct.rs`；仍属于桌面托盘、确认窗、指示条、配置文件的用例放在 `client/src-tauri/tests/`。

- [ ] I4：既有「`version_handshake_wire_is_stable`」必须仍通过；新增协议用例统一放在 `crates/airhand-proto/tests/wire.rs`：「`protocol_version_is_10`」「`direct_messages_wire_is_stable`」（含 `DirectGateOpen`）。既有 `framing.rs` 内联用例保持不动。
- [ ] I1：`crates/airhand-link/tests/direct.rs` →「`disabled_by_default_never_spawns_helper`」；预期：假 helper 工厂 0 次调用、无任何 `Direct*` 消息、无到 tailcat/DERP 的连接尝试。
- [ ] 配置：`client/src-tauri/tests/direct_config.rs` →「`direct_config_defaults_off`」「`direct_config_rejects_non_https_derpmap`」「`direct_config_has_no_helper_path_option`」「`direct_config_bitrate_cap_default_is_100mbps`」「`direct_config_rejects_bitrate_cap_over_100mbps`」。
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
- [ ] I25：「`helper_write_stall_triggers_fallback`」（可注入的假写路径在超过 `write_timeout_ms` 后仍阻塞 ⇒ 上报直连失效，Rust 侧回退中转、结束 helper 进程并拉起新进程后开新 `epoch`、会话继续，不发结束会话的 `Bye`；该次回退计入最多 3 次重试）；Go「`TestWriteTimeoutReportsPathFailure`」（隧道 `Write` / `WriteTo` 以及候选 (ii) 往 Rust 的管道写，超过默认 500 ms 即失败并上报）。
- [ ] I3（Rust 侧）：「`upgrade_timeout_keeps_relay`」「`no_switch_before_both_gates_open`」。I3（Go 侧，helper 本身）：`p2p/gate_test.go` →「`TestGateDropsWhenPathNotDirect`」（可注入路径源，断言出站与入站在关闭时都被丢弃或按方案 (b) 缓存）「`TestControllerGateIgnoresSlowButDirectPath`」（RTT 大于 250 ms 且 ≤ 400 ms 的直连不被误判，与阈值表对齐）「`TestHostGateClosedWhenCurAddrEmpty`」「`TestHostGateClosesOnRxSilence`」（首个入站之后连续 1000 ms 无入站，闸门关闭且时限 ≤ 2 s）「`TestHostGateRxSilenceNotArmedBeforeFirstInbound`」（开闸后、首个入站到达前，静默条件不得关闸）「`TestHostGateSilenceCloseIsStickyInEpoch`」（静默关闭后本 `epoch` 不再打开、不再发 `DirectGateOpen`）「`TestHostGateWindowZeroPacketsOnSelfHostedDerp`」（进程内自建 derper，不依赖外网；关闭时刻早于信任期过期时窗口内数据包为 0；只看 `CurAddr` 不得作为通过条件）「`TestHostGateWindowIncludesStatusPollInterval`」「`TestClientPingMustNotBeUsedForPath`」「`TestKDrivenGateStillSilentBeforeCommit`」（若实现 K 驱动，升级阶段仍须保留静默或其它不依赖 Ping 的关闭条件，关闭 ≤ 2 s）「`host_gate_not_closed_during_fence_wait_on_static_screen`」（Rust 侧：栅栏等待期间静止桌面不得因静默关闸；直连 `keep_alive_interval` ≤ 250 ms）「`TestSchemeBSilenceStartsAtCacheFlush`」（仅方案 (b)：静默从开闸并交付缓存起算）。另附**手动 / 真机**（机器待项目负责人指定；托管 DERP 须另行许可）：复核窗口时长与包数。
- [ ] I24：两种方案共用「`first_quic_initial_not_lost_to_host_gate`」（注入被控端闸门晚于控制端打开的时间差，升级期间因闸门关闭而丢弃的直连 QUIC 包数为 0，且不出现因 Initial 丢失而走 PTO 的重传）。方案 (a) 另有：「`controller_waits_for_direct_gate_open_before_first_quic`」（未收到 `DirectGateOpen` 且本端闸门未开时，控制端 0 个直连 QUIC 包）「`host_sends_direct_gate_open_when_gate_opens`」「`late_or_mismatched_direct_gate_open_is_ignored`」。方案 (b) 另有：「`host_inbound_cache_delivers_initial_before_gate_open`」「`host_inbound_cache_is_bounded_and_cleared_on_epoch_end`」。
- [ ] I5：「`direct_token_is_one_time`」「`direct_token_wrong_session_rejected`」「`direct_token_expired_rejected`」「`direct_token_wrong_epoch_rejected`」；证书固定：「`direct_cert_pin_mismatch_aborts_upgrade`」。
- [ ] I6：「`input_order_preserved_across_switch`」「`fence_timeout_drops_buffer_and_releases_keys`」「`late_direct_input_after_fallback_is_discarded`」「`keys_released_after_fallback`」「`settings_resynced_after_fallback`」「`host_initiated_fallback_discards_inflight_direct_input`」「`bye_on_relay_processed_after_fence`」「`expired_input_not_replayed_on_relay_after_fallback`」「`commit_after_own_fallback_is_ignored`」「`controller_resends_cached_settings_audio_display_after_fallback`」「`duplicate_frames_across_lanes_are_dropped`」「`stale_lane_frames_before_keyframe_are_dropped`」「`audio_not_replayed_after_switch`」。
- [ ] I11：「`direct_quic_never_sends_datagram_over_1232`」（行为测试：假 helper 记录并丢弃超限数据报，断言观测到的最大数据报 ≤ 1232；不得指望库对超限写入返回错误）。
- [ ] I26 直连码率上限：「`direct_session_caps_bitrate_instead_of_falling_back`」（**注入较低的 C，例如 30 Mbps**，目标码率高于该 C 时被限制在 C，且不发 `DirectFallback`；不得只在默认 C=100 Mbps 下断言，因为产品上限本来就是 100 Mbps，会变成空断言）「`fallback_to_relay_lifts_direct_bitrate_cap`」（同样在注入的较低 C 下，回退中转后解除该上限）「`direct_config_bitrate_cap_default_is_100mbps`」「`direct_config_rejects_bitrate_cap_over_100mbps`」。
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
| 规格 | 本文 + feature map，走分支 + PR | r5 已随 PR #1 合入本 fork 的 `main`（2026-11-10，merge `b24a671`）。**r6 仍在 `spec/tailcat-p2p` 上追加，须另开 Draft PR**；不写 `main`、不合并、不改仓库设置 |
| 设计评审 | 需求与架构评审（独立于实现），分级 findings，修订后复审至无严重问题 | [`tailcat-p2p-review.md`](tailcat-p2p-review.md)（各轮报告全文在 [`review/`](review/)）；r6 对应第 6 轮 |
| 项目负责人批准 (A) | 本变更属重大改动；spec 与评审都不代替批准。(A)：**仅批准 Phase 0 spike**，范围限定在独立 spike 分支，**不得合并，spike 代码不得直接复用进 Phase 1**（Phase 1 按先红后绿重写） | **已批准（2026-11-10 项目负责人批复）**，阈值同批确认；结论记录在 PR 评论 |
| 工作流脚本 / CI 移植（独立 PR） | 把 workflow v2 的检查（隐私扫描、署名 trailer、spec-requires-tests）移植进本 fork，**单独一个 PR，在 Phase 1 之前完成** | **已完成**：PR #2 已合入本 fork 的 `main`（事实记录）。本 r6 工作不改 `ci/workflow-v2` 分支与仓库设置 |
| Phase 0 spike | 实测并产出数据，见下表清单与 go/no-go 阈值 | 云 VM 结果已写入本文 r6 与下节对照表。spike 分支 PR #3 **保持 Draft**，代码不进入 Phase 1 |
| 复审 | **Phase 0 结果 → 规格 r6 → 再做一轮设计复审**直到无 S1 | 第 6 轮，见 [`review/tailcat-p2p-round6.md`](review/tailcat-p2p-round6.md) |
| 项目负责人批准 (B) | 批准 Phase 1 全量实现；“降级为仅低码率档”属范围变更，须在 (B) 中单独确认 | **尚未批准**。**(B) 的前提**：r6 复审无 S1；**真机复核按下文「批准 (B) 前真机复核判据」通过**（机器待项目负责人指定，本规格与执行者不做任何安排、不预订设备；托管 DERP 须另行许可）。结论记在 PR 评论 |
| 先红后绿 | 先提交行为断言失败的测试，再最小实现；**不做豁免**（项目负责人对本仓库 workflow v2 的要求严于原文里“可对单个 PR 豁免”的条款，以本决定为准） | 先红验收测试清单见下文。测试代码由另行安排的工作编写（本规格不附测试代码）。含本规格变更的 Draft PR 在先红测试并入之前，`spec-requires-tests` 为红是**预期状态**，不是豁免 |
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
11. 在最终选定通道上测 R，按 **k = 0.5**（真机复核判据；云 VM 上的推导曾写 k ≈ 0.5）给出建议 C，并在码率 C（及可达时接近 100 Mbps）下加测 helper CPU；记录 Windows 受限令牌结论对 never + 直连的影响。Phase 0 云 VM 未完成 100 Mbps × 5 min 与 C 码率 CPU，这两项列入真机复核。

### go/no-go 阈值（2026-11-10 项目负责人已批准；数值须在 Phase 0 报告里逐项对照）

| 指标 | 阈值 |
|---|---|
| helper 额外 CPU（被控端，1080p60、约 20 Mbps 单会话；参考硬件：近三年主流桌面 / 笔记本，在 spike 报告中写明型号级别。高码率 C 下的 CPU 见上节，另测） | ≤ 单核 30% |
| 局域网直连额外往返延迟（相对不经 helper 的同路径） | ≤ 3 ms |
| 单会话持续吞吐（5 分钟，在 QUIC 层统计） | ≥ 60 Mbps，QUIC 层丢包 < 0.1%；产品允许的最高码率为 100 Mbps（`QualitySettings.bitrate_kbps` 上限 `100_000`）。直连码率上限 C 见下节（r6 已按公式填入，须真机复核） |
| 路径判定误判（直连稳定且 RTT ≤ 400 ms，30 分钟；与 Go 用例覆盖的 RTT 范围对齐） | 0 次 |
| 存活检测误回退（直连稳定；静止桌面 + Wi-Fi 场景，30 分钟） | 0 次 |
| I3 窗口（自建 `derper`，路径被人为切断后） | 闸门关闭 ≤ 2 s；自建 DERP 上窗口内数据包上限为 **0**（Phase 0 在加入入站静默后实测；只看 `CurAddr` 的 0–10 已否决）。托管 DERP 须真机复核 |
| 升级完成时间（p50；起点：控制端发出 `DirectRequest`；参考硬件：近三年主流桌面 / 笔记本，在 spike 报告中写明型号级别） | ≤ 12 s（含 DERP 引导与打洞） |
| Windows 受限令牌 / AppContainer | 至少一种能绑定 UDP 并正常运行；否则走退路（Windows SYSTEM 被控端禁用直连） |
| 任一不达标（Windows 一行除外，Windows 一行有自己的退路，不触发整体放弃） | 降级为“仅低码率档可用”或放弃；该决定属范围变更，须项目负责人单独确认 |

### 直连会话码率上限策略（r5 定策略，r6 填入 C）

- 产品最高码率仍为 100 Mbps，与中转相同；直连**不**单独提高上限。
- **策略：限制码率，不因码率达到上限而回退中转。** 回退仍只由路径判定、存活检测、写超时（I25）、用户收回“直连”权限或主动断开触发。直连路径沿用现有自适应码率（拥塞下调、恢复上调），但其编码目标码率（`QualitySettings.bitrate_kbps`）不得超过直连上限 C。
- R 与 C 不是同一层的量：R 是在**最终选定的数据报通道（(i) 或 (ii)，二者尚未因 Windows 结论选定）**上测得的 QUIC 层持续吞吐；C 约束的是编码器目标码率。C 必须为声音、包头与重传留出余量，即 **C ≤ k×R**（k < 1），不得把 R 直接当成 C。回退到中转后，解除直连上限 C，改回产品 / 中转既有上限（100 Mbps）与既有 ABR。
- **r6 取值：C = 100 Mbps**（`bitrate_cap_kbps = 100000`），与产品上限相同，直连不再另加更低的编码上限。依据（均来自 Phase 0 **云 VM**，**不是**参考硬件）：
  - 5 分钟持续吞吐只测到了达标门槛：**60 Mbps × 5 min**，(i)/(ii) 均为 0 丢包。这是已证实的 5 分钟 R 下限，**不是**最大 R；不得把 60 Mbps 当成 R 的上限。
  - 短测 **200 Mbps × 30 s**，(i)/(ii) 各 2 次，平均均达 200.0 Mbps，每秒最低 ≥ 199.6，丢包 0–0.03%，有余量。把这段短测吞吐当作暂用的 R 下界时，取 **k ≈ 0.5** 可得 C ≤ 100 Mbps，与产品上限相接。
  - **400 Mbps × 30 s** 开始丢包（(i) 一组达 1.48%），不作为 R。
  - 不限速打满先到约 500–900 Mbps 然后卡死（I25），不能当作可用 R。
  - 20 Mbps 下 helper 额外 CPU 为 8–9%（阈值 ≤ 30%，仅作量级参考）。60 Mbps × 5 min 时被控端 helper CPU 约 20–21%。200 Mbps × 30 s 时被控端 helper CPU 约 40–51%（高码率 CPU 如实记录；阈值本身仍在约 20 Mbps 参考场景下定义）。
- **必须用真机复核后才能作为批准 (B) 的依据**。C = 100 Mbps 在云 VM 上是暂定值：5 分钟只证实了 60 Mbps，200 Mbps 只跑了 30 s，并不满足“5 分钟持续 R、再取 k = 0.5”的完整推导。Phase 0 清单第 11 项要求的“C 码率下 helper CPU”也未在 100 Mbps 下测量（只测了 60 与 200 Mbps）。判据见下表；机器待项目负责人指定，本规格不做任何安排。不达标不得在本规格内自动降档，须提交项目负责人另定。
- 本策略与 C 的数值均须在批准 (B) 时一并确认。验收见 I26 用例；机制用例须注入较低的 C，不能只靠默认 100 Mbps。

### 批准 (B) 前真机复核判据（只写判据，不安排机器与时间）

机器待项目负责人指定。被测对象默认是 spike 分支（PR #3 保持 Draft、不合并）。是否允许在 spike 上补做 I24 / I25 / 入站静默修正，属于已批准 (A) 的范围问题，须项目负责人确认；本规格不安排该补测。I24 / I25 的行为验收仍归 Phase 1（(B) 之后）。托管 DERP 复核会联系第三方服务，**须项目负责人另行许可**后才能做。

| 项 | 通过条件 | 不通过时 |
|---|---|---|
| 码率 / k | **固定 k = 0.5**。在参考硬件、最终选定通道上，5 分钟 QUIC 层持续吞吐 **R ≥ 200 Mbps**，丢包 < 0.1%，从而 C = 100 Mbps 满足 C ≤ k×R | 提交项目负责人另定 C；不在规格内自动降档 |
| 约 20 Mbps 下的 CPU | 已批准阈值：helper 额外 CPU ≤ 单核 30%，须在参考硬件上达标 | 按已批准 go/no-go 处理（降级或放弃，须项目负责人确认） |
| C 码率下的 CPU | 在目标 C = 100 Mbps 下如实记录 helper 额外 CPU。**100 Mbps 下是否可接受，由项目负责人在 (B) 时根据真机记录判断**，规格不另设自动阈值 | 记录数字，交 (B) 判断 |
| I3 自建 DERP | 闸门关闭 ≤ 2 s；窗口内数据包为 0（关闭时刻须早于 magicsock 信任期过期） | **视为 (B) 前提不满足**；须先修规格或实现后重测 |
| I3 托管 DERP | 期望窗口内数据包为 0（与自建相同的静默 / 等效条件）；该项须项目负责人许可 | 不是 0 时提交项目负责人决定允许的上限，不在规格内擅自放宽 I3 |
| 路径 / 存活 | 真实 NAT / 公网时延 / 真实 Wi-Fi 下，误判与误回退仍为 0；升级 p50 ≤ 12 s | 提交项目负责人按已批准 go/no-go 处理 |
| 写超时误触发 | 在 C 与 200 Mbps 限速下，写超时看门狗误触发次数为 0（或如实记录并交 (B) 判断）。若项目负责人不允许在 spike 上补做 I25，该行标为**未测**，移交 Phase 1 验收，并在 (B) 中注明 | 记录数字或“未测”，交 (B) 判断 |
| 码率所用通道 | 若候选通道 (i)/(ii) 尚未选定，两种候选在参考硬件上都须达到 R ≥ 200 Mbps × 5 min；或先选定通道再测 | 未选定且只有一种达标时，不得把另一种当作已通过 |
| Windows / macOS | 受限令牌或 AppContainer 至少一种能绑定 UDP 并运行；macOS 签名 / 公证可行性如实记录 | Windows 不可行则走已有退路（SYSTEM 被控端禁用直连），不触发整体放弃 |

> 工作流适配说明：`docs/workflow-v2.md` 与 `AGENTS.md` 来自另一个仓库（passkey 演示）。检查脚本与 CI 已由独立 PR #2 合入本 fork 的 `main`（`spec-requires-tests`、`agent-trailer`、`privacy-scan`）。本规格把验收映射为 Rust / Go / 前端用例名；先红测试的文件位置必须符合 `scripts/check-spec-tests.mjs` 认可的类型。本 r6 工作不改 `ci/workflow-v2` 分支。更改 `docs/specs/*.md`（`review/` 下的评审报告除外）的 PR 必须同时包含先失败测试，无豁免。

### Phase 0 结果对照（云 VM；来源 PR #3 / `065c6f7`，保持 Draft）

数据全部来自云 VM（4 vCPU，型号未公开的 Intel Xeon，约 15 GB，Linux 6.12；自建 DERP + 命名空间；仿真 WAN / Wi-Fi），**不是**“近三年主流桌面 / 笔记本”参考硬件。真实 NAT、公网时延、托管 DERP、Windows、macOS、真实 Wi-Fi **未测**，批准 (B) 前须在项目负责人指定的真机上复核。

| 指标 | 云 VM 实测 | 结论（仅限该环境） |
|---|---|---|
| helper 额外 CPU（约 20 Mbps） | 被控端 8.0% (i) / 9.0% (ii)；控制端 7.2% / 7.2% | 达 VM 阈值；须真机复核 |
| 局域网额外往返延迟 | (i) p50 +0.23–0.46 ms； (ii) 复测 p99 +0.57–0.95 ms | 达标；（ii）首测 p99 +3.83 ms 属 VM 噪声 |
| 60 Mbps × 5 min | (i)/(ii) 均为 60.0 Mbps，0 丢包 | 达标 |
| 200 Mbps × 30 s | 平均 200.0 Mbps，丢包 0–0.03% | 有余量；非 5 分钟 |
| 400 Mbps × 30 s | 开始丢包（(i) 一组 1.48%） | 不作为 R |
| 路径误判 / 存活误回退（30 min） | 闸门离开与 K 超时均为 0（仿真 Wi-Fi） | 达标；真实 Wi-Fi 未测 |
| I3 被控端闸门 | 只看 `CurAddr`：4.72 s，窗口内 0–10 包（否决）；加 1000 ms 入站静默：1.02 s，0 包 | 修正方案达标；托管 DERP 未测 |
| I3 控制端闸门 | 1.51 s（6/6） | 达标 |
| 升级 p50 | 真实中转 + 同意：DirectRequest→Commit p50 0.39 s、p90 1.36 s（180/180）；未修 I24 时呈约 0.44 s / 1.40 s 两档 | 低于 12 s；I24 为 Phase 1 前提 |
| 同意步骤开销 | 6–9 ms；never Hello→Commit p50 约 0.48 s | 可忽略量级 |
| 多 `Server` | 同进程 3 个，互相独立、无 goroutine 泄漏 | 设计仍每会话一进程 |
| `Client.Ping` / 超限 UDP / quinn 0.11 名称 | 内部超时约 10 s；超 1232 写入返回 nil 并静默丢弃；`initial_mtu(1232)` 等名称已确认 | 已写入 I11 与 §2 |
| helper 体积与许可 | linux/amd64 strip 后约 19 MB 量级；无 GPL / LGPL / MPL | 已核实许可大类 |
| 候选通道 (i)/(ii) | 吞吐与 CPU 相当；(ii) 不开本机端口，饱和卡死更易复现 | **暂不选定**，等 Windows 真机结论 |
| 控制端 Offer 前是否已有网络活动 | 规格要求无；列为验收「`controller_helper_no_network_before_offer`」 | 以该用例为准，不把 spike 细节当作已关闭 |
| Windows 受限令牌 / AppContainer / 防火墙；macOS 签名公证 | 仅交叉编译成功 | **未测**；机器待项目负责人指定 |

建议写入配置的超时（见 §2 / 配置表）：`upgrade_timeout_secs = 15`（下限 12）、栅栏 `max(1 s, 2 × 中转 srtt)`、T `max(2 s, 4 × 直连 RTT)`、Ping 250 ms、存活检测 K `max(1.5 s, 4×间隔 + 2×RTT)`（误回退判定的实现值可用 2 s；驱动闸门的 K 不得取 2 s）、`rx_silence_ms = 1000`、写超时 500 ms、Status 轮询 100 ms。

### never 模式的已知隐私风险

`approve = "never"` 且 `[direct] enabled` 时，**持码人可以连上来拿到被控端公网 / 内网 / VPN 地址后立即断开，并反复来访**。被“断开此控制端”的持码人在控制码仍然有效时也可以再连回来并再次获得直连。地址收集在画面上不可见；never 模式没有确认窗，也就没有“猜中码会惊动本机用户”这层间接防护。默认使用 Tailscale 托管 DERP 时，引导过程还会向第三方暴露被控端公网 IP 与连接时间。

这是 r5 决定（never 允许直连、显式配置视为同意、拒绝不使控制码失效）下的**已知风险**，不是实现缺陷，也不构成单独的不变量。运营者现有的关闭手段仍是：停止共享（换码）、关闭 `approve = "never"`，或关闭 `[direct] enabled`（后者只阻止直连）。

### never 模式隐私缓解选项（待项目负责人决定）

下列选项**只作为候选项写入，标为「待项目负责人决定」**。本规格**不把其中任何一项写成默认设计**，也**不让任何不变量依赖“已启用某项缓解”**。决定之前**不实现**这些选项。决定之前的现行行为就是 r5 已批准的语义（持码人被“断开此控制端”后可以再连；登录屏 / 锁屏在 never + `[direct] enabled` 下仍可直连，I22 例外），这是已经批准的现状，**不是**在替选项预设“关闭”默认值。若某项被采纳，须另行修订相应不变量、用例与 feature map 后再实现。

| 选项 | 利 | 弊 | 一旦采纳会改动的规格点 |
|---|---|---|---|
| 审计日志（本地记录 never 模式下的直连会话时间、控制端设备名、是否短连接；按 I7 脱敏，不写地址 / 密钥 / 控制码） | 事后可追查谁在何时接入、是否在收集地址；崩溃 / 写超时也可入账 | 不能阻止收集地址本身；日志仍含会话元数据，有泄露与磁盘占用；保留期限与查阅入口都要另定 | 主要是文档与实现要求；现有不变量不必改，但须补日志脱敏用例，并与 I7 对齐 |
| 对 never 模式下新会话 / `DirectRequest` 限速 | 降低反复短连接收集地址、猜码的速率 | 合法频繁重连可能被误伤；挡不住第一次得手；阈值与计数键（来源 IP / 设备标识）须另定，设备标识可伪造 | 可能扩展 I17；须新增限速用例；不改变 I21 的同意定义 |
| 定期更换控制码 | 缩小码泄露后的可用窗口，缩短被踢后反复回来的时间 | 无人值守的合法控制端会被打断，须重新获码；周期过短伤可用性，过长缓解有限；与已决定的“停止共享才换码、断开此控制端不换码”如何协调须另定 | 须修订 I18（何时换码）与配置项；补定期换码用例；与开放问题 12 的已决定语义协调 |
| 拒绝被踢掉的控制端重连（拒绝表 / 仅已保存设备） | 关闭“断开此控制端后持码人再连回来拿地址”的窗口 | 改变 r5 已写明的 never 语义（持码人可再连）；作用域（本次运行 / 持久）与伪造设备标识都要另定；可能与“拒绝不使控制码失效”冲突 | 须修订 I18 / I21 与用例 `never_mode_kicked_device_can_return_until_code_changed`、F23 |
| 登录屏 / 锁屏无法显示指示条时禁止直连 | 避免在本机无法一键停止、无法显示 SAS / 路径指示时仍暴露地址 | 改变无人值守（尤其 Windows 登录屏）直连的可用性；运营者可能正是要在登录屏使用 never + 直连 | 须修订 I21 / I22 例外与用例 `never_direct_at_lock_screen_has_no_local_indicator`，以及 Windows 登录屏一节 |

Phase 0 报告还提过两项未列入上表的想法（可行性未证实，同样不实现、不设默认）：只通告公网映射地址；托盘显示最近一次直连的时间与控制端设备名。

### 先红验收测试清单

本表供另行安排的测试工作按「先红后绿」编写**先失败**的验收测试，以便含本规格变更的 PR 通过已合入的 `spec-requires-tests` 门禁（**无豁免**）。文件位置只使用 PR #2 `scripts/check-spec-tests.mjs` 认可的测试文件类型：

- `tests/*.mjs`
- `(crates/[^/]+|server|web|client/src-tauri|android/native|p2p)/(tests|examples)/[^/]+.rs`
- `client/tests/*.test.ts`
- 任意路径下的 `*_test.go`
- `android/app/src/test/**/*.kt`
- 或在已有 `.rs` 中新增 inline `#[test]` / `#[tokio::test]` / `#[cfg(test)]`（本表仍优先给出独立测试文件，避免把位置写到不被认可的目录）

本规格**不附测试代码**。完整行为仍以上文「验收测试清单」为准；下表是优先落地、且覆盖 r6 五项前提的先红子集，文件位置与上文清单已统一（协议用例放 `crates/airhand-proto/tests/wire.rs`，配置用例放 `client/src-tauri/tests/direct_config.rs`；会话 / 直连状态机在 I23 下沉后放 `crates/airhand-link/tests/direct.rs`，仍须留在桌面托盘 / 确认窗侧的用例放 `client/src-tauri/tests/`）。手动 / 真机项不写入本表（不能通过该门禁）。缓解选项决定之前不实现，也不为它们编写默认开启或默认关闭的断言。

**Phase 1 按方案 (a) 编写先红用例**；下表中方案 (b) 专属用例仅在改用 (b) 并修订规格后替换编写，不纳入本轮必写集合。

**“先红”的表达方式**（避免与已合入的 CI 冲突）：
- Rust 用例必须通过桩 API / 最小类型使所在 crate **编译通过**，在运行时断言失败；不得使用 `#[ignore]`，也不得引用尚不存在、会导致整个 crate 无法编译的符号（否则既有测试与 `clippy --all-targets -D warnings` 会一起失败）。Draft PR 上预期**只有本清单列出的先红用例失败**；为确认其余测试仍通过，编写测试的工作应在 PR 说明中附上 `cargo test --workspace --no-fail-fast` 与（若已有 `p2p/`）`go test ./...` 的结果。不要求为此改 CI。
- 若新增 `p2p/*_test.go`，必须同时给出能让 `go test` / CI 的 `go-helper` job 启动的最小骨架：`p2p/go.mod`（及必要的 `go.sum`）、被测包可编译的空实现或桩。不得只丢测试文件。
- 本 r6 Draft PR 在上述先红测试并入之前，`spec-requires-tests` 为红是预期状态，不是豁免；并入后该检查须变绿，其它检查不得因“未实现的 API”而编译失败。

| 测试名 | 对应不变量 / 功能 | 文件位置建议 | 断言内容 |
|---|---|---|---|
| `TestHostGateClosesOnRxSilence` | I3、§2 入站静默 | `p2p/gate_test.go` | 可注入路径源：首个入站之后连续 1000 ms 无入站，被控端闸门关闭，时限 ≤ 2 s；只把 `CurAddr` 置空不得作为唯一通过条件 |
| `TestHostGateRxSilenceNotArmedBeforeFirstInbound` | I3、I24 | `p2p/gate_test.go` | 开闸后、首个入站到达前，静默条件不得关闸 |
| `TestHostGateSilenceCloseIsStickyInEpoch` | I3 | `p2p/gate_test.go` | 静默关闭后本 `epoch` 不再打开，也不再发 `DirectGateOpen` |
| `TestHostGateWindowZeroPacketsOnSelfHostedDerp` | I3 窗口包数上限 | `p2p/gate_test.go` | 进程内自建 derper，不依赖外网；关闭时刻早于信任期过期时窗口内数据包为 0 |
| `TestHostGateClosedWhenCurAddrEmpty` | I3 | `p2p/gate_test.go` | `CurAddr` 为空时闸门关闭（仍须与静默条件同时存在） |
| `TestKDrivenGateStillSilentBeforeCommit` | I3、K 等效方案 | `p2p/gate_test.go` | 若实现 K 驱动，升级阶段仍须保留不依赖 Ping 的关闭条件，关闭 ≤ 2 s |
| `host_gate_not_closed_during_fence_wait_on_static_screen` | I3、I2、I24 | `crates/airhand-link/tests/direct.rs` | 栅栏等待期间、静止桌面、无控制流 Ping 时闸门保持打开；直连 `keep_alive_interval` ≤ 250 ms |
| `TestSchemeBSilenceStartsAtCacheFlush` | I3、I24 方案 (b) | `p2p/gate_test.go` | 仅在改用方案 (b) 时编写：静默从开闸并交付缓存起算，开闸前到达的缓存包不得立即导致粘性关闭 |
| `TestHostGateWindowIncludesStatusPollInterval` | I3、§2 | `p2p/gate_test.go` | 窗口计算计入 `Server.Status()` 轮询间隔（默认 100 ms） |
| `TestGateDropsWhenPathNotDirect` | I3、F5 | `p2p/gate_test.go` | 路径非 direct 时立即丢弃写入隧道的数据报 |
| `TestControllerGateIgnoresSlowButDirectPath` | I3、§2 | `p2p/gate_test.go` | RTT > 250 ms 且 ≤ 400 ms 的直连不被误判为非 direct |
| `TestClientPingMustNotBeUsedForPath` | I3、§2 | `p2p/gate_test.go` | 路径判定代码路径不得调用 `Client.Ping` |
| `first_quic_initial_not_lost_to_host_gate` | I24（Phase 1 按方案 (a) 落地；两方案共用此确定性断言） | `crates/airhand-link/tests/direct.rs` | 注入被控端闸门晚于控制端打开的时间差：升级期间因闸门关闭而丢弃的直连 QUIC 包数为 0，且不出现因 Initial 丢失而走 PTO 的重传 |
| `controller_waits_for_direct_gate_open_before_first_quic` | I24 方案 (a)、F4 | `crates/airhand-link/tests/direct.rs` | 未收到 `DirectGateOpen` 且本端闸门未开时，控制端发出的直连 QUIC 包数为 0（仅方案 (a)） |
| `host_sends_direct_gate_open_when_gate_opens` | I24 方案 (a)、F4 | `crates/airhand-link/tests/direct.rs` | 被控端闸门打开后经中转发 `HostMsg::DirectGateOpen { epoch }`，`epoch` 与当前升级一致（仅方案 (a)） |
| `late_or_mismatched_direct_gate_open_is_ignored` | I13 / I24 方案 (a) | `crates/airhand-link/tests/direct.rs` | 迟到或 `epoch` 不匹配的 `DirectGateOpen` 被忽略 |
| `host_inbound_cache_delivers_initial_before_gate_open` | I24 方案 (b) | `crates/airhand-link/tests/direct.rs` | 闸门未开时到达的 Initial 被缓存并在开闸后交付（仅方案 (b)） |
| `host_inbound_cache_is_bounded_and_cleared_on_epoch_end` | I24 方案 (b) | `crates/airhand-link/tests/direct.rs` | 缓存不超过 32 包或 64 KiB、超时丢弃、`epoch` 结束时清空，且只接受已授权对端 |
| `TestWriteTimeoutReportsPathFailure` | I25 | `p2p/helper_test.go` | 隧道 `Write` / `WriteTo` 以及候选 (ii) 往 Rust 的管道写超过 `write_timeout_ms`（默认 500 ms）即失败并上报，进程不永久阻塞 |
| `helper_write_stall_triggers_fallback` | I25、I2 | `crates/airhand-link/tests/direct.rs` | 假写路径超时后仍阻塞 ⇒ 回退中转、结束 helper 进程并拉起新进程后开新 `epoch`、会话继续，不发结束会话的 `Bye`；计入最多 3 次重试 |
| `direct_session_caps_bitrate_instead_of_falling_back` | I26、F10 | `crates/airhand-link/tests/direct.rs` | **注入较低的 C（例如 30 Mbps）**，目标码率高于该 C 时被限制在 C，且不发 `DirectFallback` |
| `fallback_to_relay_lifts_direct_bitrate_cap` | I26、F10 | `crates/airhand-link/tests/direct.rs` | 同样在注入的较低 C 下，回退中转后解除该上限 |
| `direct_config_bitrate_cap_default_is_100mbps` | 配置、I26 | `client/src-tauri/tests/direct_config.rs` | `bitrate_cap_kbps` 默认为 `100000`；`enabled` 默认关闭 |
| `direct_config_rejects_bitrate_cap_over_100mbps` | 配置、I26 | `client/src-tauri/tests/direct_config.rs` | `bitrate_cap_kbps` > 100000 被拒绝 |
| `direct_config_defaults_off` | F1、I1 | `client/src-tauri/tests/direct_config.rs` | `[direct] enabled` 默认 false |
| `disabled_by_default_never_spawns_helper` | I1 | `crates/airhand-link/tests/direct.rs` | 未启用时假 helper 工厂 0 次调用，无 `Direct*` 消息 |
| `direct_messages_wire_is_stable` | I4、F4、I24 | `crates/airhand-proto/tests/wire.rs` | 含 `DirectGateOpen` 在内的直连控制消息线格式稳定 |
| `protocol_version_is_10` | I4 | `crates/airhand-proto/tests/wire.rs` | `PROTOCOL_VERSION == 10` |
| `direct_quic_never_sends_datagram_over_1232` | I11 | `crates/airhand-link/tests/direct.rs` | 观测到的最大数据报 ≤ 1232；超限包被丢弃而非依赖库报错 |
| `no_offer_before_approval` | I12 / I16 | `crates/airhand-link/tests/direct.rs` | 取得直连同意前不发 `DirectOffer`、不启动被控端 helper |
| `controller_helper_no_network_before_offer` | I16 | `crates/airhand-link/tests/direct.rs` | 控制端在 `DirectOffer` 之前 helper 无网络活动（不拉 DERP 图、不连 DERP） |
| `approve_never_allows_direct_when_enabled` | I21 | `crates/airhand-link/tests/direct.rs` | `never` + `[direct] enabled` 且对端声明直连时不弹窗即可升级 |
| `never_mode_kicked_device_can_return_until_code_changed` | I18 / I21 已知风险 | `crates/airhand-link/tests/direct.rs` | 被“断开此控制端”的持码人在换码前可以再连回来；本用例记录已知行为，不把拒绝重连当成默认 |
| `approve_never_operator_config_counts_as_consent` | I21 | `crates/airhand-link/tests/direct.rs` | 两项显式配置视为同意，可发 `DirectOffer` / 启动 helper |
| `static_screen_does_not_trigger_fallback` | I2 | `crates/airhand-link/tests/direct.rs` | 无画面无声音数十秒，Ping/Pong 正常 ⇒ 不回退 |
| `missing_pong_triggers_fallback` | I2、存活检测 | `crates/airhand-link/tests/direct.rs` | 直连上 K 秒收不到 `Pong` ⇒ `DirectFallback`，会话保持中转 |
| `helper_crash_does_not_end_session` | I2、I10 | `crates/airhand-link/tests/direct.rs` | helper 崩溃只回退，不结束会话 |
| `no_switch_before_both_gates_open` | I3 | `crates/airhand-link/tests/direct.rs` | 任一侧闸门未开时不切换发送端 |
| `direct_token_is_one_time` | I5 | `crates/airhand-link/tests/direct.rs` | 令牌只能使用一次 |
| `TestEmptyOrInvalidAllowlistRefusesToStart` | I14、I9 | `p2p/helper_test.go` | `client_key` 为空或无法解析时拒绝 Start |
| `stop_sharing_rotates_code_old_code_rejected` | I18 | `crates/airhand-link/tests/direct.rs` | 停止共享换码，旧码立即被拒 |
| `disconnect_one_controller_keeps_code_and_closes_both_paths` | I18 | `crates/airhand-link/tests/direct.rs` | “断开此控制端”关两条路径、不换码 |
| `direct_secrets_are_redacted_in_debug_and_logs` | I7 | `crates/airhand-link/tests/direct.rs` | 地址 / 令牌 / 密钥不出现在调试日志明文里 |
| `verification_string_is_displayed_not_required` | SAS | `crates/airhand-link/tests/direct.rs` | 升级不因用户未比对校验串而阻塞或失败 |

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

**已由 r6 根据 Phase 0 云 VM 数据关闭的项**：超时与阈值（见配置表与 §2）、C = 100 Mbps（含 k ≈ 0.5 的写法与 VM 限制）、同进程多 `Server` 的 VM 结果（设计仍每会话一进程）、helper 体积与许可大类、I3 窗口包数上限（自建 DERP 上为 0）、升级双峰与写超时看门狗（I24 / I25）。

## r6 处理记录（Phase 0 五项前提与第 6 轮复审）

**项目负责人 2026-11-10 14:30（Asia/Taipei）决定，已写入本版**：

1. **被控端闸门加上入站静默条件**（默认 1000 ms 无入站即关）。Phase 0 实测：加上后关闭时限 1.02 s，窗口内经 DERP 的数据包为 0。允许由 Rust 侧存活判定 K 驱动闸门的等效实现，须满足同一 I3（≤ 2 s，自建 DERP 上窗口内 0 包）。
2. **修复升级耗时两档**（约 0.44 s 与约 1.40 s）：控制端须在收到 `DirectGateOpen`（或等效短时入站缓存）后再发首个 QUIC 包（I24）。不得只靠调小 PTO / `initial_rtt`。
3. **写超时看门狗**（I25）：helper `Write` / `WriteTo` 默认 500 ms 超时即回退中转、换新 `epoch`、不结束会话。对应不限速打满时卡在 gVisor UDP 写上的问题。限速不能替代该看门狗。
4. **码率上限 C = 100 Mbps**，按 C ≤ k×R 写入；R 取 Phase 0 QUIC 层吞吐（5 分钟只证实了 60 Mbps 下限；200 Mbps × 30 s 有余量，取 k ≈ 0.5）。数据来自云 VM、不是参考硬件，批准 (B) 前须用真机数据复核。I3 窗口内经 DERP 的数据包上限：自建 DERP 上为 0。
5. **never 模式的隐私问题写成已知风险**：持码人可以连上来拿到地址后立即断开、反复来访。缓解只作为「待项目负责人决定」的选项，不设默认值，不变量不依赖这些选项。
6. **真机测试是批准 (B) 的前提**；机器待项目负责人指定，本规格不做任何安排。
7. 规格内给出「先红验收测试清单」，供另行安排的测试工作编写先失败用例，以通过 `spec-requires-tests` 门禁（无豁免）。本文件不附测试代码。

**针对性复审（r6.1，0 条 S1）后又做的文字修订**：闭合复审新提出的 5 条 S2——方案 (b) 静默从交付缓存起算、缓存存活不超过静默阈值；升级阶段用 QUIC `keep_alive_interval` ≤ 250 ms 而非控制流 Ping；`rx_silence_ms` 下限 1000 ms；Phase 1 按方案 (a) 写先红用例；判据表“不通过时”措辞、写超时未测时的处理、通道未选定时两种都须测、20 Mbps CPU 走 go/no-go。S3 中测试放置口径、粘性只针对已打开过的闸门、`--no-fail-fast` 说明一并写入。未再开一轮复审（本轮已无 S1）。

**第 6 轮复审（2 条 S1）后的 r6.1**：已闭合 S1-1（静默计时从首个入站之后才开始、关闭粘性、闸门双向、I24 按方案 (a)/(b) 分写用例、缓存界限）与 S1-2（增加「批准 (B) 前真机复核判据」表：k = 0.5 即 R ≥ 200 Mbps × 5 min、C 码率 CPU 交 (B) 判断、托管 DERP 期望 0 包且须许可、被测对象为 spike、I24/I25 行为验收归 Phase 1）。并落实 S2/S3：K 驱动在提交前仍须保留静默且驱动闸门的 K 不得取 2 s；I25 重启 helper 进程并计入 3 次重试、看门狗覆盖往 Rust 的写；never 选项写明“决定之前不实现、现行行为是 r5 已批准语义”；先红的编译通过 / 运行时失败写法与 `p2p/` 最小骨架、Draft PR 门禁为红是预期状态；码率机制用例注入较低的 C；I26；配置范围；确定性 I24 用例；两份清单文件位置统一。

**仍待（不阻塞 r6.1 文本，但阻塞批准 (B) 或通道选定）**：

- 真机复核（NAT / 公网 / 托管 DERP / 真实 Wi-Fi / 参考硬件上的 C 与 CPU）；机器待项目负责人指定。
- 候选通道 (i) / (ii) 的选择（等 Windows 真机结论）。
- Windows 受限令牌 / AppContainer / 防火墙，以及 macOS 签名公证。
- never 模式五项隐私缓解选项（见上节，待项目负责人决定）。
- 控制端在 Offer 前是否已有网络活动：以用例 `controller_helper_no_network_before_offer` 验收，不把 spike 细节当作已关闭。

## 项目负责人批复记录（2026-11-10）

- **批准**：Phase 0 spike（(A)，独立分支，不合并、代码不复用）；全部建议的 go/no-go 阈值；方案 B（Go helper 子进程）与第三方许可声明；默认使用 Tailscale 托管 DERP 做引导；v10 服务器与全部客户端同步升级；Phase 1 仅桌面。
- **新增要求**：被控端必须确认（控制码不等于自动接受，尤其在暴露公网 / 内网地址之前）；被控端要有随时可用的主动断开；握手参考现成方案（见“被控端确认与主动断开”）。
- **工作流脚本 / CI**：移植为独立 PR，在 Phase 1 之前完成（见阶段表）。**事实记录**：PR #2 已合入本 fork 的 `main`；本 r6 工作不改该分支与仓库设置。
- **开放问题 11–14（2026-11-10 13:08 Asia/Taipei）**：默认每会话确认；`approve = "never"` 免确认且允许直连（显式配置视为同意）；停止共享换码、另配只断开此控制端；周期性续期默认关；校验串只显示、不强制比对。见上“r5 处理记录”。
- **r6（2026-11-10 14:30 Asia/Taipei）**：写入 Phase 0 五项前提、C 与 I3 包数上限、never 已知风险与待决缓解选项；真机测试列为批准 (B) 的前提（机器待指定）；先红验收测试清单写入规格，测试代码另行编写。

## 后续阶段（TODO，仅扩展点，本次不实现）

- **Phase 1.5**：网页控制台（`web/` 容器内置 helper）与 Android（无法像桌面一样拉子进程；候选方案 C′ `libtailcat`，需另行评审）。
- **Phase 2 — Tailscale**：已在 tailnet 内的两端不需要 tailcat：被控端把本机 tailnet 地址（Tailscale 使用的 CGNAT 段与 ULA 段）作为新的 `DirectOffer` `kind`（候选 UDP 地址列表）；直连 QUIC 监听绑定到该地址（需要非回环监听，安全模型要单独评审），仍用一次性令牌 + 证书固定。地址发现只枚举本机网卡；**airhand 不接触 Tailscale 登录 / auth key / API**。新增 `kind` 需升协议版本或用 `DirectDecline` 协商。
- **Phase 3 — ZeroTier**：同上；候选地址来自 ZeroTier 虚拟网卡（按网卡名前缀 / 用户配置的 CIDR 白名单枚举）。**网络 ID 与 token 不入库，airhand 不调用 ZeroTier 本地服务 API**。
- 共同点：升级 / 回退状态机复用；对应的新规格与评审另开。

## 风险

- **tailcat 无稳定性承诺**：API / 线格式可能变；对策：固定 `v0.7.0`（commit `15ab9e6`）并记录 `go.sum` 哈希；升级走单独 PR + 回归；两端 helper 版本不一致时握手失败即回退中转。
- **不能禁止 DERP 承载数据**：采用入站静默（或等效 K 驱动）后，自建 DERP 上 I3 窗口内数据包上限为 0（Phase 0 实测）。托管 DERP 的窗口期与限速**未测**，须真机复核；共享 DERP 仍可能限速、可撤销、可见元数据。默认关闭 + 可自建 DERP。
- **性能（云 VM 已测，真机未测）**：60 Mbps × 5 min 达标；200 Mbps × 30 s 有余量；400 Mbps 开始丢包；不限速会卡在 gVisor `WriteTo`（I25）。CPU / 延迟数字来自云 VM，不是参考硬件。C = 100 Mbps 须真机复核。
- **1232 字节上限与库行为**：Phase 0 证实超限写入返回 nil 并被静默丢弃；不得依赖库报错（I11）。
- **打洞失败率**：对称 NAT / 严格网络下只能走 DERP；本设计在窗口期之后退回自建中转，因此这类网络上**没有收益但也没有长期损失**。
- **控制流切换复杂度**：栅栏 + 回退重同步是整个设计中最容易出错的部分；复审与测试重点。
- **Windows SYSTEM 被控端**：Phase 0 未测受限令牌 / AppContainer（只验证交叉编译）。在真机结论出来之前不得假设可行；退路仍是禁用该被控端上的直连。机器待项目负责人指定。
- **升级首包被丢**：未实施 I24 时升级耗时呈约 0.44 s / 1.40 s 两档。Phase 1 必须先等 `DirectGateOpen`（或等效缓存）。
- **新增运行时依赖与供应链**：Go 工具链、tailscale.com / gVisor / wireguard-go 体积（待实测）与许可声明；安装包体积增长；杀毒软件对 Go / WireGuard 二进制的误报。
- **打包与平台**：Windows 防火墙、macOS 嵌套 helper 的签名 / 公证 / hardened runtime / 应用防火墙弹窗、卸载流程（`uninstall.rs`）清理 helper、Linux 各分发形态带上 helper——均为 F12 子项，各自需验收。
- **确认步骤改变桌面被控端的既有行为**：默认 `approve = "ask"` 后，无人值守 / 登录屏主机在不改配置时无法被连接（确认超时即拒）。登录屏 / 锁屏要保持可连接，须显式设 `approve = "never"`；若还要直连，须同时开启 `[direct] enabled`。Windows SYSTEM agent 在登录屏 / 安全桌面上弹确认窗不在 Phase 1 范围（ask 模式超时拒绝）。同时 `e2e.rs` 等既有测试需随之更新。
- **无人值守直连的暴露面（已知风险）**：见“never 模式的已知隐私风险”。持码人可以连上来拿到地址后立即断开、反复来访。现有对策仅限已决定的机制：两项配置都必须显式开启；放宽配置须本机确认、不在有输入权限的在途会话期间生效；托盘可显示时醒目警告；写入 `docs/security.md`；拒绝不会使控制码失效，运营者须用“停止共享 / 更换控制码”作废旧码，或关闭 `never` / `[direct] enabled`。登录屏 / 锁屏期间没有本机指示条与一键停止（I22 例外）。审计日志、限速、定期换码、拒绝被踢控制端重连、登录屏禁止直连等**不是**当前默认对策，见待决选项。实现不把控制码、地址或密钥写入仓库或日志（I7）。
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

### 待项目负责人决定（r6；不设默认值，不变量不依赖这些选项）

15. **never 模式隐私缓解**（2026-11-10 14:30 Asia/Taipei 要求列为选项，不写成默认设计）：审计日志；限速；定期换码；拒绝被踢掉的控制端重连；登录屏无法显示指示条时是否禁止直连。利弊见上节。
16. **真机测试所用的机器**（批准 (B) 的前提）：待指定。本规格不做任何安排。

另有技术项仍待数据、但不属于“选项默认值”问题：候选通道 (i) / (ii) 等 Windows 真机结论；Windows 受限令牌是否可行；托管 DERP 与参考硬件上的 C / I3 复核。

批准 (B) 尚未作出。C = 100 Mbps 与码率策略须在 (B) 时一并确认。

## 是否重大改动

**是**。理由：新增监听 / 子进程（权限与安全边界变更）、跨 `proto` / `link` / `client` / 新增 `p2p` 多个子系统、新增第三方运行时依赖、协议版本升级、新增 IP 暴露面。
按 workflow v2「角色与硬约束」：重大改动必须等待项目负责人批准；spec 与设计评审都不代替批准。批准结论记录在 PR 评论中。
