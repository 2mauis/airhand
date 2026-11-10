# tailcat 直连 Phase 0 spike 报告

> 状态：**spike 实测报告（不合并；spike 代码不进入 Phase 1）**。分支 `spike/tailcat-p2p-phase0`，基于本 fork `main`。
> 对照规格：`spec/tailcat-p2p` 分支 `docs/specs/tailcat-p2p.md` **r4 / r4.1**（HEAD `6099b5e`），以及项目负责人 2026-11-10 13:08（Asia/Taipei）的口头变更（见“同意步骤结论”）。
> 原则：所有数字都来自本次在 VM 内的真实运行；没有运行过的项目一律标 **「未测」** 并写明所需环境。文中所有地址都是文档保留地址（RFC 5737 `192.0.2.0/24`）或回环地址，主机名为 `derp.example.com`。

## 0. 结论先行

- **建议：有条件 GO 进入 Phase 1（仅桌面）。** 在 VM 里能测的阈值都已达标，下面三条除外，而这三条都有已验证或可直接实施的修正：
  1. **I3 窗口在被控端不达标**：只看 `Server.Status().CurAddr` 时，静默断路后被控端闸门要 **4.72 s** 才关（阈值 ≤ 2 s）。根因是 magicsock 的 UDP 信任期为 6.5 s。spike 给被控端闸门加了“流上 1000 ms 无入站数据报即关闭”这一条件后，实测 **1.02 s**，且窗口内经 DERP 的数据报为 0。r5 须把它（或等效的 Rust 侧 K 判定）写进 §2。
  2. **升级耗时呈双峰**：约 70% 的升级在 0.44 s 内完成，其余约 1.40 s。多出的 1 s 来自控制端的 QUIC Initial 被尚未打开的被控端闸门丢弃（被控端比控制端晚约 93–111 ms 判定 direct），只能等 quinn 的 PTO 重传。仍远低于 12 s 阈值，但 Phase 1 应修（见 §3 第 3 项）。
  3. **饱和时卡死**：不限速打满（helper CPU > 100%，约 500–900 Mbps）时，被控端 helper 的写协程会永久阻塞在 gVisor `gonet.UDPConn.WriteTo`，直连吞吐降到 0，直到 QUIC 空闲超时。经 helper 的限速 20 / 60 / 200 / 400 Mbps 吞吐运行（12 次）与 5 Mbps 的 180 次同意 / 升级运行中都未复现。Phase 1 须设直连码率上限，并加写超时看门狗：写超时即判直连失效、回退中转。
- **真实环境的关键项均未测**：真实 NAT 打洞、公网时延、Tailscale 托管 DERP 的窗口期、Windows 受限令牌 / AppContainer / 防火墙、macOS 签名公证、真实 Wi-Fi。这些在批准 (B) 之前必须补真机数据（见 §5）。
- **同意步骤（r4）可行，开销可忽略**：确认后才启动 helper，在关键路径上的纯开销约为 **6–9 ms**（控制端 spawn+init 约 5 ms，被控端 spawn 1–4 ms），对 12 s 阈值没有影响。确认前，两端 helper 启动次数为 0、DERP 连接数为 0、画面字节为 0。`approve="never"` 模式下直连同样可用：Hello→Commit p50 约 0.48 s。

## 1. 环境与方法

| 项 | 值 |
|---|---|
| 机器 | 云 VM，4 vCPU（`Intel(R) Xeon(R) Processor`，型号未公开），约 15 GB 内存，Linux 6.12。**不是**规格要求的“近三年主流桌面 / 笔记本”参考硬件；CPU 数字只能作量级参考 |
| 工具链 | Go 1.27.1（`GOTOOLCHAIN`），Rust 1.99.1；tailcat **v0.7.0**（commit `15ab9e6`），tailscale.com `v1.103.0-pre.0.20260916030321-a2263542f260`；quinn 0.11.12 / quinn-proto 0.11.19 / quinn-udp 0.5.16 |
| 拓扑 | 4 个网络命名空间：`relay`（本地自建 DERP+STUN，以及**真实的** `airhand-server`）、`host`、`ctrl`，另有一组 `host2` / `ctrl2` 用于并行 soak。地址 `192.0.2.1 / .10 / .20 / .11 / .21` |
| 局域网 | veth + bridge，RTT < 1 ms |
| “广域网” | 内核没有 netem / tbf / prio，所以自写了 TUN 用户态 L3 仿真器 `wanemu`：按方向设置时延、抖动、丢包，以及“只丢 UDP 的静默断路”，保持 FIFO 顺序，每条命令都带 µs 时间戳记录 |
| DERP | 自建（`derpserver` + `stunserver`，自签证书，内嵌 region，**不访问**任何 Tailscale 服务）。计数器每 100 ms 输出一次：中继数据包数、disco 包数、accepts、当前连接数 |
| 被测栈 | Go helper 子进程（`spike/tailcat-p2p/helper`）：被控端为每个 epoch 新建 `tailcat.Server`（`AllowedClients=[k]`、`ServedTCPPorts` 为空但非 nil、只服务一个 UDP 端口）；控制端用 `tailcat.Client` 加 DiscoPing 判定路径。Rust bench（`spike/tailcat-p2p/bench`）用 quinn 0.11 建直连 QUIC：`direct_transport_config()` = airhand `transport_config()` + `initial_mtu(1232)` + `min_mtu(1200)` + `mtu_discovery_config(None)`，`EndpointConfig::max_udp_payload_size(1232)`；三条流与令牌、证书指纹固定都走真实的 quinn 握手 |
| 回环候选 | (i) 回环 UDP，锁定对端 4 元组；(ii) 继承的两条匿名管道（fd 3/4，2 字节长度前缀），在 Rust 侧实现为自定义 `AsyncUdpSocket` |
| 中转对照 | 真实 `airhand-server`（`airhand_link::net::connect_signal` + `transport::open_session`），三条流 |
| 复现 | `spike/tailcat-p2p/scripts/{netns,services,run-lan,run-wan,run-consent,licenses}.sh` 与 `summarize.py` |

时间戳：所有进程都在同一台机器上，使用同一个系统时钟（`SystemTime` µs），所以跨命名空间的时间差可以直接相减。

## 2. 阈值逐项对照

| 指标（已批准阈值） | 实测 | 结论 |
|---|---|---|
| helper 额外 CPU，被控端约 20 Mbps 单会话：≤ 单核 30% | 被控端 helper **8.0%**（候选 i）/ **9.0%**（候选 ii）；控制端 helper 7.2% / 7.2%（20 Mbps × 60 s，0 丢包） | **达标**（VM，非参考硬件） |
| 局域网直连额外往返延迟：≤ 3 ms | 相对不经 helper 的同路径：(i) p50 +0.23–0.46 ms、p99 +0.30–0.40 ms；(ii) p50 +0.35–0.56 ms、p99 首测 +3.83 ms，3 组复测 +0.57–0.95 ms（每组 n = 2000） | **达标**（(ii) 首测 p99 为 VM 噪声，3 组复测 +0.57–0.95 ms） |
| 单会话持续吞吐 ≥ 60 Mbps × 5 min，QUIC 丢包 < 0.1% | (i) 60.0 Mbps，每秒最低 59.6，**0 / 1,922,570** 丢包；(ii) 60.0 Mbps，每秒最低 59.8，**0 / 1,922,567** 丢包；MTU 1232。helper CPU：被控端 21.2% / 20.2%，控制端 19.1% / 16.3% | **达标** |
| 路径判定误判（直连稳定、RTT ≤ 400 ms、30 min）：0 次 | RTT 400 ms 稳定直连 1800 s：两端闸门离开 direct **0** 次（Ping 250 ms，M = 2，`max(750 ms, 3×RTT)`） | **达标** |
| 存活检测误回退（静止桌面 + Wi-Fi、30 min）：0 次 | 仿真 Wi-Fi（3–20 ms、抖动 15 ms、1% 丢包）1800 s，无画面：两端 K（2 s）超时 **0** 次，闸门离开 direct **0** 次 | **达标**（仿真 Wi-Fi；真实 Wi-Fi **未测**） |
| I3 窗口（自建 derper，人为切断后）：闸门关闭 ≤ 2 s；如实记录经 DERP 的数据报 | 控制端 **1.51 s**（6/6）；被控端只看 CurAddr 时 **4.72 s**（6/6，**超标**），加 1000 ms 入站静默条件后 **1.02 s**（6/6）。关闭前经 DERP 的数据包：只看 CurAddr 时 0–10 个，加静默条件后 0 个 | 控制端 **达标**；被控端按 r4 方案**不达标**，按 spike 修正方案**达标** |
| 升级完成时间 p50 ≤ 12 s（自 `DirectRequest` 起） | 局域网 DERP：p50 **1.01 s**（60/60 成功）；仿真广域网，中转单程 20 ms：p50 **2.19 s**（经本机 Unix 套接字交换 Offer，有偏差，见 §3 第 4 项）；**走真实中转并带同意步骤**：DirectRequest→Commit p50 **0.39 s**、p90 1.36 s（6 组 × 30 次 = 180/180 成功） | **达标**（仅限 VM；真实 NAT / 公网**未测**） |
| Windows 受限令牌 / AppContainer：至少一种能绑定 UDP 并运行 | 只验证了 `GOOS=windows` 交叉编译成功（19.3 MB） | **未测**（需 Windows 真机，以 SYSTEM 服务身份运行） |

## 3. Phase 0 清单 9 项

### 1. 吞吐 / 延迟 / CPU（候选 (i) 与 (ii)，对照中转）

局域网 RTT（控制流 ping/pong，n = 2000，ms）：

| 路径 | p50 | p90 | p99 | max | p50 额外 | p99 额外 |
|---|---|---|---|---|---|---|
| 不经 helper（plain QUIC） | 0.335 | 0.744 | 1.025 | 9.920 | 0 | 0 |
| (i) 回环 UDP | 0.563 | 0.849 | 1.322 | 10.159 | 0.229 | 0.296 |
| (ii) 管道 | 0.683 | 1.140 | 4.859 | 19.321 | 0.348 | 3.834 |
| 真实中转（同机） | 0.385 | 0.675 | 1.328 | 14.910 | 0.050 | 0.303 |

所有路径的 max 都在 10–19 ms，属于 VM 调度噪声。(ii) 的 p99 复测：见下表。

复测（同一拓扑，plain / (i) / (ii) 交替各 3 组，每组 n = 2000；“额外”均相对同一轮的 plain）：

| 轮次 | plain p50 / p99 | (i) p50 / p99 | (ii) p50 / p99 | (i) 额外 p50 / p99 | (ii) 额外 p50 / p99 |
|---|---|---|---|---|---|
| 1 | 0.259 / 0.961 | 0.715 / 1.355 | 0.817 / 1.907 | +0.46 / +0.39 | +0.56 / +0.95 |
| 2 | 0.413 / 1.000 | 0.702 / 1.380 | 0.815 / 1.783 | +0.29 / +0.38 | +0.40 / +0.78 |
| 3 | 0.314 / 1.045 | 0.666 / 1.446 | 0.707 / 1.611 | +0.35 / +0.40 | +0.39 / +0.57 |

复测中 (ii) 的 p99 额外为 0.57–0.95 ms，首测的 +3.83 ms 与那一轮所有路径 10–19 ms 的 max 同属 VM 调度噪声。按 4 组共 8000 个样本计，(i) 与 (ii) 的额外往返延迟在 p50 与 p99 上都 ≤ 3 ms。(ii) 每多经过一次管道拷贝，比 (i) 高约 0.1 ms（p50）。

吞吐（QUIC 层统计，均为限速发送）：

| 场景 | 平均 Mbps | 每秒最低 | QUIC 丢包 | 被控端 helper CPU | 控制端 helper CPU |
|---|---|---|---|---|---|
| 20 Mbps × 60 s，plain | 20.0 | 19.9 | 0 | — | — |
| 20 Mbps × 60 s，(i) | 20.0 | 19.9 | 0 | 8.0% | 7.2% |
| 20 Mbps × 60 s，(ii) | 20.0 | 19.9 | 0 | 9.0% | 7.2% |
| 60 Mbps × 300 s，(i) | 60.0 | 59.6 | 0 / 1.92 M | 21.2% | 19.1% |
| 60 Mbps × 300 s，(ii) | 60.0 | 59.8 | 0 / 1.92 M | 20.2% | 16.3% |
| 60 Mbps × 60 s，真实中转（MTU 1452） | 60.0 | 59.8 | 0 | — | — |
| 200 Mbps × 30 s，(i) ×2 | 200.0 / 200.0 | 199.8 / 199.7 | 0.005% / 0% | 51% / 48% | 44% / 42% |
| 200 Mbps × 30 s，(ii) ×2 | 200.0 / 200.0 | 199.6 / 199.6 | 0.03% / 0% | 44% / 40% | 37% / 33% |
| 400 Mbps × 30 s，(i) ×2 | 400.0 / 400.0 | 345.5 / 399.1 | 0.13% / **1.48%** | 79% / 83% | 72% / 77% |
| 400 Mbps × 30 s，(ii) ×2 | 400.0 / 400.0 | 398.9 / 399.2 | 0.24% / 0.02% | 78% / 67% | 64% / 55% |
| 不限速（目标 2000）× 20 s，plain | 2000.1 | 1996.4 | 2.09% | — | — |
| 不限速 × 20 s，(i) | **684.9** | 574.0 | 5.42% | 126.7% | 119.4% |
| 不限速 × 20 s，(ii) | **卡死**（见下） | — | — | — | — |
| 不限速 × 20 s，真实中转 | 2000.1 | 1996.6 | 4.44% | — | — |

**饱和卡死（新发现，Phase 1 风险）**：不限速打满时，经 helper 的吞吐先到 500–900 Mbps，然后**降为 0 且不恢复**，直到 QUIC 10 s 空闲超时。(ii) 复现多次，(i) 复现 1 次。经 helper 的限速 20 / 60 / 200 / 400 Mbps 吞吐运行（12 次）与 5 Mbps 的 180 次同意 / 升级运行都没有出现。卡死时抓取的 goroutine 栈显示：被控端 helper 的 Pump（本地→隧道）阻塞在 `gvisor.dev/gvisor/pkg/tcpip/adapters/gonet.(*UDPConn).WriteTo`（`gonet.go:708`，等待可写），但 tailscale netstack 的出站队列读取协程（`netstack.(*queue).ReadContext`）与全部 wireguard-go 协程都是空闲的。也就是说，gVisor UDP 端点认为发送缓冲区已满，实际却没有在途包，疑似发送缓冲计数泄漏。控制端两条桥接协程都在等读。路径闸门没有变化，也没有 `flow_end` / `drain_end` 事件。结论与建议：
- 根因在 tailcat 依赖的 gVisor / tailscale netstack 内，不在本设计范围内能修；应整理最小复现提交上游（spike 不对外提交）。
- Phase 1 必须：(a) 直连会话设码率上限（产品最高 100 Mbps；VM 里 200 Mbps 有余量，400 Mbps 开始丢包）；(b) helper 内 `Write` 加写超时（例如 500 ms），超时就上报“直连失效”，按 r4 回退中转并换新 epoch；(c) Rust 侧的 K 判定仍然兜底。

### 2. 超 1232 字节写入；quinn 0.11 配置名

- Go 用例（`helper/p2p/p2p_test.go`）：在 DERP / 早期路径与 direct 路径上，`ClientConn` 写 1233–60000 字节都**返回 nil（无错误），但数据报被静默丢弃**；1200 与 1232 能送达。所以 I11 不能依赖库报错，必须在 quinn 侧保证 ≤ 1232，并在 helper 的 Pump 处计数丢弃（spike 计数器 `oversize`）。
- quinn 0.11.12 的确切名称与默认值：`TransportConfig::initial_mtu`（默认 1200）、`min_mtu`（1200）、`mtu_discovery_config`（默认 `Some`，upper_bound 1452，interval 600 s）、`enable_segmentation_offload`（true）；`EndpointConfig::max_udp_payload_size`（默认 1472）。直连配置用 `initial_mtu(1232)` + `min_mtu(1200)` + `mtu_discovery_config(None)` + `max_udp_payload_size(1232)`，实测 `current_mtu = 1232`，没有超限数据报。**注意**：只设 `max_udp_payload_size` 而不提高 `initial_mtu` 时，MTU 会停在 1200。
- 自定义 `AsyncUdpSocket` 需要实现 `create_io_poller` / `try_send` / `poll_recv` / `local_addr` / `max_transmit_segments` / `max_receive_segments` / `may_fragment`，以及 `UdpPoller::poll_writable`；候选 (ii) 已按此实现并跑通。

### 3. DiscoPing 超时语义、CurAddr 时序、两端判定时间差、DERP 双发

- `DiscoPing` 对不可达的服务端受 ctx 约束（300 / 751 / 2000 ms 的 ctx 分别准时返回）。pong 带 `Endpoint` 即直连，否则经 DERP。控制端判定：pong 经 DERP 立即判非 direct；连续 M = 2 次在 `max(750 ms, 3×RTT)` 内收不到 direct pong 即判非 direct，Ping 间隔 250 ms。断路后控制端闸门 **1.51 s** 关闭（6/6，约等于 750 ms × 2 加一个间隔）。
- 被控端 `Server.Status().Peer[k].CurAddr`：magicsock 源码中 `trustUDPAddrDuration = 6.5 s`，只有在信任期内才填 CurAddr。实测静默断路后被控端闸门 **4.72 s**（6/6，轮询 100 ms）才关闭，超过 2 s。加入“flow 上 1000 ms 无入站数据报”条件（`HostConfig.RxSilence`）后为 **1.02 s**（6/6）。控制端在直连期间每 250 ms 发 Ping（r4 §2），静止桌面也有入站流量，不会因此误关；30 分钟 soak 验证见下表。
- **两端判定时间差**：被控端比控制端晚判定 direct，p50 **93 ms**（局域网）/ **111 ms**（仿真广域网），范围 −163 到 +114 ms。这正是“升级双峰”的根因：控制端一判定 direct 就发 QUIC Initial，此时被控端闸门还关着，Initial 被丢，只能等 PTO（约 1 s）重传。建议 Phase 1 任选其一：(a) 被控端闸门打开后经中转发一个“gate_open”提示，控制端收到后再拨 QUIC；(b) 被控端 helper 在闸门关闭时缓存少量入站方向的包，而不是丢弃；(c) 直连 Endpoint 调小 `initial_rtt`，使 PTO 约为 200 ms。
- **DERP 双发**：直连稳定期间，DERP 中继的数据包为 **0**（60 Mbps × 300 s 与 20 Mbps × 60 s，(i)/(ii) 各一次；disco 包也为 0）。断路后在被控端闸门关闭前，有 0–10 个数据包经 DERP 发出（只看 CurAddr 时；源码里信任期过后 `addrForSendLocked` 同时走 UDP 与 DERP）；加静默条件后为 0。

Soak（30 min；静止桌面，无画面；直连每 250 ms Ping；K = 2 s；被控端静默条件 1000 ms；两组并行）：

| 场景 | 时长 | Ping 数 | 控制端 K 超时 | 被控端 K 超时 | 控制端闸门离开 direct | 被控端闸门离开 direct | Ping RTT p50 / p99 / max | QUIC 丢包 |
|---|---|---|---|---|---|---|---|---|
| 直连 RTT 400 ms（每向 200 ms，稳定） | 1800 s | 7201 | **0** | **0** | **0** | **0** | 402.4 / 403.8 / 418.8 ms | 0 / 7216 |
| 类 Wi-Fi（接入链路 3–20 ms、抖动 15 ms、每条链路 1% 丢包） | 1800 s | 7201 | **0** | **0** | **0** | **0** | 23.9 / 114.8 / 252.8 ms | 65 / 7879 |

两组同时运行，共用同一个中转节点。路径判定误判 0 次、存活检测误回退 0 次，**两项都达标**，并且被控端开启了 1000 ms 入站静默条件，说明这个修正在 1% 丢包与抖动下没有引入误关。

### 4. `Client.Ping` 内部超时；DERP 图拉取 ⇒ 升级超时

- `Client.Ping` 对未授权客户端阻塞 **10001 ms**，返回 `context deadline exceeded`（内部固定 10 s）。授权客户端首个 ping 为 2 ms。所以判定路径不能用 `Ping`，spike 全部用 `DiscoPing` + ctx。
- DERP 图：只用 HTTPS 拉取了 tailcat v0.7.0 内置的默认 DERP 图 JSON（1383 字节；URL 见 tailcat 源码，本报告不重复），**没有**建立任何 DERP 连接。耗时 0.24–0.29 s，但 VM 出口疑似经过代理，不能代表用户网络。
- 升级耗时：局域网 DERP 为 p50 1.01 s / max 1.26 s；仿真广域网用 Unix 套接字交换 Offer 时为 p50 2.19 s / max 2.20 s（Offer 零时延到达，控制端在被控端 DERP 主连接还没建好时就拨号，导致 request→ctrl direct 要 1.10 s）；改用**真实中转**交换 Offer（同意步骤测试）后，Offer→ctrl direct p50 为 0.215 s，DirectRequest→Commit p50 0.39 s / p90 1.36 s / max 1.36 s。
- **建议**：`upgrade_timeout_secs` 取 12–15 s。VM 内最差为 2.2 s，但真实 NAT 打洞、DERP 冷启动与公网时延**未测**，先保留余量，真机数据出来后在 r5 收紧。

### 5. `AllowedClients` 为空 / 无效公钥

- 空列表：任意随机客户端都能 `Ping` 成功（err = nil）。**证实“列表为空即放行所有”**，I14 必要。
- spike 的 `ParseClientKey` 对空 / 无法解析 / 全零公钥拒绝启动（`TestEmptyOrInvalidAllowlistRefusesToStart` 通过）；被控端从不调用 `AddAllowedClient`。
- 非授权客户端：`Ping` 10 s 超时（见上），拿不到任何服务。I9：TCP 22 端口 SYN 3001 ms 超时（无 RST），未服务的 UDP 端口收不到任何东西。

### 6. 同一进程内多个 `Server`

- 同一进程内 3 个 `Server` 互相独立，关掉其中一个不影响其余；13 次 Server 生命周期前后 goroutine 数都是 5（无泄漏）。spike 采用“每会话一个 helper 进程、每 epoch 新建 Server”，在本 VM 内未见问题。
- 注意：第 1 项的饱和卡死发生在单个 flow 内，与多 Server 无关；但卡死期间同一进程内其他 Server 是否受影响**未测**。

### 7. Windows：受限令牌 / AppContainer / 防火墙

**未测。** 只确认了 `GOOS=windows GOARCH=amd64` 能交叉编译（19.3 MB）。需要的环境：Windows 10/11 真机或 VM，以 SYSTEM 服务身份运行被控端 agent，用 `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE)` + deny-only SID + 低完整性 + Job 对象启动 helper，并以 AppContainer 作对照；验证 UDP 绑定、网卡枚举、默认 DACL 下 Go 运行时是否正常，以及 Windows 防火墙是否弹窗。结论出来之前，按 r4 的退路处理：Windows SYSTEM 被控端禁用直连。

### 8. helper 体积与许可；macOS 签名

| 目标 | 体积（`-trimpath -ldflags="-s -w"`，CGO 关闭） | gzip -9 |
|---|---|---|
| linux/amd64 | 19.3 MB | 7.6 MB |
| linux/arm64 | 17.9 MB | 6.9 MB |
| windows/amd64 | 19.3 MB | 7.5 MB |
| darwin/arm64 | 17.8 MB | 6.9 MB |
| darwin/amd64 | 19.1 MB | 7.5 MB |

未 strip 的 linux/amd64 为 29.6 MB。许可（`scripts/licenses.sh`：按 GOOS 做 `go list -deps`，并识别每个模块的 LICENSE）：linux 41 个模块（Apache-2.0 7、BSD-2-Clause 3、BSD-3-Clause 20、ISC 1、MIT 10），windows 38 个，darwin 36 个；**没有 GPL / LGPL / MPL**。macOS 嵌套 helper 的签名 / 公证 / hardened runtime / 防火墙弹窗：**未测**（需要 macOS 真机与 Developer ID）。

### 9. 栅栏等待上限、Commit 等待 T、Ping 间隔与 K

栅栏（被控端同时在中转控制流与直连控制流上写同一标记，统计到达时间差；直连单程 1 ms，n = 200）：

| 中转单程 | 中转 RTT p50 / p99 | 中转晚到 p50 | p99 | max |
|---|---|---|---|---|
| 5 ms | 22.3 / 23.3 ms | 9.6 ms | 10.7 | 11.2 |
| 20 ms | 82.8 / 85.3 | 39.9 | 40.7 | 40.9 |
| 50 ms | 203.0 / 204.6 | 99.8 | 101.2 | 101.5 |
| 100 ms | 403.4 / 404.5 | 200.1 | 201.2 | 205.4 |
| 200 ms | 803.7 / 805.5 | 400.2 | 401.2 | 401.3 |

（表中“中转单程”是每个端点到中转节点的单程时延，被控端→中转→控制端共两段。）中转晚到 ≈ 中转端到端单程 − 直连单程，1000 个样本里直连**全部**先到。同意步骤测试里，直连上的 `DirectCommit` 比中转上的 `DirectSwitch` 平均早到约 38–40 ms（180/180），说明**控制端的栅栏缓存在每次升级时都会用到**，不是边角情况。建议：

- 栅栏等待上限：自适应，取 `max(1 s, 2 × 中转 srtt)`。不写死 500 ms：中转 RTT 达到 800 ms 时，晚到量已是 400 ms，500 ms 的上限没有余量。
- Commit 等待 T：quic→commit 实测 3.8–4.2 ms（直连 1 ms）。T = 2 s 足够，可写成 `max(2 s, 4 × 直连 RTT)`。
- Ping 间隔 250 ms、K = 2 s（加被控端 1000 ms 入站静默）：见上面的 soak 表。K 的形式取 `max(1.5 s, 4 × 间隔 + 2 × RTT)`，在 RTT 400 ms 时为 1.8 s；spike 用 2 s。

## 4. 同意步骤结论（r4 新增，以及 2026-11-10 13:08 变更）

### 4.1 被测内容（spike `bench/src/consent.rs`）

握手全部跑在**真实 `airhand-server` 中转的会话控制流**上（JSON 行替代对应的 postcard 变体，名称一致）：

1. 控制端经中转建立会话、打开三条流，发 `hello { direct: true }`。随后**故意**再发两条 `settings`，以及 `input`、`clipboard`、`direct_request`（假公钥）和 `ping` 各一条，用来检验 I15。
2. 被控端（`approve="ask"`）只回 `awaiting_approval`，不发 `HostHello`、不发画面、不启动 helper。等待期间 `ping` 照常回 `pong`；`settings` 只记下最新值；其余消息一律丢弃并计数。
3. 模拟用户在 2 s 后点“允许”（勾选“允许直连”），或“拒绝”，或不应答（60 s 超时）。
4. 允许后，被控端发 `host_hello { granted: { input, clipboard, audio, direct } }` 并开始在中转上发画面（5 Mbps 限速字节流）。**仅当 `granted.direct` 为真时**，控制端才启动 helper、取 `client_key`、发 `direct_request`；被控端才启动 helper、新建 `Server`、回 `direct_offer`。之后按 r4 的顺序：被控端先在中转上发 `direct_switch`，再把 `commit` 作为直连控制流的第一条消息发出，画面切到直连。
5. 结束方式：“停止共享”（被控端经中转发 `Bye(CodeChanged)`，两条路径一起关闭，结束 helper）、“断开此控制端”（`Bye(Kicked)`，同样关闭两条路径，不换码）、控制端主动 `Bye`。
6. 独立旁证：确认前后都读取本地 DERP 的 `accepts` / 当前连接数；两端进程内都有 helper 启动计数。

### 4.2 结果

| 检查 | 结果 |
|---|---|
| 确认前不启动 helper（I16），两端 | 控制端 0、被控端 0：ask 冷启动 2 组 60 次、never 2 组 60 次（helper 在 `host_hello` 之前）、拒绝 3、不直连 3、控制端 Bye 3、超时 1，全部为 0（预热对照组按设计为 1，不计） |
| 确认前不联系 DERP（I16） | 确认时刻 DERP `accepts` 增量 **0**、当前连接 **0**（全部运行）；拒绝 / 超时 / 未授予直连的会话，整个会话结束时 DERP 增量仍为 **0**；授予直连的会话在确认后为 +2（两端各一条连接） |
| 确认前不发画面（I15） | 被控端计数：确认前画面字节 **0**（全部运行）。控制端侧有 5/30 次在 `host_hello` 被读到之前已收到 6250 字节，这是**测量竞态**：那是确认之后发出的第一个 10 ms 画面块，走的是另一条流，比控制流上的 `host_hello` 先被读到 |
| 确认前的消息处理（I15） | `input`、`clipboard`、`direct_request` 各丢弃 1 条；两条 `settings` 只保留最后一条，确认后生效（`{"bitrate_kbps": 2000}`）；等待期间 `pong` 正常 |
| 拒绝 | `Bye(Declined)`，Hello→Bye 为 2.08 s（= 模拟的 2 s 加一个中转 RTT），helper 0 次，DERP 0 |
| 不应答 | `Bye(Unanswered)`，Hello→Bye **60.08 s**，helper 0 次，DERP 0 |
| 授予但不含“直连” | 不合规控制端发来的 `direct_request` 被回 `direct_decline(not_granted)`，被控端 helper 0 次，DERP 0 |
| 停止共享 / 断开此控制端 → 两条路径都停 | 6 组 × 30 次：中转画面流结束 p50 **47.7 ms** / max **52.3 ms**；直连上最后一个画面字节不晚于停止后 2.2 ms；被控端 helper 退出不晚于 12.3 ms；控制端收到 Bye 后 helper 退出不晚于 4.0 ms（阈值 2 s）。**注意**：被控端关闭直连后立刻结束 helper，QUIC CONNECTION_CLOSE 只有 13–22/30 次能送达控制端，其余依靠中转 `Bye` 收尾。Phase 1 可以在结束 helper 前等一个直连 RTT，或者明确以中转 `Bye` 为准 |
| 控制端主动 Bye（I19） | 被控端 helper 在控制端发 Bye 后 46.6–51.9 ms 内退出（3/3，其中约 40 ms 是中转单程） |

### 4.3 时延：“确认后才启动 helper”的纯开销

测量起点统一取被控端发出 `host_hello` 的时刻（即模拟用户点击之后），**不含人工确认等待**。中转单程 20 ms × 2 段，直连 1 ms：

| 组（各 30 次，0 失败） | 确认→Commit p50 / p90 / max | 快峰（< 1 s）次数与 p50 | 慢峰（≥ 1 s）次数与 p50 | 关键路径上的 helper 启动 p50 |
|---|---|---|---|---|
| ask，确认后冷启动，(i) | 439 / 1405 / 1411 ms | 23 次，436.8 ms | 7 次，1404.6 ms | 9.4 ms（控制端 spawn+init 5.0 + 被控端 spawn 4.3） |
| ask，确认前预热（违反 I16，仅作对照），(i) | 1392 / 1395 / 1398 ms | 15 次，430.3 ms | 15 次，1393.6 ms | 0 |
| ask，确认后冷启动，(ii) | 441 / 1403 / 1405 ms | 21 次，437.2 ms | 9 次，1401.2 ms | 5.9 ms |
| ask，确认前预热，(ii) | 435 / 1395 / 1396 ms | 16 次，430.6 ms | 14 次，1394.3 ms | 0 |
| **approve="never"**，(i) | 440 / 1402 / 1405 ms | 21 次，438.2 ms | 9 次，1401.7 ms | 8.7 ms |
| **approve="never"**，(ii) | 441 / 1404 / 1443 ms | 18 次，439.7 ms | 12 次，1402.8 ms | 6.1 ms |

- **纯开销 ≈ 6–9 ms**（直接计时：控制端 spawn+init p50 4.6–5.0 ms，被控端 spawn 1.2–4.3 ms；被控端 `Server` 启动 `serve` 约 3 ms，预热时也同样需要，因为 `AllowedClients` 要等 `client_key` 到达后才能确定）。快峰 p50 冷启动比预热多 6.5–6.6 ms，与直接计时一致。
- p50 的差异主要来自双峰（第 3 项的 QUIC Initial 问题）落在哪个峰，而不是 helper 启动：预热组反而有更多运行落入慢峰，说明这是时序巧合，与 I16 无关。
- 对阈值的影响：12 s 的阈值相比，6–9 ms 可以忽略。**未测**：真机首次启动（冷磁盘、杀毒扫描，Windows 上可能达到数百 ms）、macOS Gatekeeper 首次校验。
- ask 模式下用户可见的总耗时 = 人工确认时间 + 约 41 ms（`host_hello` 经中转）+ 上表。
- never 模式：Hello→HostHello 为 83.5 ms（一个中转 RTT，没有确认窗），Hello→Commit p50 **481.5 ms**（(i)）/ **483.4 ms**（(ii)），落入慢峰时约 1.45 s。DirectRequest→Commit 与 ask 模式相同（p50 393–394 ms），也就是说两种模式下“升级本身”的耗时一样，差别只在有没有人工确认。

### 4.4 2026-11-10 13:08 变更对规格的影响（r5 须更新）

项目负责人的变更：默认仍为逐会话确认（`approve="ask"`）；无人值守主机显式设 `approve="never"`（不弹窗），**该模式下允许直连**；“停止共享”换码，另有“断开此控制端”（不换码）；周期性续期提示默认关；校验串只显示、不强制。

- 这与 r4 的 **I21**（“`approve = "never"` ⇒ 直连整体禁用”）、§5 中“`approve = "never"` 时直连整体禁用”、配置表中 `approve` 一行、验收用例 `approve_never_disables_direct` 相冲突，r5 须改写。spike 已按新口径实现 never 模式：不发 `awaiting_approval`，立即发 `host_hello`，`granted.direct` 取被控端 `[direct] enabled`。
- **never 模式下的地址交换依据**：此时向持码人暴露本机公网 / 内网地址，依据是**被控端的常设配置**，也就是运营者显式写下 `approve="never"` 且 `[direct] enabled = true` 这一 opt-in，而**不是**当次会话里的人工同意。报告据此认定：持码人在 never 模式下可以**无人知晓**地获得这些地址。

**never 模式的隐私 / 安全关注点（请项目负责人确认是否接受，或在 r5 增加缓解）**：
1. **静默获取地址**：ask 模式下获取地址需要人在场点击；never 模式下，任何拿到控制码（或重连凭证）的人连上即可触发直连，几秒内从 magicsock 的 CallMeMaybe / 端点通告中拿到被控端的公网地址、局域网地址与 VPN 网卡地址，然后断开，现场无人察觉。“持码即可完全控制”的现有信任模型确实覆盖这一点，但控制行为在画面上可见，地址收集则不可见。建议：never 模式下直连会话写入本地审计日志，并在托盘显示最近一次直连的时间与控制端设备名。
2. **持久凭证扩大窗口**：被拒绝或被断开的控制端仍持有重连凭证（r4 已写明“拒绝不会让控制码失效”），在 never 模式下可以反复回来收集地址（例如跟踪被控端位置变化）。建议：never 模式的文档明确要求定期“停止共享”（换码），并考虑为直连另设“仅已保存设备”开关。
3. **猜码面**：控制码是 6 位数字，服务器只在码不存在时按 IP 记失败；never 模式没有弹窗，也就没有“猜中码会惊动用户”这层间接防护。建议：被控端本地对 `DirectRequest` 按来源 / 设备限速；never 模式下 `[direct]` 默认仍为关闭，需要第二个显式开关。
4. **第三方可见性**：never 模式下每个会话都在无人值守时向 DERP 图服务与 DERP 中继暴露被控端公网 IP 与连接时间（默认 Tailscale 托管）。建议 never 模式的文档推荐自建 `derper`。
5. **无人值守主机上的远程可达攻击面**：helper（tailcat / wireguard-go / gVisor）在每次直连会话中被远端驱动，并且没有人在场。第 1 项的饱和卡死说明这一层存在可被远端触发的异常状态（目前只观察到影响本会话的直连通道）。建议 never 模式同样启用写超时看门狗与码率上限，并把 helper 的崩溃 / 卡死计入本地审计。
6. **网卡地址范围**：magicsock 默认通告所有网卡地址。可以调研 tailcat / magicsock 能否只通告 STUN 得到的公网映射地址（不通告局域网 / VPN 地址），作为 never 模式的可选收窄。**未证实**是否可行。

## 5. 未测项与所需环境

| 项 | 为什么 VM 内测不了 | 所需环境 |
|---|---|---|
| 真实 NAT 打洞成功率、对称 NAT 回退 | VM 内只有命名空间 + 仿真器，没有真实 NAT 行为；已有 MASQUERADE 但未跑 | 两台位于不同运营商 / 不同 NAT 类型网络（家用宽带、4G/5G 热点、公司对称 NAT）的真机 |
| 公网时延 / 抖动、真实 Wi-Fi | `wanemu` 只是参数化仿真（Wi-Fi 组为 3–20 ms、15 ms 抖动、1% 丢包） | 真实 Wi-Fi（2.4 / 5 GHz、漫游）与跨城、跨洲链路 |
| Tailscale 托管 DERP 的窗口期、限速 | 规定不使用真实 tailcat / Tailscale 地址；spike 只用自建 DERP | 获准后使用默认 DERP 图与托管 DERP，实测断路窗口内的 DERP 包数与限速 |
| 参考硬件 CPU | VM（4 vCPU Xeon），没有编码器负载 | 近三年主流桌面 / 笔记本（例如 4–8 核移动 CPU），1080p60 实际编码同时运行 |
| Windows 受限令牌 / AppContainer / 防火墙 | 没有 Windows 环境 | Windows 10/11，SYSTEM 服务 agent，按 §3 第 7 项 |
| macOS 嵌套签名 / 公证 / 防火墙弹窗 | 没有 macOS 与 Developer ID | macOS 13+ 真机与 Apple Developer ID |
| 真机首次启动 helper 的冷启动开销 | VM 内二进制在页缓存里，杀毒软件不参与 | Windows（Defender 实时扫描）/ macOS（Gatekeeper）首次运行 |
| 确认窗 / 指示条的显示层行为、注入输入防护、锁屏 | spike 只模拟“用户决定”，没有 UI | 桌面 UI 实现后手动验证（r4 I22 已归入手动项） |
| 回退后重新升级（新 epoch） | spike 不实现回退状态机；断路后 QUIC 在 10 s 空闲后关闭，被控端会话结束 | Phase 1 实现后按 I13 用例测 |

## 6. 对 r5 的具体建议

1. §2 被控端判定：`CurAddr` 加“流上 X ms 无入站即关闭”（spike：X = 1000 ms，直连 Ping 间隔 250 ms），或改用 Rust 侧的 K 判定直接驱动 helper 闸门；I3 窗口 = max(静默阈值, K) + 轮询间隔。加用例 `TestHostGateClosesOnRxSilence`。
2. 升级握手：解决 QUIC Initial 被被控端闸门丢弃的问题（gate_open 提示、短暂缓存，或调小 `initial_rtt`）；加用例 `first_quic_initial_not_lost_to_host_gate`。
3. 直连码率上限与 helper 写超时看门狗（饱和卡死）；加用例 `helper_write_stall_triggers_fallback`（假 netstack 注入写阻塞）。
4. `upgrade_timeout_secs` 暂定 12–15 s；栅栏上限 `max(1 s, 2 × 中转 srtt)`；T = `max(2 s, 4 × 直连 RTT)`；Ping 250 ms；K 取 `max(1.5 s, 4 × 间隔 + 2 × RTT)`；`Server.Status()` 轮询 100 ms（≤ 250 ms）。
5. 候选通道：(i) 与 (ii) 在吞吐与 CPU 上相当；(ii) 不开本机端口，但 RTT p99 偏高、饱和卡死更易复现。是否选 (ii) 要等 Windows 结论（AppContainer 默认禁止回环，倾向 (ii)）。spike 不下结论。
6. 按 2026-11-10 13:08 的变更改写 I21 及其相关条目，并在 §5 加入 never 模式的地址交换依据与上节的隐私缓解。
7. 停止共享时，结束 helper 前等一个直连 RTT，或者在规格里明确以中转 `Bye` 为准（CONNECTION_CLOSE 只有约 50–70% 能送达）。

## 7. 安全与隐私自查

推送前对 `git diff main`（本分支新增的全部内容）与提交元数据做了扫描：

| 检查 | 方法 | 结果 |
|---|---|---|
| IPv4 地址 | 新增行中的全部 IPv4 | 只有 `0.0.0.0`、`127.0.0.1`、`127.0.0.2`（管道候选的占位地址）与 `192.0.2.0/24`（RFC 5737 文档地址） |
| IPv6 地址 | 正则扫描 | 无 |
| 邮箱 | 正则扫描 | 无（提交元数据只用 `users.noreply.github.com`） |
| 密钥 / 凭据 | `nodekey:`、`discokey:`、`privkey:`、`tskey-`、auth key、PEM 私钥 | 无。spike 在运行时生成的节点公钥、QUIC 证书指纹与令牌只在进程内与中转控制流上传递，bench 写日志前会删掉 `addr` / `client_key`；运行日志与原始结果**没有**提交 |
| 真实 tailcat / Tailscale 地址 | 主机名扫描 | 只有 Go 模块路径（`tailscale.com`、`github.com/tailscale/tailcat` 及版本号）；DERP 一律自建，主机名为 `derp.example.com`；报告不写默认 DERP 图 URL |
| 个人信息 / 网络 ID / 主机名 | 人工检查报告与脚本 | 无 |
| 署名 | `git log main..HEAD` | 4 个提交的作者与提交者都是 `Cursor (for 2mauis) <2908005+2mauis@users.noreply.github.com>`，均带 `Agent: cursor` trailer；提交信息不含模型档位或 token 用量 |
| 范围 | `git diff main -- server/` | 0 行；没有改 `server/`、`main`、`spec/tailcat-p2p`；只推送到本 fork 的 `spike/tailcat-p2p-phase0`；没有 force push，没有 `--no-verify`，没有改仓库设置，没有创建凭据 |

spike 自身的安全相关行为（不是对 Phase 1 实现的承诺）：被控端 `Server` 的 `AllowedClients` 在 `Start` 前给定，空 / 无效 / 全零公钥拒绝启动；`ServedTCPPorts` 为空但非 nil；只服务一个 UDP 端口；tailcat 的 `Logf` 一律丢弃；直连 QUIC 使用一次性令牌与临时自签证书的指纹固定。

## 8. 代码位置

- Go helper 与测试：`spike/tailcat-p2p/helper/`（`p2p/` 核心与测试，`cmd/airhand-p2p-spike` 是 helper，`cmd/localderp` 是自建 DERP+STUN，`cmd/wanemu` 是 TUN 仿真器）
- Rust bench：`spike/tailcat-p2p/bench/`（独立 workspace，依赖本仓库的 `airhand-link` / `airhand-proto`；`consent.rs` 是同意步骤）
- 脚本：`spike/tailcat-p2p/scripts/`

再次声明：spike 代码不合并、不复用；Phase 1 按“先红后绿”重写。
