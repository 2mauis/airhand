# tailcat 临时点对点直连：设计评审记录

> 对象：[`tailcat-p2p.md`](tailcat-p2p.md) 与 [`tailcat-p2p-feature-map.md`](tailcat-p2p-feature-map.md)。
> 性质：**独立的需求与架构设计评审**（对应 workflow v2「实现与调试、需求与架构评审、独立代码评审分开执行」中的需求与架构评审）。
> 这**不是**对实现代码的独立代码评审（那一步在实现 PR 上另行进行），也不代替项目负责人对重大改动的批准。

## 评审方式

- 评审者：Claude Code（由项目负责人指定并授权用于本次设计评审）；与撰写规格的是不同的执行者。
- 权限：只读（`Read` / `Grep` / `Glob`），禁止写文件、执行命令、联网；评审时的工作目录是本 fork 的检出（含上游源码），并只读挂载了 tailcat 源码检出；未提供任何凭据或内网信息。
- 输入：规格 + feature map + 上游源码 + tailcat 源码；复审轮另附前一轮报告。
- 输出：分级 findings（S1 严重：编码前必须修订；S2 重要；S3 一般；S4 建议）；每条给位置、依据、建议。
- 规格作者在每轮后逐条处理，并对关键事实用 `git show v0.7.0:…` 自行复核（见下表“作者核对”）。

## 四轮结果

| 轮次 | 对象 | 结论 | S1 | 主要内容 | 全文 |
|---|---|---|---|---|---|
| 1 | r1 | 不通过 | 3 | I3 无法实现（tailcat 不能禁用 DERP 数据面）；控制流切换会乱序 / 丢输入；Windows SYSTEM 被控端 helper 无降权方案；另有 9 条 S2（版本断言错位、路径检测 API 写错、对恶意中转的表述过度、v10 部署影响、postcard 前向兼容、tailcat 默认放行范围、打包遗漏等） | [round 1](review/tailcat-p2p-round1.md) |
| 2 | r2 | 有条件通过 | 2 | 媒体看门狗与“静止桌面不发帧”冲突会反复误回退；回退方向无顺序规则；并更正了第 1 轮的一个前提（桌面被控端没有“同意”环节） | [round 2](review/tailcat-p2p-round2.md) |
| 3 | r3 | 有条件通过 | **0** | r3 可提交项目负责人做批准 (A)（仅批准 Phase 0 spike）；新增 S2 / S3 列入规格“r4 待办”，批准 (B) 之前关闭 | [round 3](review/tailcat-p2p-round3.md) |
| 4 | r4 | 有条件通过 | **0** | r4 可作为 Phase 0 的依据；新增 5 条 S2（I15 与现有机制矛盾、远端注入输入代点确认、授权记忆范围、Windows 确认窗位置与授权通道、滥用防护两处不可实现）与 12 条 S3，另核出 4 处事实断言不符（`Peer` 无 OS、`is_final` 术语、停止共享的 `Bye` 原因、服务器限速）；作者已在 r4.1 中订正 / 落实（见报告末“作者处理”） | [round 4](review/tailcat-p2p-round4.md) |

## 作者核对的关键事实（对 tailcat `v0.7.0`）

- tag `v0.7.0` 的 commit 是 `15ab9e6`（`546a07d` 是附注 tag 对象），`go.mod` 要求 Go 1.27.1。
- `Server.AllowedClients` 为空时放行所有客户端；`AddAllowedClient` 存在，且在加入第一个公钥前同样放行所有人；`AllowClient` 钩子、`perf`、`libtailcat` 仅在 `main`，不在 `v0.7.0`。
- `ServedTCPPorts` / `ServedUDPPorts` 存在，`ServedTCPPorts == nil` 时对本机地址放行全部 TCP 端口。
- `MaxUDPPayload = 1232`；有 `Client.DiscoPing`、`Server.Status`、`Client.PublicKey`。
- 仍未证实（列入 Phase 0）：`Client.Ping` 内部超时、超 1232 字节写入的库内行为、magicsock 何时经 DERP 双发、`CurAddr` 在路径失效后的变化时延、Windows 受限令牌 / AppContainer 下的行为、quinn 0.11 配置项的确切名称。

## 当前状态与门禁

- 设计评审：**第 4 轮无 S1**（对象 r4，复审结论：可作为 Phase 0 依据）。第 3 轮的 S2 / S3 中不依赖 spike 数据的已在 r4 落实；第 4 轮的 S2 / S3 已在 r4.1 中处理，**仍待 Phase 0 数据**的项在规格“r4 处理记录”中列出，由 r5 与下一轮复审关闭。
- 项目负责人 2026-11-10 已批准 (A)（仅 Phase 0 spike：独立分支、不合并、spike 代码不复用）与全部建议阈值，并追加“被控端确认 / 主动断开”要求（已写入 r4）。**尚未批准 (B)**（Phase 1 全量实现）：须在 Phase 0 结果 → r5 → 复审无 S1 之后。仍待确认的开放问题见规格 11–14。
- 未编码、未合并、未发布；未向上游提交。
