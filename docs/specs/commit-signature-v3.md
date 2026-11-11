# 署名规范 v3（Code-Tool）与提交元数据隐私检查

状态：定稿，来自设计 r3（需求与架构评审结论 S1=0），并在实现时吸收最终评审的 R1–R3（S2）与 R5–R7；R8 作为后续加固写入 §9，本次不实现。D1–D5 已定（§10）。
记号：本文**不含任何真实邮箱**。项目负责人的 Gmail 写作 `<owner-gmail>`，他人邮箱写作 `<third-party-email>`；`<at>` 代表 `@`（例如 GitHub 网页产生的 committer 地址写作 `noreply<at>github.com`）。仓库里只存小写邮箱的 sha256。测试里的非法 / 夹带邮箱样例一律运行时拼接，测试源码里不出现完整的邮箱样式。

## 0. 背景与目标

署名用来区分**代码实际由哪个工具写**（`codex`、`claude-code`、`cursor-agent`；纯 git 操作另有值）。项目负责人允许自己的 Gmail 出现在提交元数据（author / committer / tagger）；`Co-authored-by` 一律去掉；文件内容里的邮箱照旧拦，他人邮箱与凭据照旧拦；放行名单不含邮箱明文（用哈希）；不新建 Actions variable / secret，不改仓库设置。

v2 的做法（`Agent:` 与 author 名绑定、AI 提交强制 noreply、CI 不扫提交元数据、`docs/workflow-v2.md` 禁止写模型档位）全部被本规范取代。

## 1. trailer 规范

### 1.1 归一化与 trailer 块

1. 归一化：`\r\n` → `\n`；每行去掉行尾空白；去掉信息尾部的空行。
2. 主题行 = 第一行。**trailer 块** = 最后一个空行分隔的段落，且 (a) 不是第一段（只有一段的信息没有 trailer 块）；(b) 该段**每一行**都匹配 `^[A-Za-z][A-Za-z0-9-]*: \S.*$`（键与冒号之间不许有空格；不支持续行折叠；只要有一行不匹配，整段就不是 trailer 块）。解析自己实现，不依赖 `git interpret-trailers`（后者更宽松）。
3. 计数：整条信息里所有匹配 `^\s*code-tool\s*:`（不区分大小写，含行首空白、冒号前空格）的行都计入 `Code-Tool` 行数；`Code-Source`（`^\s*code-source\s*:`）、`Agent`（`^\s*agent\s*:`）同理。
4. `Co-authored-by`：整条信息里匹配 `^\s*co-authored-by\s*:`（不区分大小写）的行一律禁止，合并提交也一样。

### 1.2 `Code-Tool`（必填，恰好一行，在 trailer 块内）

```
Code-Tool: <tool> <model> <effort>   # tool ∈ {codex, claude-code, cursor-agent}
Code-Tool: git-only                  # 纯 git 操作，见 §1.4
Code-Tool: human                     # 人工写
```

- `<model>`：`[a-z0-9][a-z0-9.-]{0,63}`，不知道写 `unknown`。
- `<effort>`：`none | low | medium | high | xhigh | max | ultra | unknown`；写不出档位时用 `unknown`。枚举可由文档修订扩展。
- 正则：`^Code-Tool: (?:(codex|claude-code|cursor-agent) ([a-z0-9][a-z0-9.-]{0,63}) (none|low|medium|high|xhigh|max|ultra|unknown)|git-only|human)$`；`git-only`、`human` 后面不许带字段。
- 多个工具实质参与时优先拆提交；无法拆分时写写得最多的主要工具，并在正文说明（正文不写模型 / 档位）。
- 它是**自述**，CI 只验证形式；真实性靠评审与流程（§9）。

### 1.3 `Code-Source`（D3）

写法：`Code-Source: <tool> <model> <effort>`（语法同 §1.2）、`Code-Source: human` 或 `Code-Source: unknown`（连工具也说不清）。**不许 `git-only`**。最多一行，必须在 trailer 块内（计数规则同 `Code-Tool`）。

- 非合并提交写 `Code-Tool: git-only` 时必须带恰好一行 `Code-Source`。
- `Code-Source` 只允许与 `Code-Tool: git-only` 同时出现；与其他 `Code-Tool` 值、或在没有 `Code-Tool` 行的提交里出现 → `code-source-not-allowed`。
- 干净合并（§3.2）不要求 `Code-Tool`，也不要求 `Code-Source`；写了就照常检查。
- 例：把 Cursor Agent 的格式化结果在 box 上重放：`Code-Tool: git-only` + `Code-Source: cursor-agent claude-opus-5-5 medium`；档位不可证时 `Code-Source: cursor-agent claude-opus-5-5 unknown`；连工具都说不清时 `Code-Source: unknown`。

### 1.4 `git-only` 的语义

只表示“**提交动作**是纯 git 操作，没有人或工具往里写新代码”：

- `cargo fmt` 这类格式化输出是代码改动：谁运行了格式化，谁（或其工具）就是写代码者。用 `commit-tree` 把格式化结果重放到新基线时，提交动作是 git，来源用 `Code-Source` 保留（必填）。
- 普通 `git rebase` / `git cherry-pick` 保留原提交信息与原 `Code-Tool`，不需要写 `git-only`。
- 干净合并上写 `Code-Tool: git-only` 允许，且 `Code-Source` 不是必填（写了必须合法）。非干净的合并按普通提交处理，必须写真实工具或 `human`，**不接受 `git-only`**。

### 1.5 `Agent:`（可选）

0 或 1 行；若存在，整行须匹配 `^Agent: [a-z0-9][a-z0-9-]{0,39}$`。不与 author 名绑定，没有白名单，不作为工具判断依据（v2 的 `AGENTS` 表、`author-does-not-match-agent`、`unknown-agent`、`ai-author-marked-human` 已删除）。

### 1.6 其他提交信息规则

- 禁止 AI 花费 / token 用量字样（`ai-cost-or-usage-in-message`，正则不放宽），合并提交也适用。
- `docs/workflow-v2.md` 的对应条款（D2）：PR 正文、提交信息、评论和文档里不写 AI 花费、token 用量或运行耗时；**模型与档位允许**写在 `Code-Tool` / `Code-Source` trailer、评审记录和 PR 说明里。
- 隐私条款：邮箱不得出现在 PR 正文、提交信息正文、评论和文件内容，例外只有 noreply 邮箱与 `example.com/.org/.net`、`localhost`（即 `privacy-scan` 的 `ALLOW_EMAIL`）。提交元数据（author / committer / tagger 的邮箱字段）另按 §2 判断，且元数据里**不**放行 `example.*`。
- 合法 `Code-Tool` 行、默认 `git revert` 信息里的 40 位 SHA、放行表里的 64 位十六进制串对 `scanText` 与花费正则均为 0 命中（测试 10、25 固定）。

## 2. author / committer / tagger 邮箱规则

邮箱字段先 `trim`、转小写，且必须**恰好含一个 `@`**、不含空白。允许类：

1. noreply 整串匹配：`^(?:\d+\+)?[a-z0-9](?:[a-z0-9-]{0,38})(?:\[bot\])?<at>users\.noreply\.github\.com$`（不支持带 `_` 或 `.` 的登录名）。
2. 仅 committer：`noreply<at>github.com`（GitHub 网页产生的提交）。
3. `sha256(小写邮箱)` 命中放行表。

**tagger（D5，已定）**：附注标签对象与 `mergetag` 内嵌标签的 `tagger` 行，邮箱与 author **同一套规则**（第 1、3 类：GitHub noreply 或放行表里的哈希）；不允许 → `tag-meta-email`（标签对象）/ `commit-meta-email`（合并提交的 `mergetag` 内）。每个标签对象（含 `mergetag` 内嵌标签）**恰好一行** `tagger`：重复 → `tag-ident-malformed`（`mergetag` 内 → `commit-ident-malformed`）；**缺少 tagger**（早期 git 产生的旧式标签）同样判为 `*-ident-malformed`（失败闭合：无法确认标签作者）。处理方式：用当前 git 重新创建该标签（`git tag -a` 总会写入 tagger），或由项目负责人对该次推送明确豁免；不通过改脚本绕过。

放行表 `scripts/allowed-author-hashes.json`：`{ "version": 1, "sha256_lowercase_email": [ { "sha256": "<64 位小写十六进制>", "label": "owner" } ] }`。`label` 只写角色，不含 `@`；条目不得重复。唯一条目是 `<owner-gmail>` 的小写 sha256：`8296231a7b969061e4a162230595f1238eead503866e86a022ce6f726cd70e17`。

- 增补规则（D1）：他人邮箱必须先经项目负责人明确批准、并经本人同意才能进放行表；任何新增 / 修改放行表的 PR 都会让 `policy-gate` 变红（§2.1）。`Code-Tool: human` 的提交没有“任意邮箱”例外。
- 哈希只做精确匹配：点号变体、`+tag` 变体不会命中，会被拒绝（预期行为）。
- 放行表文件缺失、不是合法 JSON、`version` 不是 1、某条 `sha256` 不是 64 位小写十六进制、条目重复或 `label` 含 `@` → 两个脚本均以退出码 2 失败。

共享模块 `scripts/commit-email-policy.mjs` 导出 `normalizeEmail`、`sha256Hex`、`loadAllowedHashes(file?)`（返回 `Set`）、`isAllowedAuthorEmail`、`isAllowedCommitterEmail`、`isAllowedTaggerEmail`（= author 规则），以及提交信息清理用的 `readCommentPrefix`、`cleanMessage`（§3.3）。

### 2.1 放行表的来源与政策文件变更门

- (a) **CI 读 base 版放行表**（纵深防御）：`workflow-v2.yml` 的 `agent-trailer` 步骤与 `privacy.yml` 的提交扫描步骤先执行 `git show "origin/${BASE_REF}:scripts/allowed-author-hashes.json" > "$RUNNER_TEMP/base-allowed-author-hashes.json"`；base 上没有该文件时写入空表 `{"version":1,"sha256_lowercase_email":[]}`（只放行 noreply）；base 引用本身不存在时步骤失败。脚本以 `--allow-hashes-file "$RUNNER_TEMP/base-allowed-author-hashes.json"` 调用。局限：这两个 job 的定义同样来自 PR，PR 可以改掉这一步；修改工作流的情形由 (b) 兜住。
- (b) **独立工作流 `.github/workflows/policy-gate.yml`，触发事件只有 `pull_request_target`**：运行 base 分支上的定义，PR 里对它的修改或删除不影响当次运行。
  - 只比对文件名：`gh api` 读 `GET /repos/{repo}/pulls/{n}/files`（含重命名的 `previous_filename`）；**不 checkout、不执行 PR 的任何代码、不使用任何 action**；PR 控制的字符串不插值进 `run:`，只经 `env:` 传入；PR 号与 `changed_files` 先校验为纯数字。
  - 权限：`permissions: { contents: read, pull-requests: read }`，不使用 secrets。
  - **政策文件清单**（任一 `filename` 或 `previous_filename` 命中即红）：`scripts/allowed-author-hashes.json`、`scripts/commit-email-policy.mjs`、`scripts/agent-trailer.mjs`、`scripts/privacy-scan.mjs`、`scripts/check-spec-tests.mjs`、`tests/workflow-scripts.mjs`、`.gitleaks.toml`、**`.github/**`**（R6：整个 `.github/` 目录，含 `policy-gate.yml` 自身、今后新增的工作流与本地复合 action；bash `case` 模式里的 `*` 本就跨越 `/`）、`.githooks/*`。`docs/` 下的规范文档不列入（避免每个文档 PR 都红；文档里的隐私问题由 `--diff` 扫描负责）。
  - 红灯含义：本 PR 触及政策文件（权限或安全边界变更），需要项目负责人明确批准。独立成工作流，不污染 `privacy-scan`、`agent-trailer` 的结论。`main` 没有分支保护与必需检查，红灯**不阻断合并**，是信号。
  - 失败闭合：改动文件数 ≥ 3000（该 API 的上限）、API 调用失败、PR 号非法，一律退出非 0。输出的路径名经 `tr -c 'A-Za-z0-9._/-' '?'` 过滤，避免伪造 `::error::` 之类的工作流命令。
  - 引入本文件的 PR 自身不会触发这道门（base 上还没有它）；该 PR 本来就须项目负责人明确批准（§6）。
- (c) 测试 42 断言：`workflow-v2.yml`、`privacy.yml` 里每一处 `--allow-hashes-file` 的实参都恰为 `"$RUNNER_TEMP/base-allowed-author-hashes.json"` 且同一步骤里前面有 (a) 的 `git show`（push main 报告步骤除外，见 §4.3(2)）。

### 2.2 哈希放行的风险与替代

邮箱熵低，持有哈希的人可对候选邮箱离线验证。对 `<owner-gmail>` 这一条不构成新增暴露（`main` 的既有历史 author 已含明文）。他人的哈希必须“本人同意 + 项目负责人批准”。替代方案（Actions variable、按 GitHub 账号关联）分别属于改仓库设置或需联网，不采用。

## 3. `agent-trailer.mjs`

### 3.1 接口、模式与错误码

`checkAgentTrailer({ message, authorEmail, committerEmail, isMerge, isCleanMerge, allowedHashes })`（`allowedHashes` 为小写十六进制哈希的 `Set`，缺省为空集；`authorName` 不再使用）。模式（恰好一个）：`--message-file FILE`、`--range BASE..HEAD`、`--range-all BASE..HEAD`、`--shas-stdin`；修饰：`--require-nonempty`、`--allow-hashes-file FILE`（缺省为仓库里的放行表）。退出码：0 通过；1 规则违反；**2 检查本身失败**（git 调用失败、范围非法、对象缺失或类型不对、范围为空且带 `--require-nonempty`、放行表文件缺失或格式错误、参数错误），统一输出 `agent-trailer: check-failed`。

| 错误码 | 条件 |
|---|---|
| `missing-code-tool` | 需要 `Code-Tool` 的提交没有这一行 |
| `duplicate-code-tool` | 计数 > 1（含大小写 / 空白变体） |
| `invalid-code-tool` | 恰好一行、在 trailer 块内，但不匹配 §1.2 |
| `code-tool-outside-trailer-block` | `Code-Tool` 行不在 trailer 块内 |
| `git-only-not-allowed` | 非干净合并提交写了 `git-only` |
| `missing-code-source` | 非合并提交写了 `Code-Tool: git-only` 而没有 `Code-Source` |
| `duplicate-code-source` | `Code-Source` 计数 > 1 |
| `code-source-outside-trailer-block` | `Code-Source` 行不在 trailer 块内 |
| `invalid-code-source` | 恰好一行、在 trailer 块内，但语法非法（含 `git-only`、缺字段、非法档位） |
| `code-source-not-allowed` | 出现 `Code-Source` 而没有 `Code-Tool` 行，或 `Code-Tool` 是合法的非 `git-only` 值 |
| `invalid-agent-trailer` | `Agent:` 多于一行或值非法 |
| `co-authored-by-forbidden` | §1.1 第 4 点 |
| `author-email-not-allowed` / `committer-email-not-allowed` | §2 |
| `ai-cost-or-usage-in-message` | 沿用 |
| `check-failed` | 退出码 2 的统一输出 |

输出只含 `sha12 错误码`（信息文件模式为 `HEAD 错误码`），不输出邮箱。

### 3.2 合并提交只有“干净合并”才免 `Code-Tool`

对 `--range`、`--range-all`、`--shas-stdin` 里的每个提交（**同一条代码路径**）：

- **非合并提交**：执行全部检查。
- **合并提交**，按序判定，第一个命中的分支决定结果：
  1. 三个及以上父提交：**不调用 `merge-tree`**，按普通提交处理。
  2. 恰好两个父提交：`git cat-file -e <P>^{commit}` 确认二者都在本地；任一缺失 → 退出码 2（浅克隆、对象缺失属于检查失败；`merge-tree` 对不存在的对象也以 1 退出，与“有冲突”相同，所以必须先查）。
  3. `git merge-base P1 P2`：退出码 1（不相关历史）→ 按普通提交处理（其根提交本来就在范围里，会被逐个检查）；其他非 0 → 退出码 2。
  4. `git merge-tree --write-tree P1 P2`（git ≥ 2.38）：stdout 第一行必须是树对象 id，否则退出码 2。退出码 0：该树**等于** `<merge>^{tree}` → 干净合并；不等 → 夹带改动 → 普通提交。退出码 1：有冲突 → 普通提交。其他退出码 → 退出码 2。
  - 退化合并（一个父提交是另一个的祖先）：树与另一个父提交相同则干净，否则按普通提交。
- **干净合并上的检查**：`co-authored-by-forbidden`、邮箱、`ai-cost-or-usage-in-message`；`Code-Tool` / `Code-Source` / `Agent` 写了就照常检查计数、位置、语法与 `code-source-not-allowed`；只有 `missing-code-tool`、`missing-code-source`、`git-only-not-allowed` 不适用。
- **普通提交**（含所有“按普通提交处理”的合并）：要求 `Code-Tool`；合并提交不许 `git-only`。
- 已知误判：GitHub 服务端合并与本地 `merge-tree` 的结果理论上可能因策略不同而树不一致，被保守地要求 `Code-Tool`；由项目负责人明确豁免，不改脚本绕过。
- `--range-all`：与 `--range` 同一套判定（含干净合并免填），区别只在调用约定（用于 `main`、不默认带 `--require-nonempty`）。`main` 的旧历史没有 `Code-Tool`，启用前必须先设定起点提交；本次不启用。

### 3.3 其他模式

- `--message-file FILE`（commit-msg 钩子）：先清理，再执行与 CI 相同的检查；邮箱取自 `git var GIT_AUTHOR_IDENT` / `GIT_COMMITTER_IDENT`（取不到 → 退出码 2）。
  - 注释前缀：`git config core.commentString`（可为多字符）→ `core.commentChar` → `#`。显式前缀：丢弃从剪刀线（`<前缀> ------------------------ >8 ------------------------`）起的所有行，再丢弃以该前缀开头的行。
  - `auto`：git 会从 `#;@!$%^&|:` 里挑一个信息里没有以它开头的行的字符，不一定是 `#`。剪刀线用 `^[#;@!$%^&|:] -{24} >8 -{24}$` 识别并截断；然后**只剥离文件末尾那段连续的注释块**（块内每行是空行或以同一候选字符开头，该字符取自最后一个非空行的首字符），不剥离正文里以候选字符开头的行。已知限制：没有 git 生成的注释块（例如 `-m`）且正文最后一行恰以候选字符开头时，该行会被当成注释剥掉——只影响钩子。
  - `--cleanup=verbatim` 或 `-m` 时钩子清理后的文本可能与最终提交信息不一致；**钩子只是本地便利，pre-push 与 CI 为准**。
  - 合并状态：用 `git rev-parse --git-path MERGE_HEAD` 判断（worktree 里 `.git` 是文件），合并中只做邮箱、`co-authored-by-forbidden` 与花费字样检查。
- `--shas-stdin`：stdin 每行一个提交 id（40 或 64 位小写十六进制；其他格式、不存在或不是提交 → 退出码 2），逐个按 §3.2 检查。空输入 → 通过（除非带 `--require-nonempty`）。供 pre-push 与 push main 报告步骤使用。
- `--require-nonempty`：提交数为 0 时退出码 2。PR 模式的 CI 调用与 push main 报告步骤带它；pre-push 不带。
- 所有 git 调用失败一律退出码 2，不把 git 错误当作“0 个提交通过”。

## 4. `privacy-scan.mjs`

### 4.1 `--commits RANGE` 与 `--shas-stdin`

`--commits BASE..HEAD [--allow-hashes-file FILE] [--require-nonempty]`；`--shas-stdin [--allow-hashes-file FILE] [--require-nonempty]`（stdin 每行一个对象 id，类型必须是 commit 或 tag，用 `git cat-file -t` 区分）。范围参数校验同 agent-trailer。逐对象读原始字节，不用 `git log --format` 切分：

1. `git rev-list BASE..HEAD`（含合并提交）得到 N 个提交；git 失败 → 退出码 2；`--require-nonempty` 且 N = 0 → 退出码 2；扫描的提交数必须等于 N。
2. `git cat-file <type> <id>` 的原始字节：头部在第一个空行之前，以一个空格开头的行是上一个头的续行（去掉一个前导空格）；其余为正文。同一对象按 `utf8` 与 `latin1` 各解码扫描一遍，命中取并集。
3. 提交头部：
   - `tree`、`parent`：单行十六进制 id 时跳过，否则照扫。
   - `author`、`committer`：每个提交**恰好一个** `author`、一个 `committer` 头，缺少或重复 → `commit-ident-malformed`。每个头用严格正则 `^(author|committer) ([^<>]*) <([^<>]*)> \d+ [+-]\d{4}$` 解析（名字与邮箱字段都不许含 `<`、`>` 或换行），不匹配 → `commit-ident-malformed`。邮箱字段按 §2 判断（含放行表；元数据里不放行 `example.*`），不允许 → `commit-meta-email`。名字字段交给 `scanText`。**解析差异说明**：git 的 `%ae` 取名字后面出现的第一个 `<…>`，而 r2 设计的宽松正则取最后一个，二者可能不同（名字写成 `Name <邮箱样式>`、邮箱字段为 noreply 时，git 报出的是夹带的邮箱）；本规范禁止名字字段含 `<`/`>`，所以两者不会再不同；即使将来放宽，名字里夹带的那一段仍交给 `scanText`，不构成绕过。
   - `gpgsig`、`gpgsig-sha256`、`mergetag`：值按“装甲签名块规则”处理（下面）；`mergetag` 的值是内嵌标签对象全文，按 §4.1a 解析，字段名一律为 `mergetag`，tagger 邮箱不允许 → `commit-meta-email`，tagger 缺少 / 重复 / 畸形 → `commit-ident-malformed`。
   - 其他所有头（`encoding`、未知头、`x-*` 等）整行（含续行）交给 `scanText`，即使里面写了 `-----BEGIN … SIGNATURE-----` 也不豁免。
4. **装甲签名块规则**（只用于 `gpgsig`、`gpgsig-sha256`、`mergetag` 内嵌标签的说明、标签对象的说明）：取**最后一个**匹配 `^-----BEGIN (PGP SIGNATURE|SSH SIGNATURE|SIGNED MESSAGE)-----$` 的行为 BEGIN（与 git 的 `parse_signed_buffer` 取最后一个签名起始行一致；git 还认 `-----BEGIN PGP MESSAGE-----`，本规范不认，遇到时整段照扫）；它必须有同类型的 `-----END …-----`，且 END 是值的**最后一个非空行**。不满足 → 整个值照扫。满足时，只有 BEGIN 与 END 之间的**纯 base64 行** `^[A-Za-z0-9+/]+={0,2}$` 与**校验和行** `^=[A-Za-z0-9+/]{4}$` 不扫（BEGIN / END 行本身也不扫）；装甲头行（`Version:`、`Comment:` 等）、其他形状的行、BEGIN 之前与 END 之后的内容一律照扫。跳过 base64 行的原因：随机签名 base64 里 `+`、`/` 把串切成长短不一的段，`cloudflare-token-like` 规则会大量误报。
5. 正文：`scanText` 全部规则，**任何非 noreply / 非 `example.*` 的邮箱——包括 `<owner-gmail>`——都命中**，哈希表对正文不生效；**不认 `privacy-scan: allow` 行内豁免**（`scanText(text, where, { honorAllow: false })`）；正文里的装甲块一律不豁免；含 `\x01` 之类字节不影响解析。
6. `PRIVACY_WORDLIST` 对名字、头与正文同样生效。
7. 输出只含 `sha12:字段[:行号] 规则名`（字段：`author`、`committer`、`author-name`、`committer-name`、`header`、`gpgsig`、`gpgsig-sha256`、`mergetag`、`message`、`tag-header`、`tagger`、`tagger-name`、`tag-message`），stdout 另有 `privacy-scan: 已扫描 N 个对象` 与 `privacy-scan: N 处命中`；有命中退出码 1；检查失败输出 `privacy-scan: check-failed`，退出码 2。

#### 4.1a 附注标签对象

`git cat-file tag <id>`：头部 `object`、`type`、`tag`、`tagger`，空行，标签说明；签名（若有）直接接在说明末尾。

- `object`、`type`：跳过。`tag` 头与其他头整行交给 `scanText`（字段 `tag-header`）。
- `tagger`：恰好一行（缺少或重复 → `tag-ident-malformed`），严格正则 `^tagger ([^<>]*) <([^<>]*)> \d+ [+-]\d{4}$`，不匹配 → `tag-ident-malformed`；邮箱按 author 同一套规则（D5）→ 不允许为 `tag-meta-email`；名字交给 `scanText`。
- 标签说明：`honorAllow=false`，装甲块规则同上。
- 标签链：`type tag` 时沿 `object` 继续读下一层标签对象，每层都按本节处理；指向 commit 时，该提交的范围由调用方另行给出；指向 tree / blob 时到此为止。

### 4.2 `--message-file FILE`

与 agent-trailer 同样清理（§3.3）后用 `honorAllow=false` 扫描，命中输出为 `HEAD:message:行号 规则名`。`--files` / `--diff` / `--stdin` 保持原行为（认行内豁免）。

### 4.3 CI 与钩子

**(1) `privacy.yml` 的 `privacy-scan` job**（`fetch-depth: 0`）：在“扫描 PR 新增的行”之后加步骤“扫描 PR 提交元数据与提交信息”：先生成 base 版放行表（§2.1(a)），再 `node scripts/privacy-scan.mjs --commits "origin/${BASE_REF}..${HEAD_SHA}" --allow-hashes-file "$RUNNER_TEMP/base-allowed-author-hashes.json" --require-nonempty`；条件 `if: ${{ !cancelled() && github.event_name == 'pull_request' }}`（前一步失败也照样运行）。`workflow-v2.yml` 的 `agent-trailer` 步骤同样先生成 base 版放行表，再 `node scripts/agent-trailer.mjs --range "origin/${BASE_REF}..${HEAD_SHA}" --require-nonempty --allow-hashes-file "$RUNNER_TEMP/base-allowed-author-hashes.json"`。

**(2) push main 报告步骤**（失败即红；R2、R5）：

- 条件 `if: ${{ !cancelled() && github.event_name == 'push' && github.ref == 'refs/heads/main' }}`（R5：防止 `privacy.yml` 的 push 触发范围将来扩大到其他分支或标签）。**不带 `continue-on-error`**。
- 范围：`before` 是合法 40 位 id、不是全零且在本地存在时取 `git rev-list "$AFTER_SHA" --not "$BEFORE_SHA"`；否则退回 `git rev-list -n 20 "$AFTER_SHA"`。**两处 `rev-list` 都检查退出状态**（`if ! git rev-list …; then echo "::error::…"; exit 1; fi`），失败时步骤变红，不会因为空列表而静默变绿（R2）。
- 两个脚本都以 `--shas-stdin --require-nonempty --allow-hashes-file scripts/allowed-author-hashes.json` 调用（R2：push 事件至少有一个新提交；空列表即异常）。push main 没有 base，放行表取工作区里的表。
- 失败时写 `::error::` 注释与 `$GITHUB_STEP_SUMMARY`（只含 sha12 与规则名），并显式 `exit 1`。`main` 无保护，红灯不阻断任何操作，只起警报作用；**任何凭据命中一律按“已泄露，需要轮换”处理**，强推 / 重写历史后旧提交仍可通过 SHA 访问。
- `privacy.yml` 里唯一保留 `continue-on-error: true` 的步骤是“全库扫描（信息性）”。

**(3) `.github/workflows/policy-gate.yml`**：见 §2.1(b)。

**(4) 本地钩子**（需要 `git config core.hooksPath .githooks`；只是本地便利，CI 为准；脚本路径按钩子所在目录解析）：

- `.githooks/commit-msg`：`agent-trailer.mjs --message-file "$1"` 与 `privacy-scan.mjs --message-file "$1"`。
- `.githooks/pre-push`：`$1` 为远程名（或 URL），stdin 每行 `local_ref local_sha remote_ref remote_sha`。逐行：
  1. `local_sha` 全零（删除分支 / 标签）：跳过。
  2. **不使用 `remote_sha`**（它可能不在本地，`remote_sha..local_sha` 会以 128 失败）。范围用 `git rev-list "<local_sha 剥到提交>" --not --remotes="$1"`——**只排除正在推送的那个远程的跟踪分支**（R1）：已推到其他远程（例如私有镜像）的提交推到 origin 时仍会被扫。
  3. `$1` 不是已配置的远程名（例如直接写 URL），或该远程没有任何跟踪分支：范围无法确定，只检查 tip 一个提交，并在 stderr 说明“范围无法确定，CI 为准”。
  4. 空范围放行（不带 `--require-nonempty`）：新分支无新提交、标签指向已推送的提交都是正常推送。提交 id 经 `--shas-stdin` 交给两个脚本。
  5. `local_sha` 是附注标签时，把标签对象交给 `privacy-scan --shas-stdin`（脚本沿标签链逐层扫描，§4.1a）；剥到最后若是提交，提交范围按第 2 步算；指向 tree / blob 的标签只扫标签对象。
  6. 钩子自身 git 调用异常：报错并以非 0 退出（失败闭合）。
  - 已知局限：远程跟踪分支只与上次 `fetch` 一样新，领先于真实远端时会少扫；标签推送不经过 CI，附注标签的扫描只有 pre-push 这一道（§9）。git 在没有任何引用需要更新时不运行 pre-push。
- 文件内容的 `--diff` 规则与 `ALLOW_EMAIL` 完全不变：文件内容里的 `<owner-gmail>` 仍命中。

## 5. 测试（`tests/workflow-scripts.mjs`）

测试替身：放行表用 `owner` + `<at>` + `example.com` 的运行时 sha256，CLI 用 `--allow-hashes-file` 注入临时文件；邮箱样例运行时拼接（`fixture.invalid`、`example.com`、noreply 占位 `12345+fixture-bot`）；伪造的装甲块用 `crypto.randomBytes` 运行时生成；真实签名在临时目录里用 `ssh-keygen` / `gpg`（临时 `GNUPGHOME`）现签；对象用 `git hash-object -t <type> -w --literally` 构造；合并判定的 git 调用用 PATH 上的 git 包装器记录 / 模拟。

- A（trailer 解析）：1–11、5a–5c。
- B（合并提交）：12–18、18a–18c；18c 含改写自旧测试的三条。
- C（邮箱）：19–25、50，及 D5 的纯函数用例。
- D（`--commits` / `--shas-stdin`）：26–37、30a–30c、34a、35a，及 D5 的标签对象 / `mergetag` 用例（tagger 为放行哈希邮箱 → 通过；表外非 noreply 邮箱 → 拒绝；tagger 恰好一行）与标签链用例。
- E（信息文件与钩子）：38–41、38a、41a–41f，及 R1 的两条（已在另一个远程上的提交推到 origin 仍被扫描；推送目标为 URL 时只检查 tip）。
- F（工作流与文档一致性）：42–48（47 含 R2 的 `rev-list` 退出状态检查与 `--require-nonempty`、R5 的 `github.ref` 条件；48 含 R6 的 `.github/**`）。
- **R7**：30c 的真实签名用例在 `CI=true` 时缺 `gpg` / `ssh-keygen` 判失败；本地缺工具时明确输出 `SKIPPED  30c …`，不计入通过数。合成签名的 30c 用例不依赖外部工具，始终运行。

## 6. 与 workflow v2 的关系

- 分类：重大改动中的**权限或安全边界变更**（隐私检查对提交元数据放宽；新增 `pull_request_target` 工作流）。合并须项目负责人明确批准，且批准时需知道本改动新增了一个 `pull_request_target` 工作流（§9）。
- 角色分离：实现为 Cursor Agent（`claude-opus-5-5`，medium）；需求与架构评审为 claude.ai 网页（Opus 5.5 Medium），评审者没有读到仓库文件；独立代码评审另行进行，工具与档位如实记录（由项目负责人决定）。
- **先红后绿（R3）**：提交 A 只含测试——新测试**和按新语义改写的旧测试**都在提交 A 里，对旧脚本为红；提交 B 实现脚本、工作流、钩子、文档与本 spec。旧测试 → 新测试对应关系：

| 旧测试（v2 语义） | 取代它的新测试（v3 语义） |
|---|---|
| trailer：正确署名通过（只有 `Agent:`） | 4（合法 `Code-Tool` 通过）、9（`Agent:` 可选） |
| trailer：缺 Agent 失败 | 1（缺 `Code-Tool` → `missing-code-tool`） |
| trailer：非 noreply 邮箱失败 | 19（表外邮箱 → `author-email-not-allowed`） |
| trailer：author 与 Agent 不匹配失败 | 9（`Agent:` 与任意 author 名组合通过，绑定已删除） |
| trailer：人工提交允许本人普通邮箱 | 19（`Code-Tool: human` 也没有任意邮箱例外）、23 / 24（放行哈希） |
| trailer：人工提交仍可自选 noreply | 22（noreply 形态通过） |
| trailer：AI 名称不能伪装 human | 9（名字不再参与判断；`ai-author-marked-human` 删除） |
| trailer：AI committer 普通邮箱失败 | 19（`committer-email-not-allowed`） |
| trailer：AI 提交允许 GitHub 服务 committer | 20（网页 committer 地址：committer 合法、author 非法） |
| trailer CLI：message-file 拒绝 AI 普通 committer 邮箱 | E“信息文件模式”（陌生 committer → `committer-email-not-allowed`） |
| trailer CLI：message-file 接受 AI noreply committer | E“信息文件模式”（noreply + `Code-Tool` 通过） |
| trailer CLI：message-file 接受人工姓名与普通邮箱 | E“信息文件模式”（放行哈希邮箱通过、表外邮箱拒绝、缺 `Code-Tool` 拒绝） |
| trailer CLI：PR 范围保留原非 merge 检查语义（`--range` 跳过合并） | 18c（`--range` 检查合并：干净合并免填，夹带合并报错） |
| trailer CLI：main 检查包含缺标识的 merge 提交（`--range-all`） | 18c（`--range-all` 放行干净合并、拒绝缺 `Code-Tool` 的非干净合并） |
| trailer CLI：main 正确署名的所有提交通过 | 18c（`--range-all` 全部带 `Code-Tool` 的提交通过） |

- 合并方式：`docs/workflow-v2.md` 规定只用 “Create a merge commit”；不用 squash；“Rebase and merge” 不推荐。关闭 squash 属于仓库设置（§10）。

## 7. 迁移

- 引入本规范的 PR，其提交须同时满足新规则（noreply 身份 + `Code-Tool`；用 `commit-tree` 重建时写 `git-only` 并补 `Code-Source`）。该 PR 上 base 版放行表为空，只能用 noreply 身份；**不要点 “Update branch”**，也不要用项目负责人自己的身份在本地合并 `main`（测试 50 固定）。
- 合入后，PR #3、#4、#5 的下一次 CI 运行会用新脚本，对只有 `Agent:` 的旧提交报 `missing-code-tool`，属于预期过渡状态。决定（D4）：新规则评审通过后重建这三个 PR 的旧提交、补新 trailer、`--force-with-lease` 推送，不合并；每个 PR 重建前先回报新旧 SHA 对照与单独的扫描结果。不做按日期 / SHA 的宽限。
- Cursor VM 的 hook 会注入 `Co-authored-by`：建议 Cursor 云 agent 不直接 push，导出 `git format-patch`，在 box 上用 `git commit-tree` 重建（写好 `Code-Tool`），扫描后再推送。
- 给 Cursor / Codex / Claude Code 的提示须包含 `Code-Tool` 写法与“不写 `Co-authored-by`”。
- 由工具自动注入的 trailer 推导 `Code-Source` 的方案继续延后，核实前不做。

## 8. 实现记录

- 提交 A（只含测试）对旧脚本：113 项里 91 项失败（全部新测试与改写的旧测试），22 项通过（原有的 privacy / spec / go.sum 用例 21 项，及纯回归测试 36）。
- 提交 B 后全部通过；完整日志随 PR 说明提供。

## 9. 风险与限制

- `Code-Tool` 是自述字段，无法证明。
- 放行表与脚本随 PR 变动；§2.1 的 (a)(b)(c) 只能让“自我放行”不再静默，不能替代分支保护 / CODEOWNERS（§10）。`policy-gate` 是信号，不是阻断；改政策文件的直接推送不经 PR，门不运行（push main 报告步骤只检查提交元数据与信息）。
- **新增 `pull_request_target` 工作流**：该事件在 base 上下文运行，可访问 secrets，若检出并执行 PR 代码或把 PR 控制的字符串插值进 shell，就是“pwn request”。缓解：不 checkout、不执行 PR 代码、不使用任何 action；权限显式只读；不使用 secrets；PR 控制的字符串只经 `env:`，PR 号校验为数字，输出路径经字符过滤；清单写在 base 上的工作流里；测试 46 守护这些不变量，改 `.github/**` 本身会让门变红。它只对 base 上已有该文件的 PR 触发；在 GitHub 上以 `pull_request_target` 真实触发尚未验证（合入前无法触发）。
- 提交元数据里允许 `<owner-gmail>` 意味着该邮箱继续出现在 PR / 历史里，这是项目负责人的明确决定；D5 后它也可以出现在附注标签的 tagger 里（附注标签会出现在 Releases 页面）。
- `main` 无分支保护：直接推送、squash 合并、强推后的旧提交 CI 都管不到；只能靠 push main 报告步骤（失败即红）、pre-push 钩子与“命中即视为已泄露”的流程补位。push main 报告步骤在 `before` 不可用（新分支、强推）时只取最近 20 个提交，**强推后可能扫不全**（R5）；强推回退到已有提交时新提交列表为空，`--require-nonempty` 会让步骤变红，需人工确认。
- **装甲块里的纯 base64 行被跳过**：只含 `A-Za-z0-9+/` 与结尾 `=` 的凭据样式字符串（例如去掉首尾标记行的 PEM 私钥正文、只含这些字符的云密钥）可以藏在结构合法的 `gpgsig` / `gpgsig-sha256` / `mergetag` / 标签签名块里；邮箱、含 `-`、`_`、`.`、`@` 的凭据藏不进去。意外泄露几乎不会落进这里，有意隐藏者在文件内容里做编码同样能绕过 `scanText`，所以定为可接受的残余（S3）。
- **后续加固（R8，本次未实现）**：(1) 装甲块内每行不超过 76 个字符，只有最后一行可以更短；(2) SSH 签名：base64 解码后检查开头是否为 `SSHSIG` 魔数；(3) PGP 签名：解码后检查首个包是否为签名包（新格式 tag 2，即首字节 `0xC2`；或旧格式首字节 `0x88`–`0x8B`）；(4) 解码后的总长度设上限（例如 16 KiB）。做了之后，能藏下的只剩“恰好看起来像合法签名的随机字节”。
- 附注标签的扫描只有 pre-push 一道；pre-push 依赖远程跟踪分支的新鲜度，且只是本地便利。缺少 tagger 的旧式标签会被判为畸形（§2），需重建或明确豁免。
- 三个仓库（airhand、passkey-demo、ainews）的脚本已不一致，本改动只覆盖 airhand。
- 干净合并判定依赖 git 的合并策略，与 GitHub 服务端可能不同，会有保守误判（§3.2）。
- `--message-file` 在 `core.commentChar=auto` 下是启发式（§3.3），只影响本地钩子。

## 10. 决定与待拍板的仓库设置

**已定**

- **D1**：他人邮箱须先经项目负责人明确批准（并经本人同意）才能进放行哈希表。
- **D2**：允许在 trailer、评审记录和 PR 说明里写模型与档位；`docs/workflow-v2.md` 的对应禁令同步修改。
- **D3**：加 `Code-Source:`；非合并的 `git-only` 提交必须带它。
- **D4**：新规则评审通过后重建 PR #3、#4、#5 的旧提交补新 trailer，`--force-with-lease` 推送，不合并；属于合入后的后续步骤。
- **D5**：附注标签（含 `mergetag` 内嵌标签）的 tagger 邮箱与 author / committer 同一套规则：GitHub noreply，或其哈希在 `scripts/allowed-author-hashes.json` 里。每个标签恰好一行 tagger，缺少或重复判为畸形（§2）。他人邮箱进放行表仍须按 D1 先经项目负责人明确批准。

**待项目负责人拍板的仓库设置**（均不属于本改动）：

1. 关闭 squash 合并（只留 “Create a merge commit”）。
2. 给 `main` 开分支保护：要求 PR、要求状态检查（至少 `agent-trailer`、`privacy-scan`、`workflow-scripts`；若要让门成为阻断，再加 `policy-gate`）、禁止强推。
3. 为放行表、共享模块、两个检查脚本、`.github/`、`.githooks/` 加 CODEOWNERS 并要求代码所有者评审。
4. 若要以 Actions variable 代替哈希表，属于改设置。

## 11. 与设计 r3 的差异（实现时的取舍）

- `checkAgentTrailer` 增加 `isMerge` 参数：设计只列了 `isCleanMerge`，但“非合并 `git-only` 必须带 `Code-Source`”与“非干净合并不许 `git-only`”需要区分三种情形（非合并 / 干净合并 / 其他合并）。
- `--shas-stdin` 的 id 格式收紧为恰好 40 或 64 位（设计写的是 `{40,64}`），以便“长度不对”判为退出码 2。
- 标签链由 `privacy-scan --shas-stdin` 自己逐层读取（设计写的是由 pre-push 逐层取出）；pre-push 只需交出顶层标签对象，行为相同、逻辑集中在一处。
- 提交信息清理函数放在共享模块 `scripts/commit-email-policy.mjs` 里，两个脚本共用。
- 测试 38a 的 `core.commentString` 用例直接构造信息文件，不依赖 git ≥ 2.45 的行为（脚本只读取该配置项）。
- 提交 B 对提交 A 的测试做了两处测试侧修正（不改断言语义）：命中解析允许 `--message-file` 模式的 `HEAD:` 前缀（该模式没有对象 sha）；“推送目标为 URL”用例在推送前先新建一个提交（没有引用需要更新时 git 不运行 pre-push）。
