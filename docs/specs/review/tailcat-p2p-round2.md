# tailcat 临时点对点直连：规格 r2 复审（第 2 轮）

## 结论

**有条件通过。** 前提是先修订下面 2 条 S1；改完后，可以按"分两段批准"走第一段：只批准 Phase 0 spike。**现在还不能批准 Phase 1 编码。**

第 1 轮的 3 条 S1 有进展，但没有全部关闭：
- **S1-1（DERP 有界承诺）**：已关闭。
- **S1-2（控制流切换）**：部分关闭。升级方向写清楚了；回退方向还没定义，而且和 500 ms 超时规则互相矛盾。
- **S1-3（Windows 降权）**：已关闭，但前提是把它作为 Phase 0 的门禁来执行。方案本身没有硬伤，只是验收项名不符实，也缺"不可行时怎么办"的退路。

另有两个新问题：
- 新增的"媒体看门狗"和 airhand 现有行为冲突：画面静止时根本不发帧。按现在的写法，直连在普通桌面上会反复误回退。
- 我第 1 轮 S2-5 的前提有一部分是错的：**桌面被控端没有"本机用户同意"这一步**，r2 沿用了这个前提。这里一并更正。

---

## 第 1 轮 finding 关闭情况

| 编号 | 状态 | r2 依据 | 说明 |
|---|---|---|---|
| S1-1 I3 无法实现 | **已关闭** | I3；§2 第 1、2 条；风险"不能禁止 DERP"；验收 I3 | 已改为有界承诺；helper 内部加了丢包开关；风险写实了；测试已改名，并补了 derper 集成测试。检测语义还有细节没定，见新 S2-3 |
| S1-2 控制流乱序 / 丢失 | **部分关闭** | §1 第 7、8 步；§2"回退""重同步"；I6 | 升级方向的栅栏有了；回退方向、栅栏超时后怎么处理、Commit 和栅栏谁先谁后，都没定义，见新 S1-2 |
| S1-3 Windows 降权 | **已关闭（作为 Phase 0 门禁）** | §5"Windows SYSTEM 被控端"；F18；阶段表 Phase 0 | 写了具体方案，并要求在 Phase 0 证实。剩余问题见新 S2-6 |
| S2-1 版本断言错位 | **部分关闭** | 文首"固定到 tag v0.7.0"；事实表里标注了哪些属于 `main` | 已改为固定到 v0.7.0，`main` 上的 API 也标出来了。但：① "commit `546a07d`"可能是附注 tag 对象的哈希，不是 commit；② v0.7.0 里有 `AddAllowedClient`，规格漏了；③ 好几条"已核实"其实只对照了 `main`。见 S4-1、新 S2-5 |
| S2-2 Ping / DiscoPing | **已关闭** | 事实表"是否真正直连"；§2 | 两端分别用什么 API 已写清楚 |
| S2-3 没有单一决策者 | **部分关闭** | §1 第 7 步；§2"回退"；I13 | Commit 由被控端裁决、`DirectFallback` 强制对端回退，都已写入。但没写 Commit 丢失时怎么办；看门狗的设计也有缺陷。见新 S1-1、S2-1 |
| S2-4 中转信任表述过度 | **已关闭** | §5"信任假设" | — |
| S2-5 IP 暴露 / Offer 时机 | **已关闭（前提需更正）** | I12；§5"IP 暴露" | 桌面被控端没有同意流程，I12 在 Phase 1 是空操作，见新 S2-4 |
| S2-6 v10 部署影响 | **已关闭** | §3 第 1 条；"发布与回滚"；I1；非目标；F20 | — |
| S2-7 postcard 前向兼容 | **已关闭（缺测试）** | §3 末段；I8 | 验收清单里没有 I8 对应的用例，见 S3-6 |
| S2-8 默认放行 / AllowClient 时机 | **已关闭** | §1 第 3、4 步（公钥先行）；§5"最小暴露面" | 新问题：允许列表为空时可能"放行所有人"，见新 S2-5 |
| S2-9 打包与平台 | **部分关闭** | F12 子项；风险"打包与平台"；手动验收 | 已列为子项，但杀毒误报、Linux 分发两项没有可检查的验收标准 |
| S3-1 helper_path | **已关闭** | §4 末段；`direct_config_has_no_helper_path_option` | — |
| S3-2 回环暴露面 | **已关闭** | 方案 B 的 (i)/(ii)；I9 | 新的细节见 S3-2、S3-3 |
| S3-3 libtailcat | **已关闭** | 方案 C′ | — |
| S3-4 并发 | **部分关闭** | §2"并发"；F19 | "每会话一个实例"和"每个 epoch 新建实例"指的是进程还是 Server 对象，没写清，见 S3-1 |
| S3-5 媒体头没有 epoch | **已关闭** | §1 第 8 步"画面 / 声音" | — |
| S3-6 会话生命周期耦合 | **部分关闭** | §2"直连通道失败只触发回退" | 原则已写，但和现有的 2 s 队列过期、写入失败即结束会话的代码路径怎么接，没写，见 S3-4 |
| S3-7 每个 epoch 新建 Server | **已关闭** | §2"每个重新升级尝试……" | — |
| S3-8 验收名不符实 | **基本关闭** | 验收前言；I4、I11、I10 各项 | 已经迁到 `client/src-tauri`，改成了行为测试，补了版本号断言。出现新的名不符实项，见 S2-6、S2-4 |
| S4 背景措辞 | **部分关闭** | 第 18–19 行 | 第 18 行仍写"DERP 仅用于引导"，紧接着第 19 行又说它也是兜底通道，前后矛盾 |
| S4 CLI UDP / I11 措辞 / quinn 配置 / rcgen / `Secret<T>` / 超时 | **已关闭** | 事实表 UDP 行；I11；§3；§5；§1 第 5 步 | — |
| S4 mTLS | **未采纳，也没说理由** | — | 可以接受，但应在开放问题里写一句理由 |

---

## 新 findings

### S1（编码前必须修订）

#### S1-1 媒体看门狗和"画面静止不发帧"冲突，直连会反复误回退
- **位置**：§2 第 3 条（"直连上 N ms（默认 1500）没有画面包且中转上仍有会话活动，则经中转发 `DirectFallback`"）
- **问题**：
  - airhand 被控端**只在画面变化时编码，静止画面完全不发帧**。普通桌面在大多数时间是静止的，所以升级后约 1.5 秒就会触发回退。
  - 回退后每 30 秒重试一次，共 3 次，用完就永久留在中转。结果是这个特性在普通使用中基本不起作用。
  - 另外，升级完成后中转流是空闲的热备，不承载任何东西，"中转上仍有会话活动"这个条件无从判断。
- **依据**：
  - `client/src-tauri/src/host/encoder/mod.rs:8`（"被控端只在画面变化时才编码，静止画面完全不占带宽"）
  - `client/src-tauri/src/host/capture/x11.rs:5`、`capture/sck.rs:3`（静止桌面不产生帧）
  - `client/src-tauri/src/e2e.rs:256`（"画面静止时不会有帧"）
  - 音频同理：`host/audio/mod.rs:5`（没有声音时不发包）
- **建议**：
  - 删掉"无画面"看门狗，改用**直连控制流上的 Ping / Pong**判断存活。控制端本来就每秒发一次 Ping（`controller/session.rs:302-328`），被控端回 Pong（`host/session.rs:170`）。
  - 控制端：直连上 K 秒（例如 2 s）收不到 Pong，就经中转发 `DirectFallback`。这一条同时覆盖两个方向的故障，非对称故障也能检测到。
  - 被控端：直连上 K 秒收不到 Ping，就回退。
  - 补测试 `static_screen_does_not_trigger_fallback`。
  - 同时修正 §2 的"收不到对端 Ping / Ack"：控制端收的是 Pong，不是 Ping；Ack 在静止时也不会有，不能当存活信号（见 S3-5）。

#### S1-2 控制流回退方向没有顺序规则，和 500 ms 栅栏超时互相矛盾，I6 不成立
- **位置**：§1 第 8 步"控制流"；§2"回退"（"因为接收端一直两路都读，所以不需要握手即可恢复"）；I6
- **问题**：
  1. **回退方向**：按第 8 步，栅栏必须是发送端在"旧通道"上发出的最后一条消息。回退时旧通道是直连，而直连通常已经坏了，栅栏送不到。接收端如果按栅栏规则执行，会缓存中转上的消息，500 ms 后超时，而"超时视为切换失败并回退"本身就处在回退之中，规则循环了。
  2. **如果回退不用栅栏**（按 §2 的写法）：在非对称故障或看门狗误判时，直连并没有完全断，只是变慢，直连上迟到的 `Input{KeyDown}` 可能在中转上的 `ReleaseAll` **之后**才到，按键因此卡住。这正是 `ReleaseAll` 要防的情况，I6 的"不乱序"不成立。
  3. **栅栏超时后缓存怎么处理**没写：丢掉会丢输入，照常处理会乱序。
  4. **`DirectCommit` 和栅栏谁先谁后**没写：Commit 在直连控制流上发，而控制端在读到中转栅栏之前会缓存直连上的消息。必须写明：被控端先在中转上发 `DirectSwitch`，再把 `DirectCommit` 作为直连控制流的第一条消息。
- **依据**：r2 §1 第 8 步、§2；`InputEvent::ReleaseAll`（`session.rs:337`）由控制端发给被控端；被控端本地能直接调用 `injector.release_all()`（`host/session.rs:163`）。
- **建议**（二选一，写进规格）：
  - (a) **反向栅栏**：回退方在中转上发出的**第一条**消息必须是 `DirectFallback{epoch}`。接收端无论从哪个方向收到它，都立即停止处理该 epoch 的直连控制流，后面再到的直连控制消息一律丢弃；然后被控端在本地执行 `release_all()`，再处理中转上之后的消息。另外写明：升级栅栏超时时，**丢弃**缓存的直连消息，并执行同样的回退流程（丢失由 `ReleaseAll` 和状态重同步补偿）。
  - (b) v10 本来就要改协议，可以趁机给控制消息加信封 `{lane_seq: u32, msg}`（仅在直连会话中使用）。接收端按序号合并两路，不再需要栅栏和超时；序号有缺口时触发 `ReleaseAll` 和状态重同步。
  - 补测试：`late_direct_input_after_fallback_is_discarded`、`fence_timeout_drops_buffer_and_releases_keys`、`commit_is_first_message_after_host_fence`。

### S2（重要）

#### S2-1 `DirectCommit` 丢失时，控制端没有超时
- **位置**：§1 第 7 步；I13
- **问题**：被控端发出 Commit 就把画面切到直连；如果直连随后坏掉，控制端永远收不到 Commit。规格只给被控端定了"没收到三条流就放弃"的超时，**控制端打开三条流后没有等待 Commit 的超时**。在 S1-1 修复之前，看门狗也没说升级前是否生效，结果是画面会在一段不确定的时间里黑屏。
- **建议**：控制端开完三条流后，T 秒（例如 2 s）内没收到 Commit，就经中转发 `DirectFallback{epoch}`。被控端对已经提交的 epoch 也必须接受这条消息。补测试 `commit_lost_controller_falls_back_within_bound`。

#### S2-2 `Bye` 走哪条通道没定；"回退后重发 `Bye`"实现不了
- **位置**：§2"回退后的状态重同步"（"`Bye` 等关键消息在回退后经中转重发一次"）；I10
- **问题**：被控端发出 `Bye` 后立刻 `break` 结束会话，之后不存在"回退"这一步，没机会重发。如果 `Bye` 走了直连而直连恰好坏了，控制端只看到中转断开，会按普通断线处理并**自动重连**，这违背 `Bye` 的语义（不应自动重连）。
- **依据**：`host/session.rs:193-198`；`airhand-link/src/problem.rs:100`；`session.rs:47`（"控制端不应自动重连"）。
- **建议**：规定 `Bye` 这类终止消息**一律经中转控制流发送**（中转是会话生命周期的权威，与 I10 一致），可以同时在直连上也发一份，然后再结束会话。删掉"回退后重发"这句。补测试 `bye_always_sent_on_relay`。

#### S2-3 ≤250 ms 路径检测的判定语义没定，验收测的也不是 helper 本身
- **位置**：§2 第 1 条；I3；验收 `media_stops_within_bound_when_path_degrades`
- **问题**：
  - **`DiscoPing` 超时的含义没写**：路径不通时，magicsock 不会回调，只能等 ctx 到期（tailcat `tailcat.go:2298-2301`）。按 250 ms 的周期，ctx 也只能 ≤250 ms；直连 RTT 超过 250 ms 的跨洲路径会被**误判为非 direct**，开关关闭，画面断，然后回退。
  - **控制端没有 `Status()`**：`Client` 没有 `Status` 方法（`tailcat.go` 中 `Client` 的导出方法只有 PublicKey / DERPRegion / Close / Ping / DiscoPing / Dial*），所以控制端只能靠 `DiscoPing`。
  - **两端可能不一致**：升级初期，控制端可能已判定 direct，而被控端 `Status` 的 `CurAddr` 还是空的，被控端开关仍关着，直连 QUIC 握手会卡住。
  - **验收只测了 Rust 侧**：I3 承诺的是 Go helper 的行为，但验收项用假 helper 测 Rust 侧，Go 侧开关逻辑没有任何测试，名不符实。
  - **按 tailscale 已知行为推断（本地没有 tailscale.com 源码，未证实）**：magicsock 在直连路径的信任期内（`trustUDPAddrDuration` 约 6.5 s）不会经 DERP 发数据，信任过期后才会同时经 DERP 双发。如果属实，开关的超时阈值可以放宽到几百毫秒甚至更长，而不会让媒体走 DERP。
- **建议**：
  - 写明判定规则，例如：`DiscoPing` 的 pong 经 DERP 返回，立即判非 direct；连续 M 次（或 `max(750ms, 3×RTT)`）没收到 direct pong，判非 direct；被控端以 `CurAddr` 是否为空为准。
  - I3 的界限改成与上述规则一致的表达。
  - 补 Go 测试 `TestGateDropsWhenPathNotDirect`，用可注入的路径源。
  - 把"magicsock 何时开始双发 DERP"列为 Phase 0 实测项。

#### S2-4 桌面被控端没有"同意"流程；I12 和 §5、I10 的相关表述在 Phase 1 名不符实
- **位置**：I12；§5"同意与可见性不变"；验收 `no_offer_before_approval`、`direct_session_requires_same_approval_and_shows_controller`；feature map F11
- **问题**：桌面被控端**从不**请求本机用户同意，持码人连接后马上就收到 `Hello`。所以：
  - I12 在 Phase 1（只做桌面）里是空操作；
  - "直连会话与中转会话走完全相同的请求同意路径"这个说法对桌面不适用；
  - 任何持码人都能**立即**拿到被控端的公网和内网 IP。这和"持码即可完全控制"的现有信任模型一致，但应该如实写出来。

  这也更正我第 1 轮 S2-5 的部分前提。
- **依据**：`client/src-tauri/src/host/session.rs:172`（"桌面被控端不请本机用户同意"）；`client/src-tauri/src/e2e.rs:336`（"桌面被控端不会请本机用户同意"）。
- **建议**：
  - I12 改为"`DirectOffer` 只在 `HostHello` 发出之后发送；在有同意流程的平台（Android，Phase 1.5）上还须在同意之后"。
  - 测试改名，例如 `no_offer_before_host_hello`；同意相关的测试留到 Phase 1.5。
  - 在隐私说明里写明"持码人即可获知本机 IP"。

#### S2-5 允许列表为空时可能放行所有人；v0.7.0 的 API 描述不完整
- **位置**：事实表"安全属性"；§1 第 3、4 步；§5"双重限定"
- **问题**：
  - **可能 fail-open**：`main` 上 `AllowClient` 为 nil 时"all clients are allowed"（`tailcat.go:455-456`）。v0.7.0 的 `AllowedClients` 为空列表时是否同样放行所有人，**未证实**（CLI 不带 `--allow` 时是开放的，可以佐证）。如果 `client_key` 解析失败或为空，helper 就会起一个**谁都能连**的 Server（只剩令牌这一层）。
  - **漏了 `AddAllowedClient`**：CHANGELOG 写明 Unreleased 版本替换掉了"`Server.AllowedClients` and `Server.AddAllowedClient`"（`CHANGELOG.md:5-6`），说明 v0.7.0 里**有** `AddAllowedClient`。规格"必须在 Start 前给定"的说法不完整。公钥先行仍然是更好的设计（没有时间窗口），但事实表应该写准确。
- **建议**：
  - helper 收到的 `client_key` 无效或为空时**拒绝 Start**，Rust 侧同时做校验。
  - 补 Go 测试 `TestEmptyOrInvalidAllowlistRefusesToStart`。
  - 用 `git show v0.7.0:tailcat.go` 核对 `AllowedClients` 为空时的语义，以及 `AddAllowedClient` 的签名。

#### S2-6 Windows 受限令牌方案：没有硬伤，但验收名不符实，缺退路
- **位置**：§5"Windows SYSTEM 被控端"；验收 `helper_token_is_not_system_on_windows`；F18
- **问题**：
  - **可行性**：从 SYSTEM 令牌出发，用 `CreateRestrictedToken` 把用户 SID 和 Administrators 设为 deny-only，再设低完整性级别、套 Job 对象。整体可行；用调用者主令牌的受限版本调用 `CreateProcessAsUserW`，不需要 `SeAssignPrimaryToken` 特权。这些依据来自 Windows 公开文档，本地未验证。
  - **名不符实**：用户 SID 设为 deny-only 后，令牌的用户**仍然是 S-1-5-18（SYSTEM）**，只是带上了 deny-only 属性。断言"令牌非 SYSTEM"在字面上会失败；要么测试被写成断言别的东西，要么方案被误改。
  - **Phase 0 清单缺两个常见坑**：
    - 令牌的默认 DACL 通常只授予 SYSTEM 和 Administrators；这两个 SID 变成 deny-only 后，进程可能打不开自己新建的对象（Chromium 沙箱需要专门处理这一点），Go 运行时可能出问题；
    - 只用 deny-only、不加 restricting SIDs 时，仍保留 Everyone / Authenticated Users 的读权限。
  - **缺退路**：Phase 0 证实不可行时怎么办，规格没写。
  - **备选未评估**：AppContainer（网络能力需要显式授予，默认禁止回环，正好配合候选 (ii)）没有列入。
- **建议**：
  - 验收改名为 `helper_token_is_restricted_on_windows`，断言：用户 SID 与 Administrators 为 deny-only；特权只剩 `SeChangeNotifyPrivilege`；完整性级别为 Low；位于 Job 内且受限；（如采用）restricting SIDs 存在。
  - Phase 0 清单补上默认 DACL、restricting SIDs 对 Winsock 的影响，以及 AppContainer 对比。
  - 写明退路："不可行时，Windows SYSTEM agent 作为被控端时禁用直连；控制端不受影响。"

#### S2-7 两段批准基本符合 workflow，但缺三项保障
- **位置**：阶段表"项目负责人批准""Phase 0 spike"；开放问题 4
- **评估**：workflow 规定重大改动必须等待项目负责人批准，spec 不替代批准。先批准 spike（可丢弃的独立分支）、再批准 Phase 1，**符合**这条规定：两次都是显式批准，spec 也不代替批准。但还缺：
  1. **go/no-go 没有量化阈值**，"不达标"是什么没定义，第二段批准就缺少客观依据。阈值应在第一段批准**之前**写进规格，例如：1080p60 下额外 CPU ≤ X%，额外延迟 ≤ Y ms，吞吐 ≥ Z Mbps，Windows 受限令牌下能绑定 UDP、能枚举网卡。
  2. spike 结果"追加到本规格"会改变设计（选 (i) 还是 (ii)、各项超时、Windows 方案），但阶段表里**没有 spike 之后的设计复审**。
  3. 没写明 spike 代码**不合并、不直接复用**进 Phase 1。复用没有测试的 spike 代码，与"先红后绿"冲突。
- **建议**：阶段表补上"Phase 0 结果 → 规格 r3 → 设计复审 → 批准 (B)"；第一段批准范围写明"仅限 spike 分支，不得合并"；"降级为仅低码率档"属于范围变更，须在第二段批准里单独确认。

### S3（一般）

| 编号 | 位置 | 问题 | 依据 | 建议 |
|---|---|---|---|---|
| S3-1 | §1 第 4 步 vs §2"并发" | "每个 epoch 新建一个 helper serve 实例"和"每会话一个 helper serve 实例"，指的是进程还是 `tailcat.Server` 对象，前后不一致。同一进程里跑多个 Server 是否安全**未证实**：例如 `netns.SetEnabled(false)` 是进程级全局设置（`tailcat.go:1908`） | 同左 | 建议"每会话一个进程，每个 epoch 在进程内新建 Server，并 `Close` 旧的"，或者干脆每个 epoch 一个进程；在 Phase 0 验证。控制端同样写明"每个 epoch 新的 `Client` 和密钥" |
| S3-2 | 方案 B (ii) | stdin/stdout 已经用来传 JSON 行控制协议，数据报"经 helper 的 stdio"就会和控制协议冲突 | §方案 B 首段与 (ii) | 数据报改用单独的管道（Unix 用 fd 3；Windows 用 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 继承额外句柄），或者在 stdio 上定义统一的二进制分帧，并写进 F6 |
| S3-3 | 方案 B (i) | Windows 上如果不用 `SO_EXCLUSIVEADDRUSE`，本机其他进程可以用 `SO_REUSEADDR` 抢绑同一个回环端口，"4 元组锁定"因此不成立 | Windows Winsock 公开行为（未本地验证） | 选 (i) 时，两端在 Windows 上都用 `SO_EXCLUSIVEADDRUSE` 绑定，并补测试 |
| S3-4 | §2"直连失败只回退" | 控制端命令队列超过 2 s 未发出就让会话失败；被控端 `io.writer.failed()` 也会直接结束会话。直连写入卡住时，可能在 3 s 检测之前就结束会话，违背 I2 | `controller/queue.rs:21, 158`；`host/session.rs:199`；`transport.rs:24` | 规定直连检测阈值 < 2 s；写入失败要区分通道：直连写入失败转为回退，并把排队的消息改投中转 |
| S3-5 | §2 第 3 条 | "超过 3 秒收不到对端 Ping / Ack"：控制端收的是 `Pong` 而不是 Ping；静止画面时没有 Ack | `session.rs:32, 216, 224`；`encoder/mod.rs:8` | 按端分别写清存活信号（并入 S1-1 的修订） |
| S3-6 | 验收清单 | I8 没有用例；新增设计也缺用例：Commit 丢失、栅栏超时、回退后迟到的直连输入、静止画面、`Bye` 走中转、允许列表为空、Go 侧丢包开关、多个 Server 并存 | — | 补 `unknown_offer_kind_is_declined`，以及 S1-1、S1-2、S2-1、S2-2、S2-3、S2-5 中列出的测试 |
| S3-7 | I7 | tailcat / magicsock 的内部日志（`Logf`）会打印节点公钥简写和对端 IP；helper 的 stderr 如果被 Rust 收进日志（Windows 写到 `OutputDebugString`），IP 会落进本机其他用户可读的位置 | `tailcat.go:1994-1996`（Logf 默认是 `log.Printf`） | helper 默认把 tailcat 的 `Logf` 设为丢弃，诊断模式下脱敏；Go 测试覆盖 |
| S3-8 | §1 第 8 步，500 ms | 中转路径多一跳；上行慢的时候，中转上的栅栏可能超过 500 ms 才到，频繁触发切换失败；缓存期间 Ack 被扣住，被控端的在途窗口也会停住 | `transport.rs:50`（已有流优先级，可以缓解） | 超时设为可配置或按 RTT 自适应，由 Phase 0 实测确定；写明缓存期间 Ack / Ping 照常处理（它们和顺序无关） |
| S3-9 | §2"回退后的状态重同步" | "双方各自执行 `ReleaseAll`"语义不清：`ReleaseAll` 是控制端发给被控端的输入事件 | `session.rs:337`；`host/session.rs:163` | 写成：被控端在本地调用 `release_all()`；控制端清空本地记录的按键状态，不需要再发一次 |

### S4（建议）

- **S4-1 tag 哈希**：`546a07d` 来自 `refs/tags/v0.7.0`，`FETCH_HEAD` 里写的是"tag 'v0.7.0'"。如果是附注 tag，这是 tag 对象的哈希，不是 commit。建议记录 `v0.7.0^{commit}` 的哈希和 `go.sum`。
- **S4-2 "已核实"的范围**：事实表里有些条目是对照 `main` 得出的，v0.7.0 上**未证实**，至少包括：Go 1.27.x、`MaxUDPPayload = 1232`、`Ping` 内部 10 s 超时、`ServedTCPPorts` / `ServedUDPPorts` 的语义。建议逐条用 `git show v0.7.0:…` 核对，或者标为"未证实（v0.7.0）"。已能确认属于 v0.7.0 的：`Server.Status` 带 Peer 的 `CurAddr` / `Relay`（`CHANGELOG.md:62-64`）；`Server.Listen` 支持 UDP（`CHANGELOG.md:67-72`），可以作为 `OnUDP` 的替代写进规格。
- **S4-3 背景措辞**：背景第 18 行的"DERP 仅用于引导"和第 19 行矛盾，删掉"仅用于引导"。
- **S4-4 失效链接**：文首链接的 `tailcat-p2p-review.md` 在 `docs/specs/` 下不存在。
- **S4-5 feature map 不一致**：F7 写"被控端直连 QUIC 监听（仅回环）"，和候选 (ii)"不开任何本机端口"不一致，改成"视候选而定"。
- **S4-6 哈希校验的实际价值**：在用户可写的安装位置（AppImage、`~/Applications`），helper 哈希校验只能防误放文件，防不了同用户的攻击者（对方本来就能替换主程序）。如实写明它的作用范围即可。
- **S4-7 mTLS**：mTLS 未采纳，建议在开放问题里补一句理由。

---

## 可以进入编码阶段的前置条件清单

**第一段：批准 Phase 0 spike 之前**

1. 修订 S1-1：存活检测改用直连上的 Ping / Pong，不再依赖画面包。
2. 修订 S1-2：定义回退方向的顺序规则（反向栅栏或 `lane_seq`）、栅栏超时后的处理，以及 Commit 和栅栏的先后。
3. 写明 S2-7 要求的量化 go/no-go 阈值，以及"spike 不合并"的约束。
4. 把 S2-3、S2-5、S2-6 中需要实测的项补进 Phase 0 清单：
   - `DiscoPing` 超时、RTT 对判定的影响；
   - magicsock 何时开始经 DERP 双发；
   - 两端判定为 direct 的时间差；
   - v0.7.0 下 `AllowedClients` 为空时的语义；
   - 默认 DACL、restricting SIDs、AppContainer 对比；
   - 同一进程内多个 Server 是否安全；
   - 候选 (ii) 的独立管道方案。
5. 项目负责人在 PR 中**显式批准 (A)**，范围仅限 spike 分支。

**第二段：批准 Phase 1 编码之前**

6. 把 Phase 0 结果写成规格 r3：选定 (i) 或 (ii)、确定各项超时、确定 Windows 方案或"Windows SYSTEM 被控端禁用直连"的退路，然后**再做一轮设计复审**，直到没有 S1。
7. 关闭 S2-1 到 S2-6：控制端的 Commit 超时；`Bye` 一律走中转；路径判定规则；I12 改写；允许列表为空时拒绝启动；Windows 验收改名并补退路。
8. 验收清单补齐 S3-6 列出的用例，并修正名不符实的项（S2-3、S2-4、S2-6）。
9. 用 `git show v0.7.0:…` 核对 S4-1、S4-2 列出的 v0.7.0 事实，或者明确标为"未证实"。
10. 项目负责人在 PR 中**显式批准 (B)**。之后按"先红后绿"开新分支实现。