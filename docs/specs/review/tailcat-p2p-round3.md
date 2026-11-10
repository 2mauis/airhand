# tailcat 临时点对点直连：规格 r3 复审（第 3 轮）

## 结论

**有条件通过。r3 可以提交给项目负责人做批准 (A)，批准范围仅限 Phase 0 spike。**

- **本轮没有 S1。** 第 2 轮的 2 条 S1 已经关闭：
  - 存活检测改成了直连上的 Ping/Pong，不再看画面包；
  - 回退方向采用了"反向栅栏"方案 (a)。
- **S2-1 到 S2-7 基本都已关闭。**
- 新问题集中在三处，都可以在 Phase 0 之后的 r4 里改，**不阻断 (A)**：
  1. 回退**发起方**怎么处理直连上收到的消息，规格没写；
  2. 存活超时 K 的默认值和"须小于 2 秒"自相矛盾，余量也太小；
  3. go/no-go 阈值缺两项核心指标。
- 例外是新 S2-3：阈值本来就是要负责人在 (A) 时确认的。**强烈建议提交 (A) 时顺手补上缺的阈值**，或者在 (A) 的批准记录里注明"这几项阈值在 r4 补"。

---

## 第 2 轮 finding 关闭情况

| 编号 | 状态 | r3 依据 | 说明 |
|---|---|---|---|
| S1-1 媒体看门狗误回退 | **已关闭** | §2"存活检测"；I2；`static_screen_does_not_trigger_fallback`、`missing_pong_triggers_fallback` | K 的取值是否可行另有问题，见新 S2-2 |
| S1-2 回退方向缺顺序规则 | **基本关闭** | §1 第 7、8 步；§2"回退（反向栅栏）"；I6；`late_direct_input_after_fallback_is_discarded`、`fence_timeout_drops_buffer_and_releases_keys`、`host_fence_then_commit_is_first_direct_message` | 方案 (a) 已写入：栅栏超时丢弃缓存，Commit 排在栅栏之后。还差三点：发起方一侧的规则、栅栏之后中转上哪些消息仍要处理、"改投中转"的语义，见新 S2-1 |
| S2-1 Commit 丢失时控制端无超时 | **已关闭** | §1 第 7 步；I13；`commit_lost_controller_falls_back_within_bound` | 计时细节见 S3-2 |
| S2-2 `Bye` 走哪条通道 | **已关闭** | §1 第 8 步"终止消息"；I10；`bye_always_sent_on_relay` | 栅栏之后中转上的 `Bye` 必须照常处理，没写明，并入新 S2-1 |
| S2-3 路径判定语义 | **基本关闭** | §2"闸门"；I3；`TestGateDropsWhenPathNotDirect` 等 3 个 Go 用例；Phase 0 第 3 项 | 两处遗留：判定时长和"<2 s"约束冲突（新 S2-2）；被控端 `Status()` 的轮询间隔和 `CurAddr` 何时变化没定（S3-1） |
| S2-4 桌面没有同意流程 | **规格已关闭，feature map 未改** | I12；§1 第 4 步；§5；开放问题 10 | feature map 的 F4 还用旧用例名 `request_before_approval_is_dropped`；时序图仍写"本机用户同意后画面开始""同意之后才处理"，见 S3-5 |
| S2-5 允许列表为空时放行所有人 | **已关闭** | 事实表"安全属性"；§1 第 3 步；I14；`TestEmptyOrInvalidAllowlistRefusesToStart`、`empty_client_key_refuses_start` | 建议在 I14 补一句"helper 不调用 `AddAllowedClient`" |
| S2-6 Windows 验收名不符实、缺退路 | **已关闭** | §5"Windows SYSTEM 被控端"；`helper_token_is_restricted_on_windows`；Phase 0 第 7 项 | 已改名，补了 DACL、restricting SIDs、AppContainer 和退路。验收手段还有缺口，见 S3-4 |
| S2-7 两段批准缺保障 | **已关闭** | 阶段表 (A)/(B)、"复审"行；go/no-go 表；开放问题 4 | 写明了不合并、不复用 spike 代码，加了 r4 复审，范围变更须在 (B) 单独确认。阈值本身还不全，见新 S2-3 |
| S3-1 进程与 Server 模型 | **已关闭** | §2"并发与进程模型"；`TestMultipleServersInOneProcessAreIndependent` | — |
| S3-2 stdio 冲突 | **已关闭** | 方案 B (ii)；`TestDatagramPipeIsSeparateFromControlStdio` | — |
| S3-3 `SO_EXCLUSIVEADDRUSE` | **部分关闭** | 方案 B (i) | 规格写了，但验收清单里没有对应用例 |
| S3-4 2 s 队列过期会结束会话 | **部分关闭** | §2"写入失败不结束会话"；`direct_write_failure_requeues_on_relay` | 写了"转为回退"，但和 K 的约束互相矛盾，"改投"还会补发过期输入，见新 S2-1、S2-2 |
| S3-5 Ping/Ack 措辞 | **已关闭** | §2"存活检测" | — |
| S3-6 缺用例 | **已关闭** | 验收清单 | — |
| S3-7 tailcat 内部日志 | **已关闭** | §5"日志"；I7；`TestTailcatLogfDiscardedByDefault` | — |
| S3-8 500 ms 栅栏超时 | **已关闭** | §1 第 8 步（交给 Phase 0 校准；缓存期间 Ack/Ping 照常处理） | — |
| S3-9 `ReleaseAll` 语义 | **部分关闭** | §2"状态重同步" | 被控端本地 `release_all()` 写对了。但控制端的另外两条要求在现有代码里没有对应物，见 S3-3 |
| S4-1 tag 哈希 | **已关闭** | 文首；"风险"（记录 `go.sum`） | — |
| S4-2 "已核实"的范围 | **已关闭** | 事实表前的核对范围说明 | — |
| S4-3 背景措辞 | **基本关闭** | 第 18–19 行 | 已不矛盾，但同一句话说了两遍；开放问题 2 还留着"仅做引导"的说法 |
| S4-4 失效链接 | **未关闭** | 第 3 行、阶段表"设计评审"行 | `docs/specs/` 下仍然没有 `tailcat-p2p-review.md` |
| S4-5 F7 措辞 | **已关闭** | feature map F7 | — |
| S4-6 哈希校验的实际作用 | **已关闭** | §4 末段；`helper_hash_mismatch_disables_direct` | — |
| S4-7 mTLS | **已关闭** | 开放问题 9 | — |

---

## 新 findings

### S1（批准 Phase 0 spike 之前必须修订）

**无。** r3 里剩下的问题都是状态机细节、超时取值或阈值范围。spike 本身就是去实测和校准这些量的，不影响"能不能开始实测"这个决定。

### S2（重要，批准 (B) 之前必须关闭；S2-3 建议提交 (A) 时就补）

#### S2-1 反向栅栏只规定了接收方，回退发起方的行为没定；"改投中转"会补发过期输入

- **位置**：§2"回退（反向栅栏）"第 1、2 条，"写入失败不结束会话"；§1 第 8 步"控制流"；I6
- **问题**：
  1. **发起方怎么处理直连入站消息没写。** 规格只说"**收到** `DirectFallback` 的一方此后丢弃该 epoch 的直连控制消息"。
     - 以被控端发起回退为例：控制端要过一个中转单程才知道回退，在这期间它还在往直连上发 `Input`。
     - 被控端对这些消息是处理还是丢弃，没有规定。
     - 如果处理：直连上没有"最后一条"的标记，这些消息和中转上之后的消息谁先谁后无从判定，I6 不成立。
     - 如果丢弃：会丢掉大约一个 RTT 的输入，需要明确写出来，并靠 `release_all()` 补偿。
  2. **栅栏之后，中转上哪些消息仍须处理，没列出来。** 按升级栅栏规则，读到栅栏之后"只处理新通道"。但 `Bye`、`DirectFallback`，以及**另一方向**的 `DirectSwitch`，在栅栏之后仍然只走中转。实现时如果严格照着写，会把 `Bye` 也丢掉。控制端现在是靠 `HostMsg::Bye` 结束会话的（`crates/airhand-link/src/controller/session.rs:279`）。
  3. **"把排队消息改投中转"和现有队列的设计目的冲突。**
     - 现有队列故意**不在网络恢复时补发过时的远程操作**（`controller/queue.rs:5-7`，`MAX_AGE` 为 2 s，见 `queue.rs:21`）。
     - 由"队列过期"触发的回退，再把这些超过 2 s 的输入改投到中转，正好违背这一点。
     - 另外，正在 `write_chunk` 里卡住的那一条（`session.rs:195`）到底送没送到，无法判定：改投可能重复，不改投就丢失。规格没定。
- **建议**：
  - 写明发起方从发起那一刻起，**也**丢弃该 epoch 后续的直连控制消息，丢失靠被控端的 `release_all()` 和重同步补偿。或者改成：接收方以收到 `DirectFallback` 为准回一条 `DirectFallback` 作为确认，发起方在确认到达前继续处理直连消息，并设超时。二选一写进规格。
  - 列出栅栏之后仍须处理的中转消息：`Bye`、`DirectFallback`、对向的 `DirectSwitch`。
  - "改投"只限还没写出、而且没过期的消息：过期的 `Ordered` 输入直接丢弃；`Settings`、`Audio`、显示器这类状态由重同步覆盖。正在写的那一条一律视为丢失。
  - 补用例：`host_initiated_fallback_discards_inflight_direct_input`、`bye_on_relay_processed_after_fence`、`expired_input_not_replayed_on_relay_after_fallback`。

#### S2-2 存活超时 K：默认值和约束字面矛盾，Ping 间隔 1 s 时余量不够，"<2 s"的约束和其他规则冲突

- **位置**：§2"存活检测"（"K 秒（默认 2，须小于现有的 2 秒命令队列过期与写入失败判定）"）；§4"其它阈值"（"均须小于 2 秒"）；§2 闸门规则；Phase 0 第 9 项
- **对照代码**：

  | 现有机制 | 实际取值 | 位置 |
  |---|---|---|
  | 控制端 Ping 间隔 | 1 s，跟统计定时器绑在一起 | `controller/session.rs:302, 328` |
  | Ping 在队列里的合并方式 | 只保留一个，可以越过排队的输入 | `queue.rs:113` |
  | 控制端写超时 | 2 s，超时即会话失败 | `session.rs:195` |
  | 控制端命令队列过期 | 2 s | `queue.rs:21, 158` |
  | 控制端判定被控端无响应 | 20 s 没收到任何消息 | `session.rs:311` |
  | 被控端回 Pong | 收到 Ping 立即回 | `host/session.rs:170` |
  | 被控端判定控制端无响应 | `PEER_TIMEOUT` = 30 s | `host.rs:32` |
  | 被控端写卡住判定 | 10 s（不是 2 s） | `transport.rs:24` |

- **问题**：
  1. **字面矛盾**：默认 K = 2，而规则要求"小于 2 秒"。
  2. **余量太小**：Ping 每 1 s 一次，K < 2 s 意味着只能容忍不到 1 s 的额外延迟。以下任何一种情况都会让一次 Pong 迟到 1 s 以上，从而触发误回退：
     - QUIC 丢包后要等 PTO 才重传，连续丢包时 PTO 还会指数退避；
     - Wi-Fi 突发丢包；
     - 同一条控制流上前面排着一条较大的 `Clipboard`；
     - 闸门短暂关闭。

     误回退的代价是：30 s 后重试，3 次用完就永久留在中转。这和第 2 轮 S1-1 的后果一样，只是触发条件不同。
  3. **约束自相矛盾**：
     - §2 已经规定"直连写入失败 / 队列过期一律转为回退"。既然如此，"K < 2 s"这条约束就没有存在的理由了：它原本是为了赶在 2 s 判死**之前**检测出来。
     - §4 要求"路径判定的 M / RTT 系数也须小于 2 秒"，但 M = 2、`max(750 ms, 3×RTT)` 时，判定时长 ≥ 1.5 s；RTT > 333 ms 时就超过 2 s。这和 `TestControllerGateIgnoresSlowButDirectPath`（RTT > 250 ms 的直连不应被误判）互相冲突。
     - 被控端的写卡住判定是 10 s，不是 2 s，"写入失败判定"这个说法只对控制端成立。
- **建议**：
  - 去掉"须小于 2 秒"这条约束。改为明确：直连通道的 2 s 写超时和 2 s 队列过期**按通道区分**，在直连上一律转为回退（与 S2-1 一起改）。
  - 把 K 和 Ping 间隔脱钩：直连期间单独用更短的 Ping 间隔（例如 250–500 ms；`Ping` 是现有消息，不需要改协议）。K 取类似 `max(1.5 s, N×间隔 + 2×RTT)` 的形式，由 Phase 0 校准。
  - go/no-go 增加一项"误回退次数"阈值，见 S2-3。

#### S2-3 go/no-go 阈值缺两项核心指标，部分阈值无法直接测量

- **位置**："建议的 go/no-go 阈值"表；Phase 0 第 1、3、9 项
- **问题**：
  1. **I3 的窗口没有阈值。** "有界经共享 DERP 发出"是本特性对 DERP 的核心承诺。Phase 0 第 3 项会测窗口时长和双发时机，但表里没有 go/no-go 值，比如"路径退化后经 DERP 发出的数据报 ≤ X 个或 ≤ Y ms"。
  2. **存活检测的误回退没有阈值。** S2-2 是本轮最大的风险，表里只有"路径判定误判"一项，不覆盖误回退。
  3. **测试范围对不上**：阈值写的是"RTT ≤ 200 ms 时误判 0 次"，Go 用例测的是"RTT > 250 ms 不被误判"，两者不重合。
  4. **几项缺测量条件**：
     - "直连成立后升级完成时间"没写起点（是从 helper 报告 direct 算起，还是从发出 `DirectRequest` 算起）；
     - CPU 阈值没写参考硬件；
     - "丢包 < 0.1%"没写在哪一层统计（QUIC 层还是 tailcat UDP 层）。
  5. **吞吐阈值低于产品上限**：阈值是 60 Mbps，而 `QualitySettings.bitrate_kbps` 允许到 100 Mbps（`airhand-proto/src/session.rs:277`）。只达到 60 Mbps 时，直连会话里用户选了更高码率怎么办（限制码率，还是回退），没写。
- **建议**：
  - 补两行阈值：
    - "I3 窗口：自建 `derper` 下路径退化后，经 DERP 发出的数据报 ≤ __ 个 / 持续 ≤ __ ms"；
    - "误回退：直连稳定、静止桌面加 Wi-Fi 场景，30 min 内 0 次"。
  - 把误判阈值的 RTT 范围和用例对齐，例如都覆盖 ≤ 400 ms。
  - 写明起点、参考机型和统计层。
  - 写明直连会话的码率上限策略。
  - 这些最好在提交 (A) 时一起补上，因为表头写的就是"供项目负责人在批准 (A) 前确认"。

### S3（一般）

| 编号 | 位置 | 问题 | 依据 | 建议 |
|---|---|---|---|---|
| S3-1 | §2 闸门"被控端"；I3 | 被控端用多快的频率轮询 `Server.Status()` 没写。`CurAddr` 在路径失效后要多久才清空，也不知道：如果 magicsock 要等信任期过期才改（按 tailscale 的已知行为推断，**未证实**），被控端闸门可能在路径已经坏掉的几秒里一直开着。这段时间直连包只是丢掉，不会走 DERP，但信任期过期那一刻正好是可能开始经 DERP 双发的时候 | `tailcat.go:2614-2616`（Status 只给 `CurAddr` / `Relay`，没有时间戳） | 写明轮询间隔，例如 ≤ 250 ms。把"`CurAddr` 在路径失效后多久变化"加进 Phase 0 第 3 项，并计入 I3 窗口 |
| S3-2 | §1 第 7 步；I13 | 有两处没定：① 控制端的 T 计时，排在栅栏后面缓存着的 `DirectCommit` 算不算"已收到"；② Commit 超时时控制端从没离开过中转，"中转上的**第一条**消息是 `DirectFallback`"在这种情况下应当理解为"**下一条**"。另外，T 超时和 Commit 恰好同时到达时，控制端必须忽略自己回退之后才到的 Commit，这一点也没写 | §1 第 7、8 步；§2"回退" | 写明 T 以"读到 `DirectCommit`，不论是否还在缓存中"为准，回退后迟到的 Commit 一律忽略。补用例 `commit_after_own_fallback_is_ignored` |
| S3-3 | §2"状态重同步" | 控制端的两条要求在现有代码里没有对应物：① "控制端清空本地记录的按键状态"：Rust 控制端不记录按键状态，只在断开时发 `ReleaseAll`（`controller/mod.rs:372-375`）；② "重发当前的 `Settings` / `Audio` / 选中显示器"：Rust 控制端只是透传界面命令，不缓存最新值（`controller/mod.rs:330-335`）。按键状态在前端（TS / Kotlin），而 Phase 1 承诺不改前端行为 | 同左 | 删掉 ①：被控端的 `release_all()` 已经够用，之后前端多发一个 KeyUp 无害。② 改为"`airhand-link` 控制端按会话缓存最近一次 `Settings` / `Audio` / `SelectDisplay`"，写进 F9 |
| S3-4 | 验收"Windows（Windows CI / 真机）" | ① 本 fork 没有 CI（第 233–234 行），所以"Windows CI"并不存在，而且普通 CI runner 也不是以 SYSTEM 身份运行，需要专门的测试支架（以服务或 SYSTEM 身份拉起）；② 只断言了令牌属性，没有**功能**验证（受限令牌下能否完成一次真实的直连会话），也没有**反向**验证（降权是否真的生效，比如打不开父进程、写不了安装目录） | 第 189、233 行 | 标为"手动 / 真机，以 SYSTEM 服务身份运行"。补 `helper_under_restricted_token_completes_direct_session`（手动）和 `restricted_helper_cannot_open_parent_or_write_install_dir` |
| S3-5 | feature map F4、时序图 | 还残留旧的"同意"前提：F4 用的是 `request_before_approval_is_dropped`；时序图写"本机用户同意后画面开始""同意之后才处理"，和规格 I12 及 §1 第 4 步不一致 | feature map 第 13、46–47 行 | 改成 `request_before_host_hello_is_dropped`，时序图改为"`HostHello` 之后" |
| S3-6 | 方案 B (i) | 规格要求 `SO_EXCLUSIVEADDRUSE`，但没有对应用例（第 2 轮 S3-3 的遗留） | 验收 I9 一行 | 选 (i) 时补 `direct_listener_uses_exclusive_addr_on_windows` |
| S3-7 | 验收映射 | `bye_always_sent_on_relay` 归在 I6 下面，但 `Bye` 的规则写在 I10；I14 和 I9 合用一行。可以追溯，只是不便核对 | 验收清单 | 把 `bye_always_sent_on_relay` 移到 I10，I14 单独一行 |

### S4（建议）

- **S4-1**：第 3 行和阶段表里的 `tailcat-p2p-review.md` 链接仍然失效。可以把评审报告放进 `docs/specs/`，或者改链接。
- **S4-2**：第 18、19 行把"DERP 也是兜底通道"说了两遍，删掉一处。开放问题 2 的"仅做引导"改成"引导，以及窗口期内有界承载"。
- **S4-3**：§2 存活检测里"写入失败判定"的 2 秒只对控制端成立，被控端是 10 s（`transport.rs:24`），措辞写准确。
- **S4-4**：建议在 I14 补一句"helper 不调用 `AddAllowedClient`"，防止以后有人改成运行时追加公钥，重新引入"列表为空即放行"的窗口。
- **S4-5**：go/no-go 表最后一行写"任一不达标 → 降级或放弃"，Windows 那一行写"不达标 → 走退路"。建议写明 Windows 一行优先适用，不触发整体放弃。

### 关于"有无新的未证实断言"

已经对照代码核实的：
- 控制端每秒发一次 Ping（`controller/session.rs:302, 328`）；
- 被控端收到 Ping 立即回 Pong（`host/session.rs:170`）；
- 静止画面没有帧，也没有 Ack；
- 2 s 队列过期（`queue.rs:21`）和 2 s 写超时（`session.rs:195`）。

还没证实、需要在 Phase 0 落实的：
- 被控端 `CurAddr` 在路径失效后多久变化（S3-1）；
- "直连 QUIC 握手在升级超时内可以重试，能容忍两端判定的时间差"：合理，但应列进 Phase 0 第 3 项去实测。

其余 Windows / AppContainer 相关说法来自公开文档，r3 已经列为 Phase 0 验证项，处理得当。

---

## 回答

**r3 是否可以提交给项目负责人做批准 (A)（仅批准 Phase 0 spike）？**

**可以。** 没有 S1。(A) 的范围（只在 spike 分支、不合并、不复用 spike 代码）、spike 之后的 r4 复审、"范围变更须在 (B) 单独确认"都已经写进规格。

建议提交时一并做两件事，不阻断：
1. 按新 S2-3 补齐 I3 窗口和误回退两项阈值，并修正其余测量条件。补不了的话，在 (A) 的批准记录里写明"这几项在 r4 补"。
2. 顺手改掉 feature map 里残留的"同意"字样（S3-5）。

**批准 (B) 之前还需要：**

1. Phase 0 结果写成规格 r4：
   - 选定数据报通道 (i) 或 (ii)；
   - 给出各项超时的实测值：栅栏等待、Commit 等待 T、存活检测 K 和直连 Ping 间隔、M / RTT 系数、`Status` 轮询间隔；
   - 给出 I3 窗口的实测值，以及 magicsock 何时开始经 DERP 双发；
   - 确定 Windows 方案，或者采用"Windows SYSTEM 被控端禁用直连"的退路；
   - 每一项阈值都附上达标 / 不达标的数据。
2. 关闭新 S2-1：
   - 写明回退发起方怎么处理直连入站消息；
   - 列出栅栏之后仍须处理的中转消息；
   - 限定"改投中转"的语义，不补发过期输入；
   - 补对应用例。
3. 关闭新 S2-2：去掉"<2 s"约束，直连通道的写失败按通道转为回退，直连 Ping 间隔和 K 脱钩，按实测定值。
4. 按 Phase 0 实测数据核对 S2-3 的阈值，任何不达标项的降级或放弃决定，在 (B) 里单独确认。
5. 处理 S3-1 到 S3-7，尤其是：
   - S3-3：状态重同步要落在 `airhand-link` 里缓存最新的 `Settings` / `Audio` / 显示器；
   - S3-4：Windows 验收要有以 SYSTEM 身份运行的支架，加上功能验证和反向验证。
6. r4 再做一轮设计复审，直到没有 S1，然后项目负责人在 PR 里**显式批准 (B)**，再开新分支按"先红后绿"实现。