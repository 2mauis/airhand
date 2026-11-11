# 第 4 轮设计复审（对象：规格 r4）

> 评审者：Claude Code（由项目负责人指定；只读，工具限定为 Read / Grep / Glob，无写入、无命令、无联网）。
> 输入：规格 r4 + feature map + 上游源码（`a317551`）+ tailcat 源码检出 + 第 3 轮报告；未提供任何凭据或内网信息。
> 以下为评审报告原文；报告末尾的“作者处理”由规格作者追加。

# tailcat 临时点对点直连：规格 r4 复审（第 4 轮）

## 结论

**有条件通过。r4 可以作为 Phase 0 spike 的依据。**

- **本轮没有 S1。** Phase 0 测的是传输层：helper 吞吐 / CPU、路径判定、I3 窗口、各项超时校准、Windows 受限令牌。新增的"被控端确认与断开"属于会话层，不改变这些测量的对象和方法。
- **新章节的方向是对的：** 确认放在中转控制流上，排在地址交换之前，并沿用 `AwaitingApproval` / `Declined` / `Unanswered`。核对源码时发现以下问题，都要在批准 (B) 之前改：
  1. **几处事实断言与源码不符**：`Peer` 没有 OS 字段；终止原因在代码里叫 `is_final` 而不是"致命"；"停止共享"写成发 `Kicked`，与 `NewCode` 的语义不一致；"计入服务器猜码限速"在不改服务器的前提下做不到。
  2. **I15 与现有确认机制矛盾**：I15 把 `ControllerMsg::Hello` 和 `Settings` 也丢掉，同设备复用授权和直连能力位都会失效。
  3. **三个安全缺口**：
     - 已获输入权限的远端可以用注入的键鼠点自己的确认窗 / 指示条，自行勾上"允许直连"，项目负责人"暴露地址之前必须确认"的要求因此落空；
     - 被"断开此控制端"的设备可以免确认再连回来；
     - Windows 安装版的确认窗放在哪个进程、经哪条 IPC 授权，规格没定。
- 第 3 轮的 S2-1、S2-2 已关闭，S2-3 只关闭了一部分（码率上限策略写着"在 r4 定"，r4 没定）。

---

## 一、新章节事实断言核对（对照上游 `a317551` 源码）

| 断言（规格位置） | 结果 | 依据 |
|---|---|---|
| 已有 `HostMsg::AwaitingApproval`、`ByeReason::Declined` / `Unanswered`；等待期间照常回 Pong | **属实** | `airhand-proto/src/session.rs:10-13, 30, 84-93`；Android `host/session.rs:184` |
| Android 被控端超时 60 秒（`APPROVAL_TIMEOUT`） | **属实** | `android/native/src/host/session.rs:32`。另有 `HELLO_WAIT` = 2 s（:30）：控制端没报上 `Hello` 时按"没见过的设备"处理并询问（:222-224），规格没提 |
| `Problem::Declined` / `Unanswered` 是"致命"的（控制端不自动重连） | **行为属实，术语不对** | 代码里判断的是 `Problem::is_final()`，它覆盖 `Kicked` / `CodeChanged` / `Declined` / `Unanswered`（`problem.rs:101-106`）。`is_fatal()` 是另一个函数，只对 `Incompatible` 为真（:96-98）。规格第 164、177 行和 I15 写"致命"，实现者可能误改 `is_fatal`。另外"不自动重连"只是客户端自觉遵守，恶意客户端可以无视 |
| 同一次运行里同一设备断线重连"沿用先前授予的权限、不再询问（沿用现有语义）"（第 178 行） | **部分属实** | 现有语义只在 Android 上有。`trusted` 是 `HashSet<[u8;16]>`，只记设备，**不记权限**（`android/native/src/host/mod.rs:126-127`）。任何 `HostCmd`（KickAll / NewCode）都会清空整个集合（:376-379）。桌面端没有这套机制，`ControllerMsg::Hello(_) => {}` 直接忽略（`client/src-tauri/src/host/session.rs:172-173`）。所以"授权记忆 + 权限集"在桌面上是新设计，不是沿用 |
| 确认窗显示"现有 `Peer`：自报的设备名与 OS、来源 IP"（第 176 行） | **不符** | `Peer` 只有 `name` 和 `ip`（`airhand-proto/src/signal.rs:104-108`）。`host_os` 在 `SessionGrant` 里，方向是发给控制端的 |
| `HostCmd::KickAll` / `NewCode` | **属实**：KickAll 让所有会话发 `Bye(Kicked)`，控制码不变；NewCode 发 `Bye(CodeChanged)`，最多等 800 ms，再 `Release`（最多 2 s）并重新注册（`airhand-link/src/host.rs:38-53, 184-207`） | 规格说"停止共享"等于"`NewCode` 语义 + 发 `Bye(Kicked)`"（第 186 行、I18），**和 NewCode 实际发的 `CodeChanged` 矛盾**。现有代码里**没有**按会话踢人的命令：kick 是对所有会话广播的 `watch`（host.rs:160, 187），"断开此控制端"属于新增 |
| controller `Command::Disconnect` | **属实** | `controller/mod.rs:371-379`：先发 `Input(ReleaseAll)`，等 150 ms 再结束任务，**不发任何 Bye**，被控端看到流结束才知道。`ControllerMsg::Bye` 现在确实不存在，规格写"新增"是对的 |
| 桌面被控端当前不请求同意；`e2e.rs` 断言不会出现 `AwaitingApproval` | **属实** | `client/src-tauri/src/host/session.rs:80-91`（会话一开始就发 `Hello`）；`e2e.rs:336`（`panic!("桌面被控端不会请本机用户同意")`） |
| 被拒 / 无应答超限后"计入服务器现有的猜码限速"（第 203 行） | **不可实现** | 服务器只在控制码不存在（`Denied::Unknown`）时按 IP 记失败（`server/src/broker.rs:557-562`）。带重连凭证的请求**不限速**（:587-596）。被控端没有任何消息能把"被拒绝"报告给服务器，而非目标里写了不改 `server/` 源码 |
| `max_pending_approvals` 不超过 `AIRHAND_MAX_SESSIONS`（第 199 行） | **无法在客户端执行** | `AIRHAND_MAX_SESSIONS` 是服务器的环境变量（`server/src/main.rs:9, 97`），被控端拿不到它的值 |
| RustDesk：`LOGIN_GRACE` = 180 s、未授权连接上限 64 / 单地址 16、`MAX_UNAUTHORIZED_MESSAGE` = 128 KiB；`password-click` 是"双因子" | **本轮未核实**（按要求不联网） | 建议标注 RustDesk 的 commit 和行号。"双因子"的说法存疑：据我所知，RustDesk 的 `password-click`（"两者皆可"）是**密码或点击任选其一**，而不是两者都要。若属实，"未采纳项"第 2 条的理由要改写 |
| CRD"续期提示默认按钮为停止共享"、AnyDesk 接受窗口的行为 | **未核实** | 不影响设计，标为"参考，未核实"即可 |

---

## 二、第 3 轮 finding 关闭情况

| 编号 | 状态 | 说明 |
|---|---|---|
| S2-1 回退发起方 / 栅栏后的中转消息 / 改投 | **已关闭** | §2 第 87-91 行：发起方也丢弃直连消息、列出栅栏后仍须处理的中转消息、改投只限未写出且未过期的消息；补了 4 个用例，内容自洽 |
| S2-2 K 与"<2 s"约束矛盾 | **已关闭** | 去掉了"<2 s"约束；直连 Ping 间隔 250 ms；K = `max(1.5 s, N×间隔 + 2×RTT)`；写超时按通道区分；10 s 措辞已改准。遗留一处小问题：被控端算 K 要用 RTT，从哪里取没写（见 S3-8） |
| S2-3 go/no-go 阈值 | **部分关闭** | 已补"误回退"一行、I3 窗口一行（闸门 ≤ 2 s，DERP 数据报数量待定）、RTT ≤ 400 ms、升级耗时起点、QUIC 层统计。还差：① **码率上限策略**表里写"在 r4 定"，r4 没定（第 316 行）；② Go 用例仍写"RTT > 250 ms"，没对齐到 ≤ 400 ms（第 268 行）；③ CPU 一行写"参考硬件同上"，但参考硬件定义在它**下面**的升级耗时一行，而且只写了"近三年主流"；④ 表头仍是"供批准 (A) 前确认"，(A) 已经批了 |
| S3-1 `Status()` 轮询计入 I3 窗口 | **已关闭** | 第 81 行；`TestHostGateWindowIncludesStatusPollInterval` |
| S3-2 Commit 计时 | **已关闭** | 第 68 行；`commit_after_own_fallback_is_ignored` |
| S3-3 状态重同步 | **基本关闭** | 规格已写由 `airhand-link` 缓存。还差：feature map 的 F9 没更新；缓存的 `SelectDisplay` 可能已经过期（见 S3-6） |
| S3-4 Windows 验收 | **已关闭** | 改为手动、真机、以 SYSTEM 服务身份运行；补了功能验证和反向验证 |
| S3-5 feature map 残留"同意"字样 | **已关闭** | F4 和时序图已改 |
| S3-6 `SO_EXCLUSIVEADDRUSE` 用例 | **已关闭** | — |
| S3-7 用例归属 | **已关闭** | — |
| S4-1 失效链接 | **已关闭** | `tailcat-p2p-review.md` 已存在。但第 3 行写"四轮报告全文"，第 288 行写"三轮"，`review/` 下目前只有三轮 |
| S4-2 DERP 重复表述 | **未关闭** | 第 18、19 行仍然重复；"已决定"第 2 条只写"做引导"，没提窗口期内的有界承载 |
| S4-3 / S4-4 / S4-5 | **已关闭** | — |

---

## 三、新 findings

### S1（进入 Phase 0 与后续批准前必须修订）

**无。** 理由见"结论"。

### S2（批准 (B) 之前必须关闭）

#### S2-1 I15 "只处理 Ping / Bye" 会丢掉 `ControllerMsg::Hello` 和 `Settings`，与"沿用现有机制"和同设备复用授权互相矛盾

- **位置**：I15（第 239 行）；"滥用与资源"第 204 行；用例 `non_ping_messages_dropped_while_awaiting`
- **依据**：
  - 控制端打开会话后**立刻**连发 `Hello` 和 `Settings`（`controller/mod.rs:270-278`），这时被控端还在 `AwaitingApproval` 阶段。
  - Android 的做法是：授权前**记下 `Hello` 里的设备标识、`Settings` 和 `Audio`**，授权后再生效（`android/native/src/host/session.rs:12, 173-186, 215`）。
- **后果**：
  1. 按 I15 丢掉 `Hello`，就拿不到 `device`，"同设备重连沿用授权"做不了；
  2. 规格第 2 步说能力位 `direct` 放在 `ControllerHello` 里，丢掉 `Hello` 也就拿不到能力位，确认窗不知道该不该显示"允许直连"；
  3. 丢掉 `Settings`，授权后被控端按默认画质开播，控制端不会重发，属于回归。
- **建议**：
  - I15 改为"只**处理** `Hello` / `Ping` / `Bye`；`Settings` / `Audio` / `SelectDisplay` **只记下最新值**、授权后生效；其余（输入、剪贴板、`Ack`、`Direct*`）丢弃"。
  - 用例拆成 `pre_approval_settings_applied_after_approval` 和 `pre_approval_input_clipboard_direct_dropped`。

#### S2-2 远端可用注入的键鼠操作被控端自己的确认窗 / 指示条，绕过"必须由被控端用户确认"

- **位置**：第 180 行（权限随时可改）、第 185-189 行（常驻指示、续期提示）、I20、I22
- **问题**：
  - 控制端拿到"输入控制"（默认开）以后，可以：
    - 点开指示条，勾上"允许直连"（以及剪贴板、声音）；
    - 在续期提示上点"继续"；
    - 在另一台控制端排队的确认窗上点"允许"。
  - I22 只要求指示"不能被远端画面关闭或遮盖"，没要求"不能被远端**输入**操作"。
  - 这正好落空了项目负责人"尤其在暴露公网 / 内网地址之前"必须确认的要求。
- **建议**（写进规格并补用例）：
  1. 指示条**只能收回、不能扩大**权限；要加权限，只能在**新会话的确认窗**里勾。
  2. 确认窗和续期提示过滤注入输入：
     - Windows：低层钩子 `LLMHF_INJECTED` / `LLKHF_INJECTED`，属于公开 API 的已知行为，需在 Phase 0 或实现阶段验证；
     - macOS / Linux：能否区分注入输入**未证实**。做不到的平台，在**有已授权、带输入权限的会话**期间，**不显示**"允许直连"勾选和"允许"按钮，新请求一律按拒绝处理，或者要求本机解锁凭据。
  3. 补用例 `indicator_cannot_grant_permissions`、`injected_input_cannot_approve_prompt`（Windows 手动）、`prompt_hides_direct_option_while_remote_input_active`。

#### S2-3 "授权记忆"的范围和清除规则没定：被断开的设备可以免确认回来

- **位置**：第 178、187 行；I18、I20；用例 `same_device_reconnect_reuses_grant_in_same_run`
- **问题**：
  1. 桌面被控端常驻运行，可能好几周都在"同一次运行"里。授权记忆什么时候清除，规格没写。Android 的做法是任何 KickAll / NewCode 都清空（`mod.rs:376-379`）。
  2. "断开此控制端（保留控制码）"之后，如果没有清掉这台设备的记忆，对方手动重连就能**免确认**连回来。`Kicked` 是 `is_final`，只阻止**自动**重连，挡不住手动重连。直连权限也会一起恢复。
  3. 权限收回以后再重连，沿用的是**收回后**的权限还是最初授予的权限，没写。
  4. 设备标识是控制端自报的、经中转明文转发。按信任假设，中转本来就能重放，这一点可以接受，但要写明。
- **建议**：
  - 写明"授权记忆按设备保存**当前**权限集；任何一种被控端主动断开（停止共享、断开此控制端、KickAll）、权限收回到全无、会话结束超过 N 分钟，都清除"。
  - **直连权限不随授权记忆复用**，每次都要重新确认（或者做成单独可配置项）。这与"暴露地址前确认"的要求最一致。
  - 补用例 `kicked_device_must_reapprove`、`reused_grant_reflects_revocations`、`direct_grant_not_reused_on_reconnect`。

#### S2-4 Windows 安装版的确认窗放在哪个进程、经哪条通道授权，规格没定

- **位置**：第 176 行、风险第 357 行、"手动"验收第 265 行
- **依据**：
  - Windows 安装版的被控端是在控制台会话里运行的 SYSTEM agent，界面在托盘进程里，两者经命名管道通信（`winsvc/mod.rs:13-17`）。
  - 管道允许同会话的任何交互用户进程（IU）连接（`ipc.rs:42-45, 176-179`）。`ToAgent` 现在只有 KickAll / NewCode / SetAutostart / SaveDevices（:67-77）。
  - "托盘和服务同进同退"（`winsvc/mod.rs:16`）。登录屏和锁屏时没有托盘，或者托盘不可见。
- **问题**：
  1. 确认窗如果放在托盘里，"允许"就要经管道传给 agent。同会话的任何进程都能伪造"允许"，相当于把授权能力交给了管道的 ACL。
  2. 确认窗如果放在 agent 里（SYSTEM 在 Winlogon / 安全桌面上弹窗），这是新能力。
  3. 风险里写"能否在登录屏显示**未证实**"。但从现有架构看，现在**根本没有**能在登录屏显示的界面路径，这一点可以直接下结论，不必列为未证实。
- **建议**：
  - 写明确认窗的所在进程、新增的 IPC 消息（例如 `Approve { session_id, granted }` / `Decline`），以及授权方的身份校验（至少核对客户端进程是本安装目录下的 `airhand.exe`，并如实说明同用户攻击者不在防护范围内）。
  - 登录屏 / 锁屏的行为写成确定结论："无托盘 ⇒ 超时拒绝"，或者"在 agent 里弹窗"，后者列为 Phase 0 可选验证项。
  - 补 `tray_ipc_approve_requires_*` 用例。

#### S2-5 滥用防护有两处做不到，一处会占满会话槽位

- **位置**：第 199、203 行；I17
- **问题**：
  1. "计入服务器猜码限速"和"不超过 `AIRHAND_MAX_SESSIONS`"在不改服务器的前提下都做不到（见第一节）。
  2. 被拒绝的控制端仍然持有重连凭证。凭证在换码之前一直有效，而带凭证的请求不限速（`broker.rs:587-596`），所以持码人可以不停地重连弹窗。
  3. "同一来源"按 IP 或自报设备来认：设备标识攻击者可以随意换，IPv6 地址也容易换，所以还需要全局上限。
  4. "静默拒绝"如果是不回应、等超时，会话在服务器上会一直占着槽位（默认每台被控端 4 个，`broker.rs:617`）。`max_pending_approvals` 的默认值 4 正好等于槽位数，待确认的会话可以把已授权用户的槽位全部挤掉。
  5. 只弹一个确认窗、其余排队时，排队中的请求 60 秒计时从什么时候开始，没写。
- **建议**：
  - 删掉"计入服务器限速"，改为"被控端本地按 IP 限速，另设全局速率上限"。
  - 静默拒绝改为**立即**发 `Bye(Declined)` 并关闭会话，不弹窗。
  - `max_pending_approvals` 默认值小于服务器槽位数（例如 2），并去掉对服务器环境变量的引用。
  - 写明排队中的请求计时从入队时开始。
  - 在 `docs/security.md` 说明"拒绝不会让控制码失效，要阻止持码人就停止共享 / 换码"。

### S3（一般）

| 编号 | 位置 | 问题 | 依据 | 建议 |
|---|---|---|---|---|
| S3-1 | 第 186 行、I18 | "停止共享"写成发 `Bye(Kicked)` 并"等于 `NewCode` 语义"，但 NewCode 发的是 `CodeChanged`；控制端显示的原因文案也不同（`problem.rs:134-135`） | `host.rs:49-50` | 统一为 `CodeChanged`。另外写明现有托盘的"断开所有控制端"（KickAll）是保留还是被"断开此控制端"取代，I10 用例 `kick_all_*` 要与之对应 |
| S3-2 | 第 187 行 | "断开此控制端"要新增按会话的命令。现在的 kick 是全体广播 | `host.rs:160, 187` | 新增 `HostCmd::Kick { session_id }`，在 Windows 上对应新增的 `ToAgent` 消息；写进 F23 |
| S3-3 | I18、I19；第 92 行 | 会话任务可能在任意 await 处被取消：NewCode 返回时会丢掉 `JoinSet`（`host.rs:206`）；Android 端特意把清理放在 `Drop` 里（`android/.../session.rs:58-72`）。"结束 helper 进程"必须写在取消安全的路径上 | 同左 | 写明：helper 用 `kill_on_drop`，并在 stdin 读到 EOF 时自行退出；Windows 加 Job 的 `KILL_ON_JOB_CLOSE`（已有），Linux 用 `PR_SET_PDEATHSIG`。补用例 `aborted_session_task_kills_helper` |
| S3-4 | 第 188 行；I19 | `ControllerMsg::Bye { reason }` 的 `reason` 类型没定。`ByeReason` 现有的值都是站在被控端角度的；`Command::Disconnect` 现在的顺序是"先 ReleaseAll，等 150 ms"，加入 Bye 和直连清理以后的顺序没写 | `controller/mod.rs:371-379` | 定为无字段的 `ControllerMsg::Bye`，或者单独定义原因类型；顺序写为：ReleaseAll（当前通道）→ 经中转发 `Bye` → 关闭直连 → 结束 helper。被控端收到 `Bye` 时本地调用 `release_all()`（桌面端已有，`client/src-tauri/src/host/session.rs:246`） |
| S3-5 | I20 | 只规定了收回输入和直连两种权限。剪贴板是**双向**的：被控端的轮询（:188-191）和 `clip.apply`（:171）现在都不受任何条件限制；声音权限收回后怎么取消订阅，也没写 | `host/session.rs:171, 174-191` | 写明每项权限在两个方向上的执行点。补 `revoking_clipboard_stops_both_directions`、`revoking_audio_unsubscribes` |
| S3-6 | §2 重同步（第 91 行） | 回退后重发缓存的 `SelectDisplay` 可能已经过期：被控端可能已经因为显示器变化自行切换（`follow_display`，:224-237），重发旧序号会把画面切回去。`Settings` 和 `Audio` 的重发是幂等的，没有这个问题（`index != current`、`sound.is_none()` 判断） | `host/session.rs:160-168, 224-237` | 控制端收到 `HostMsg::Displays { current }` 时同步更新缓存；或者不重发 `SelectDisplay`，以被控端为准 |
| S3-7 | 第 176 行 | 被控端或控制端没启用直连（默认就是这样）时，确认窗不该显示"允许直连" | 规格第 2 步 | 写明"仅当本端 `[direct] enabled` 且 `ControllerHello.direct` 时才显示"；补用例 |
| S3-8 | §2 存活检测 | 被控端算 K 要用 RTT，但被控端拿不到控制端 Ping 测出的 RTT（只有 `Flow` 的 srtt）；切换通道后 `Flow` 的 `min_rtt` 要过 10 s 才更新（`flow.rs:58-61`），ABR 会短暂误判拥塞 | `flow.rs` | 写明被控端 K 的 RTT 来源；切换和回退时重置 `min_rtt`（Phase 0 观察是否会造成码率抖动） |
| S3-9 | 第 108 行 | "Kotlin / TypeScript Phase 1 不改行为"不再成立：确认窗、指示条、`PermissionsChanged`、SAS 和路径显示都在 `client/` 的 TS 里；Android 和网页控制端也要能容忍新的事件类型和 `Problem` 类型（例如 `Busy`，也可以考虑复用现有的 `Problem::HostBusy`） | `problem.rs:129` | 改写范围说明，把前端改动列进 F21 / F23 / F26；补"旧前端忽略未知事件"的用例 |
| S3-10 | 第 199 行 | 同一时刻只弹一个确认窗，其余排队；控制端在等待期间断开时，确认窗或排队项要撤下，没写（Android 用 `take_pending` 处理，`session.rs:66`） | 同左 | 补 `prompt_withdrawn_when_controller_leaves` |
| S3-11 | 验收清单 | I15–I22 的覆盖缺口：<br>① 控制端一侧没有"授权里没有直连就不发 `DirectRequest`"的用例；<br>② 缺"已允许但没授予直连"时**两端**都不启动 helper 的用例（现有用例只测了"被拒绝"）；<br>③ I22 的"不可遮盖、不可关闭"无法自动化，应标为手动；<br>④ 缺 S2-2、S2-3 对应的负向用例 | 第 258-265 行 | 补 `controller_never_requests_without_direct_grant`、`approved_without_direct_never_spawns_helper_either_side`，并把 I22 的显示层验收列入手动项 |
| S3-12 | 实现位置 | 确认闸门在 Android 上已经实现了一份（`android/native`），桌面端又要在 `client/src-tauri` 里另写一份，两份逻辑容易走样 | — | 建议把闸门状态机（等 Hello → 询问 → 放行 / 拒绝、授权前缓存、授权记忆）下沉到 `airhand-link`，两端共用，验收用例也放在那里 |

### S4（建议）

- **S4-1**：阶段表第 291、292 行仍写"Phase 0 结果写入规格 r4"，与文首的"r5"矛盾。第 3 行和第 288 行的轮数也不一致。
- **S4-2**：feature map 编号顺序是 F21–F27 排在 F12 前面，而 F27 讲的是 CI 移植，和确认功能无关。F9 没写缓存重发；F18 仍是旧用例名；F26 漏了 `verification_string_changes_if_offer_is_substituted`。
- **S4-3**：SAS 的 transcript 里含 token 和地址。写明只显示哈希截断后的短串，而且计算过程不进日志（I7）。
- **S4-4**：`HostHello.granted` 在 `approve = "never"` 时取什么值（例如除直连外全部为真），需要写明。
- **S4-5**：第 3 轮的 S4-2 还没处理（见第二节表格）。

---

## 四、回答

**r4 能否作为 Phase 0 的依据？能。**

- Phase 0 清单的 9 项和 go/no-go 阈值都已经获批，测量对象和本轮发现的问题互不影响。
- 建议（不阻断）在 Phase 0 顺带做两项可行性验证：
  1. Windows 上确认窗能否过滤注入输入（`LLMHF_INJECTED`）；
  2. SYSTEM agent 能否在登录屏 / 安全桌面上弹出确认窗并接收点击。
- 提醒：spike 的 helper 会真实暴露地址，spike 期间只在自己的测试设备之间运行。

**批准 (B) 之前还需要：**

1. **规格 r5，填入 Phase 0 实测数据**：
   - 选定数据报通道 (i) 或 (ii)；
   - 各项超时的实测值：升级超时、栅栏上限、T、Ping 间隔、K、M、`Status()` 轮询；
   - I3 窗口和 DERP 数据报数量的上限；
   - 同进程多个 `Server` 是否安全；
   - Windows 方案结论，或者采用退路；
   - 每项阈值都附达标 / 不达标的数据；
   - **补上直连会话的码率上限策略**，关闭 S2-3 的剩余部分。
2. **关闭本轮 S2-1 到 S2-5**，并修正第一节里不符的断言：`Peer` 的字段、`is_final` 的术语、停止共享发的 `Bye` 原因、服务器限速、`AIRHAND_MAX_SESSIONS`。补上 RustDesk 引用的 commit 和行号，并重新核实 `password-click` 的定性。
3. **项目负责人对开放问题 11–14 作出决定**。第 11 项默认 `ask` 会改变无人值守主机的使用方式，第 12 项涉及固定控制码，这两项直接影响 Phase 1 的范围和 Windows 安装版的可用性。
4. **处理 S3-1 到 S3-12**，重点是：
   - S3-3：取消安全的 helper 清理；
   - S3-5：剪贴板权限双向执行；
   - S3-11：补齐验收用例。
5. **r5 再做一轮复审**，直到没有 S1，然后由项目负责人在 PR 里**显式批准 (B)**，另开分支按"先红后绿"实现。

---

## 作者处理（规格 r4.1）

结论为**无 S1**，因此未再追加一轮评审；报告中的 S2 / S3 / S4 与事实订正已在 r4.1 中逐条处理（对应规格章节）：

| 报告条目 | 处理 |
|---|---|
| 事实：`Peer` 无 OS 字段 | 已订正：确认窗显示自报设备名与中转看到的来源 IP，不显示 OS |
| 事实：“致命”术语 | 已订正：统一为 `Problem::is_final()`（终止性），并注明不是 `is_fatal()` |
| 事实：停止共享的 `Bye` 原因 | 已订正：停止共享发 `Bye(CodeChanged)`；“断开此控制端”发 `Bye(Kicked)`，并新增 `HostCmd::Kick { session_id }` |
| 事实：服务器限速、`AIRHAND_MAX_SESSIONS` | 已删除；改为被控端本地按 IP + 全局速率限制，超限立即 `Bye(Declined)`；`max_pending_approvals` 默认 2 |
| 事实：RustDesk `password-click` | 已订正为“口令与点击二选一”（`ApproveMode::Both`），并记录核对的 master 提交；本设计不采用“有口令即免点击” |
| S2-1 I15 与现有机制矛盾 | I15 改为处理 `Hello` / `Ping` / `Bye`，`Settings` / `Audio` / `SelectDisplay` 记下最新值、确认后生效；用例拆分 |
| S2-2 注入输入代点确认 | 新增“防注入”：指示条只能收回权限；有带输入权限的在途会话时确认窗不显示“允许直连 / 允许”；Windows 过滤注入输入（待验证）；新增用例 |
| S2-3 授权记忆 | 新增清除规则；直连权限不随记忆复用；被断开设备须重新确认；用例 |
| S2-4 Windows 确认窗位置 | 定为托盘进程 + `ToAgent::Approve/Decline` + 管道客户端校验；登录屏 / 锁屏无托盘 ⇒ 超时拒绝（结论） |
| S2-5 滥用防护 | 见上“事实”条；新增“拒绝不会让控制码失效”的说明与排队计时规则 |
| S3-1~S3-12 | 已落实：停止共享原因、按会话踢人命令、取消安全的 helper 清理、`ControllerMsg::Bye` 无字段与断开顺序、权限双向执行点（剪贴板 / 声音）、`SelectDisplay` 缓存同步、直连选项显示条件、被控端 K 的 RTT 来源、前端 TypeScript 改动范围、排队项撤下、验收用例补全、确认闸门下沉到 `airhand-link`（I23） |
| S4 | 阶段表 / 轮数 / 阈值表措辞订正；feature map 行序与用例；SAS 只显示截断短串且不进日志；`approve = "never"` 时 `granted` 取值 |
| 第 3 轮 S2-3 的剩余部分 | 码率上限策略仍依赖 Phase 0 数据，明确推迟到 r5；Go 用例 RTT 范围与阈值表对齐 |
| 仍待 Phase 0 / 项目负责人 | r5 数据项；开放问题 11–14；Windows 注入过滤 / 安全桌面确认的可行性（可选验证） |
