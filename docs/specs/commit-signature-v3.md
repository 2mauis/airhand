# 署名规范 v3（Code-Tool）与提交元数据隐私检查

状态：定稿，来自设计 r5.1（在 r3 的基础上：来源证据、Co-authored-by、工具 bot 邮箱）。r3 部分来自设计 r3（需求与架构评审结论 S1=0），并在实现时吸收最终评审的 R1–R3（S2）与 R5–R7；R8 作为后续加固写入 §9，本次不实现。D1–D5 与 D6–D9、OQ1–OQ4 已定（§10）。
记号：本文**不含任何真实邮箱**。项目负责人的 Gmail 写作 `<owner-gmail>`，他人邮箱写作 `<third-party-email>`；`<at>` 代表 `@`（例如 GitHub 网页产生的 committer 地址写作 `noreply<at>github.com`，工具 bot 邮箱也一律写成带 `<at>` 的形式）。仓库里只存小写邮箱的 sha256。测试里的非法 / 夹带邮箱样例一律运行时拼接，测试源码里不出现完整的邮箱样式。

## 0. 背景与目标

署名用来区分**代码实际由哪个工具写**（`codex`、`claude-code`、`cursor-agent`；纯 git 操作另有值），不是限制 noreply 或 `Co-authored-by`。`Co-authored-by` 是合法的来源证据之一，但**只有邮箱匹配已知工具表（§2.3）的行才能识别工具**；`Code-Tool` / `Code-Source` / 工具共同作者任一能明确识别工具即合格，人类提交照旧可标 `human`（D6）。项目负责人的 Gmail、GitHub noreply、已知工具 bot 邮箱允许出现在提交元数据（author / committer / tagger）与 trailer 块内的共同作者行；文件内容里的邮箱照旧拦，他人邮箱与凭据照旧拦；放行名单不含邮箱明文（用哈希）；不新建 Actions variable / secret，不改仓库设置。

v2 的做法（`Agent:` 与 author 名绑定、AI 提交强制 noreply、CI 不扫提交元数据、`docs/workflow-v2.md` 禁止写模型档位）全部被本规范取代。

## 1. trailer 规范

### 1.1 归一化与 trailer 块

1. 归一化：`\r\n` → `\n`；每行去掉行尾空白；去掉信息尾部的空行。
2. 主题行 = 第一行。**trailer 块** = 最后一个空行分隔的段落，且 (a) 不是第一段（只有一段的信息没有 trailer 块）；(b) 该段**每一行**都匹配 `^[A-Za-z][A-Za-z0-9-]*: \S.*$`（键与冒号之间不许有空格；不支持续行折叠；只要有一行不匹配，整段就不是 trailer 块）。解析自己实现，不依赖 `git interpret-trailers`（后者更宽松）。
3. 计数：整条信息里所有匹配 `^\s*code-tool\s*:`（不区分大小写，含行首空白、冒号前空格）的行都计入 `Code-Tool` 行数；`Code-Source`（`^\s*code-source\s*:`）、`Agent`（`^\s*agent\s*:`）同理。
4. `Co-authored-by`（严格解析；r3 的 `co-authored-by-forbidden` 已删除）：凡匹配 `^\s*co-authored-by\s*:`（不区分大小写，含行首空白、冒号前空格）的行都是**共同作者候选行**，必须同时满足以下两条才是「合法共同作者行」：
   1. 位于 trailer 块内；
   2. 整行匹配（键名大小写不敏感）：`^Co-authored-by: ([^<>\s](?:[^<>\n]*[^<>\s])?) <([\x21-\x3B\x3D\x3F\x41-\x7E]+@[\x21-\x3B\x3D\x3F\x41-\x7E]+)>$`。名字组首尾必须是非空白字符；邮箱组限制为 ASCII 可打印且不含 `<`、`>`、`@`（所以两种解码对邮箱字段的理解必然相同，见 §4.4）。

   其余一律 `coauthor-malformed`（失败闭合）：行首空白、`Co-authored-by :`、冒号后 Tab 或多空格、缺邮箱、缺尖括号、两个 `<>`、两个 `@`、名字为空、名字含 `<` / `>`、名字与 `<` 之间两个空格、折行续行（续行以空格开头，不满足块行正则，整段不再是 trailer 块）、全角 `＜＞`、位于 trailer 块之外。Unicode 连字符写的键名（如 U+2011）**不匹配前缀**，不算共同作者行，整行按普通正文处理（`privacy-scan` 照常命中其中的邮箱）。CRLF 先归一化再判，CRLF 的合法行通过。数量不设上限；同一行重复不算错。合并提交也适用。合法行的邮箱再按 §2 判断（不允许 → `coauthor-email-not-allowed`），是否构成来源证据见 §1.7。

### 1.2 `Code-Tool`（必填，恰好一行，在 trailer 块内）

```
Code-Tool: <tool> <model> <effort>   # tool ∈ {codex, claude-code, cursor-agent}
Code-Tool: git-only                  # 纯 git 操作，见 §1.4
Code-Tool: human                     # 人工写
```

- `<model>`：`[a-z0-9][a-z0-9.-]{0,63}`，不知道写 `unknown`。
- `<effort>`：`none | low | medium | high | xhigh | max | ultra | unknown`；写不出档位时用 `unknown`。枚举可由文档修订扩展。
- 正则：`^Code-Tool: (?:(codex|claude-code|cursor-agent) ([a-z0-9][a-z0-9.-]{0,63}) (none|low|medium|high|xhigh|max|ultra|unknown)|git-only|human)$`；`git-only`、`human` 后面不许带字段。
- 多个工具实质参与时优先拆提交；无法拆分时写写得最多的主要工具，删掉次要工具的共同作者行，并在正文说明（正文不写模型 / 档位；OQ2）。
- 它是**自述**，CI 只验证形式；真实性靠评审与流程（§9）。
- 例：`Code-Tool: cursor-agent claude-opus-5-5 high`（D9：今后的示例与新提交写 `high`；解析器的档位枚举不变）。

### 1.3 `Code-Source`（D3）

写法：`Code-Source: <tool> <model> <effort>`（语法同 §1.2）、`Code-Source: human` 或 `Code-Source: unknown`（连工具也说不清）。**不许 `git-only`**。最多一行，必须在 trailer 块内（计数规则同 `Code-Tool`）。

- 非合并提交写 `Code-Tool: git-only` 时必须带恰好一行 `Code-Source`。
- `Code-Source` 只允许与 `Code-Tool: git-only` 同时出现；与其他 `Code-Tool` 值、或在没有 `Code-Tool` 行的提交里出现 → `code-source-not-allowed`。
- 干净合并（§3.2）不要求 `Code-Tool`，也不要求 `Code-Source`；写了就照常检查。
- 例：把 Cursor Agent 的格式化结果在 box 上重放：`Code-Tool: git-only` + `Code-Source: cursor-agent claude-opus-5-5 high`；档位不可证时 `Code-Source: cursor-agent claude-opus-5-5 unknown`；连工具都说不清时 `Code-Source: unknown`。

### 1.4 `git-only` 的语义

只表示“**提交动作**是纯 git 操作，没有人或工具往里写新代码”：

- `cargo fmt` 这类格式化输出是代码改动：谁运行了格式化，谁（或其工具）就是写代码者。用 `commit-tree` 把格式化结果重放到新基线时，提交动作是 git，来源用 `Code-Source` 保留（必填）。
- 普通 `git rebase` / `git cherry-pick` 保留原提交信息与原 `Code-Tool`，不需要写 `git-only`。
- 干净合并上写 `Code-Tool: git-only` 允许，且 `Code-Source` 不是必填（写了必须合法）。非干净的合并按普通提交处理，必须写真实工具或 `human`（或凭单一工具共同作者合格，§1.7），**不接受 `git-only`**。
- 非合并提交的 `git-only` 仍必须带 `Code-Source`，**工具共同作者不豁免**（OQ6）。

### 1.5 `Agent:`（可选）

0 或 1 行；若存在，整行须匹配 `^Agent: [a-z0-9][a-z0-9-]{0,39}$`。不与 author 名绑定，没有白名单，不作为工具判断依据（v2 的 `AGENTS` 表、`author-does-not-match-agent`、`unknown-agent`、`ai-author-marked-human` 已删除）。

### 1.6 其他提交信息规则

- 禁止 AI 花费 / token 用量字样（`ai-cost-or-usage-in-message`，正则不放宽），合并提交也适用。
- `docs/workflow-v2.md` 的对应条款（D2）：PR 正文、提交信息、评论和文档里不写 AI 花费、token 用量或运行耗时；**模型与档位允许**写在 `Code-Tool` / `Code-Source` trailer、评审记录和 PR 说明里。
- 隐私条款：邮箱不得出现在 PR 正文、提交信息正文、评论和文件内容，例外只有 noreply 邮箱与 `example.com/.org/.net`、`localhost`（即 `privacy-scan` 的 `ALLOW_EMAIL`），以及**提交信息 trailer 块内合法共同作者行（§1.1 第 4 点）里按 §2 放行的邮箱**（只放宽邮箱字段本身，名字字段照常扫描，见 §4.4）。提交元数据（author / committer / tagger 的邮箱字段）另按 §2 判断，且元数据里**不**放行 `example.*`。单提交 PR 会用提交信息自动填 PR 正文，其中的共同作者行要删掉（PR 正文扫描会命中它，这是预期）。
- 合法 `Code-Tool` 行、默认 `git revert` 信息里的 40 位 SHA、放行表里的 64 位十六进制串对 `scanText` 与花费正则均为 0 命中（测试 10、25 固定）。

### 1.7 来源证据与合格判定（D6、D8）

证据记号：

| 记号 | 来源 | 取值 |
|---|---|---|
| **K**（声明） | `Code-Tool: <tool> <model> <effort>`；或 `Code-Tool: git-only` + `Code-Source: <tool> <model> <effort>` | 工具名（带 model / effort） |
| **H** | `Code-Tool: human`；或 `git-only` + `Code-Source: human` | 人工 |
| **U** | `git-only` + `Code-Source: unknown` | 不知道；**对冲突判定视同「无声明」** |
| **C** | 合法共同作者行里邮箱属于工具表（§2.3）的那些行，取其工具名集合 | 只有工具名，**无 model / effort** |

K / H / U 只取合法的 `Code-Tool` / `Code-Source` 值（非法、重复或块外的行不构成声明）。共同作者邮箱是 owner 哈希或普通 noreply（含 github-actions 等 bot）时**不是证据**，既不帮助合格也不冲突；名字字段不参与判断。author / committer 邮箱不是证据（OQ1）；`Made-with: Cursor` 不是证据（OQ3）。

判定顺序（每个提交，在 §1.1–§1.6 的形式检查之后）：

1. **形式检查**照旧；形式错误总是报告，不会因为有 C 而被豁免。
2. **冲突**（`source-evidence-conflict`，硬错误，不静默裁决，D8），满足任一即报：(a) K 存在，且 C 里有**不等于 K** 的工具；(b) H 存在，且 C 非空；(c) K、H 都不存在（含 U），且 C 含 **≥ 2 个不同工具**。同一工具多次出现、K 等于 C 的唯一工具不冲突。
3. **合格**（`missing-code-tool`，语义「没有任何来源证据」）：**非干净合并**的提交，若**没有任何 `Code-Tool` 行**且 C 的工具数不是恰好 1 → `missing-code-tool`。只要有 `Code-Tool` 行（哪怕非法）就不再附带 `missing-code-tool`（非法由 `invalid-code-tool` 报）；已报冲突时也不再报。干净合并免填。
4. `git-only` 仍必须带 `Code-Source`；工具共同作者不豁免（OQ6）。
5. 工具来自 C 时 model / effort 未知，不补不推断。
6. **notice**（只提示，不改变通过 / 失败）：非干净合并的提交，若**没有任何 `Code-Tool` 行**、C 恰好含 1 个工具、且无冲突，`agent-trailer` 向**标准输出**打印一行 `notice <sha12> source-evidence-from-coauthor-only`（`--message-file` 模式为 `notice source-evidence-from-coauthor-only`），不含邮箱，**不计入退出码**；让评审在 CI 日志里直接看到「只靠共同作者行合格、缺 model / effort」的提交。有 `Code-Tool` 行、只有 owner 共同作者、干净合并、已报冲突的提交都不输出。

冲突的处理：(a) 声明写错 → 改正；(b) 共同作者行是自动注入而与事实不符 → 在 box 上用 `commit-tree` 重建并去掉该行（重建对象写 `git-only` + `Code-Source`）；(c) 两个工具确实都参与 → **拆提交；拆不开就只写主工具，删掉次要工具的共同作者行，并在正文说明**（OQ2）；项目负责人豁免只作最后手段（§9）。

**决策表 T1–T15**（测试 60 逐行固定）：

| # | 场景 | 结果 |
|---|---|---|
| T1 | `Code-Tool: cursor-agent claude-opus-5-5 high` + owner 共同作者（云补丁实际形态） | 通过 |
| T2 | 只有 owner 共同作者，无 `Code-Tool` | `missing-code-tool` |
| T3 | 无 `Code-Tool` + Cursor 工具共同作者 | 通过（工具 = cursor-agent；带 notice） |
| T4 | 无 `Code-Tool` + Claude 共同作者（键名 `Co-Authored-By`） | 通过（带 notice） |
| T5 | `Code-Tool: codex …` + Cursor 工具共同作者 | `source-evidence-conflict` |
| T6 | `Code-Tool: cursor-agent …` + Cursor 工具共同作者 | 通过 |
| T7 | `Code-Tool: human` + Cursor 工具共同作者 | `source-evidence-conflict` |
| T8 | `Code-Tool: human` + owner 共同作者 | 通过 |
| T9 | 无声明 + Cursor、Codex 两个工具共同作者 | `source-evidence-conflict` |
| T10 | `git-only` + `Code-Source: claude-code …` + Cursor 共同作者 | `source-evidence-conflict` |
| T11 | `git-only` + `Code-Source: unknown` + 单一 Cursor 共同作者 | 通过 |
| T12 | 共同作者邮箱是他人个人邮箱 | `coauthor-email-not-allowed` |
| T13 | 共同作者行在 trailer 块之外 | `coauthor-malformed` |
| T14 | `git-only` + `Code-Source: unknown` + 两个不同工具 | `source-evidence-conflict` |
| T15 | 非法 `Code-Tool` + 单一工具共同作者 | 只有 `invalid-code-tool`，不附带 `missing-code-tool` |

**已知冲突场景**（测试 81 固定，不得为让它们通过而放宽规则）：共同作者的语义是「这个工具参与了这次提交」，不等于「代码由它写」，以下两种合法工作流会被判冲突，这是预期：
- 项目负责人写代码，让 Claude Code / Cursor 代为提交：`Code-Tool: human` + 工具共同作者 → 冲突。
- D4 重建 PR #3–#5 时，由开着归属功能的工具执行 `commit-tree`：`git-only` + `Code-Source: cursor-agent …` + Claude 共同作者 → 冲突。

缓解：做纯 git 操作或代提交人工代码时**关闭工具归属**——Claude Code 的 `attribution.commit`、Cursor 的 `attribution.attributeCommitsToAgent`、Codex 的 `commit_attribution_enabled`（只在程序文件里见到字段名，配置位置未核实）——并**提交后核对**（关设置后工具仍可能按默认提示写归属行，核对不能省）。

## 2. author / committer / tagger / 共同作者邮箱规则

邮箱字段（author / committer / tagger / `mergetag` 内 tagger / 合法共同作者行的邮箱组）先 `trim`、转小写，且必须**恰好含一个 `@`**、不含空白。四类，五个角色同一套：

| 类 | 判定 | author | committer | tagger | 共同作者 |
|---|---|:-:|:-:|:-:|:-:|
| ① GitHub noreply | `^(?:\d+\+)?[a-z0-9](?:[a-z0-9-]{0,38})(?:\[bot\])?<at>users\.noreply\.github\.com$`（含 github-actions、dependabot 等 bot；不支持带 `_` 或 `.` 的登录名） | ✓ | ✓ | ✓ | ✓ |
| ② GitHub 网页 committer | `noreply<at>github.com` | ✗ | ✓ | ✗ | ✗ |
| ③ owner 哈希 | `sha256(小写邮箱)` 命中放行表 | ✓ | ✓ | ✓ | ✓ |
| ④ 工具 bot 表 | 精确等于 `TOOL_IDENTITIES` 的某项（§2.3） | ✓ | ✓ | ✓ | ✓ |

- 判定顺序：先工具表，再 noreply，再哈希（`emailClass` 返回 `'tool' | 'noreply' | 'owner' | null`）。
- `example.*` 只是「不属于任何一类」，**不是**一条压过哈希类的显式拒绝规则：若某个 `example.*` 邮箱的小写 sha256 恰好在放行表里，它按 ③ 放行（实现**不得**加显式的 example 拒绝）。注意 `privacy-scan` 的 `ALLOW_EMAIL` 按**域名后缀**放行 noreply / `example.*` / `localhost`，这只影响正文扫描，与此处的分类无关；测试的 owner 夹具因此另用 `fixture.invalid` 域名，不依赖 `example.*` 的行为。
- 不允许时：author / committer → `author-email-not-allowed` / `committer-email-not-allowed`（`privacy-scan` 为 `commit-meta-email`）；共同作者 → `coauthor-email-not-allowed`（`privacy-scan` 为 `coauthor-email`）；tagger 见下。

**tagger（D5，已定）**：附注标签对象与 `mergetag` 内嵌标签的 `tagger` 行，邮箱与 author **同一套规则**（①③④：GitHub noreply、放行表里的哈希或工具表邮箱）；不允许 → `tag-meta-email`（标签对象）/ `commit-meta-email`（合并提交的 `mergetag` 内）。每个标签对象（含 `mergetag` 内嵌标签）**恰好一行** `tagger`：重复 → `tag-ident-malformed`（`mergetag` 内 → `commit-ident-malformed`）；**缺少 tagger**（早期 git 产生的旧式标签）同样判为 `*-ident-malformed`（失败闭合：无法确认标签作者）。处理方式：用当前 git 重新创建该标签（`git tag -a` 总会写入 tagger），或由项目负责人对该次推送明确豁免；不通过改脚本绕过。

放行表 `scripts/allowed-author-hashes.json`：`{ "version": 1, "sha256_lowercase_email": [ { "sha256": "<64 位小写十六进制>", "label": "owner" } ] }`。`label` 只写角色，不含 `@`；条目不得重复。唯一条目是 `<owner-gmail>` 的小写 sha256：`8296231a7b969061e4a162230595f1238eead503866e86a022ce6f726cd70e17`。

- 增补规则（D1）：他人邮箱必须先经项目负责人明确批准、并经本人同意才能进放行表；任何新增 / 修改放行表的 PR 都会让 `policy-gate` 变红（§2.1）。`Code-Tool: human` 的提交没有“任意邮箱”例外。
- 哈希只做精确匹配：点号变体、`+tag` 变体不会命中，会被拒绝（预期行为）。
- 放行表文件缺失、不是合法 JSON、`version` 不是 1、某条 `sha256` 不是 64 位小写十六进制、条目重复或 `label` 含 `@` → 两个脚本均以退出码 2 失败。

共享模块 `scripts/commit-email-policy.mjs` 导出 `normalizeEmail`、`sha256Hex`、`loadAllowedHashes(file?)`（返回 `Set`）、`TOOL_IDENTITIES`、`toolOfEmail`、`emailClass`、`isAllowedAuthorEmail`、`isAllowedCommitterEmail`、`isAllowedTaggerEmail`（= author 规则）、`isAllowedCoauthorEmail`（= author 规则），trailer 解析用的 `normalizeMessage`、`trailerBlock`（§1.1；`agent-trailer.mjs` 重新导出），共同作者行分析用的 `analyzeCoauthors`、`rebuildCoauthorLine`、`PLACEHOLDER`（§4.4），以及提交信息清理用的 `readCommentPrefix`、`cleanMessage`、`cleanMessageBytes`（§3.3）。

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

### 2.3 工具表（`TOOL_IDENTITIES`）

入表 = 精确匹配（`trim` 后小写、整串相等）的邮箱，既是四个角色的**放行邮箱**（§2 第 ④ 类），也是共同作者里的**工具来源证据**（§1.7 的 C）。近似写法（`+tag`、前后缀、大小写以外的任何变体、末尾点号）一律不匹配。工具表放在 `scripts/commit-email-policy.mjs` 里（常量 `TOOL_IDENTITIES`，以 `{ local, domain, tool }` 三元组存储、运行时拼 `@`），不放 JSON、不往 `ALLOW_EMAIL` 加任何域名：该文件已在 `policy-gate` 清单里，改工具表会让门变红，不必改清单；工具表不依赖放行表，所以在 PR 的 base 放行表为空时工具邮箱仍通过。**增删须项目负责人批准并附核实来源（D7）**。D5 的 tagger 规则同样适用工具表。

| 工具 | 邮箱（`<at>` = `@`） | 状态 | 核实来源 |
|---|---|---|---|
| cursor-agent | `cursoragent<at>cursor.com` | 已核实，入表 | 云 agent 补丁的 `From:`（author 为该邮箱，注入的共同作者是 owner）；Cursor 员工论坛确认 https://forum.cursor.com/t/disable-auto-co-authored-by-cursor-on-agent-commits-or-make-it-opt-in/168258 ；公开提交 https://github.com/content-hive/content-hive/commit/0075ed12de4869079e4ceee07ff92599bab45ef5 ；GitHub 账号 `cursoragent`（id 199161495）存在。官方文档 https://cursor.com/docs/cli/reference/configuration 只描述归属开关与 “Made with Cursor” trailer，没写邮箱。云 agent 的 committer 邮箱未核实 |
| claude-code | `noreply<at>anthropic.com` | 已核实，入表 | https://code.claude.com/docs/en/settings-reference （`attribution.commit` 默认 `Co-Authored-By: <模型名> <noreply…anthropic.com>`）；Claude Code 2.1.300 程序文件字符串；官方仓库 issue https://github.com/anthropics/claude-code/issues/72477 。trailer 落点未实测（§9） |
| codex | `noreply<at>openai.com` | 部分核实，入表 | Codex CLI 0.166.0 程序文件里的内置归属策略文本（含 `commit_attribution_enabled` 字段）；公开提交 https://github.com/SinhyeokKang/bugshot-2/commit/d96b07efcc735283019d226cf1c4d7f2ad71053f 。exec 模式实测不写它；交互模式 / App / Cloud 未实测；无官方文档页面。若后续确认全部路径都不写，可降为候选（走 D7） |
| codex | `199175422+chatgpt-codex-connector[bot]<at>users.noreply.github.com` | 已核实，入表（Codex Cloud） | GitHub API：`users/chatgpt-codex-connector[bot]` id 199175422、Type Bot；openai/codex 仓库提交以该 Co-authored-by 行结尾 https://github.com/openai/codex/commit/b527ee28907a7dcc68185f256c0441d90a07fe9d 。Codex Cloud 路径未实测 |
| （非编码工具）GitHub Actions | `41898282+github-actions[bot]<at>users.noreply.github.com` | 已核实；noreply 类，不入工具表，不是证据 | GitHub API id 41898282、Type Bot；actions/checkout 官方 README 建议值 https://github.com/actions/checkout/pull/1707 |

候选、不入表：`267193182+codex<at>users.noreply.github.com`（OQ4 已定不入；账号存在但归属 OpenAI 未经证实，User 类型；作为 noreply 放行，只是不算证据）；`codex<at>openai.com`（未核实，旧写法；不放行）；`codex<at>users.noreply.github.com`（无 id，未核实；作为 noreply 放行，不算证据）；Copilot 系（未核实，不在 `Code-Tool` 枚举内，OQ8）；`Made-with: Cursor` trailer（OQ3：不算证据；它是合法 `Key: value` 行，留在 trailer 块里不影响解析）。

## 3. `agent-trailer.mjs`

### 3.1 接口、模式与错误码

`checkAgentTrailer({ message, authorEmail, committerEmail, isMerge, isCleanMerge, allowedHashes })` 返回错误码数组；`analyzeAgentTrailer(同参数)` 返回 `{ errors, notices }`（`message` 为字符串或原始字节；`allowedHashes` 为小写十六进制哈希的 `Set`，缺省为空集；`authorName` 不再使用）。模式（恰好一个）：`--message-file FILE`、`--range BASE..HEAD`、`--range-all BASE..HEAD`、`--shas-stdin`；修饰：`--require-nonempty`、`--allow-hashes-file FILE`（缺省为仓库里的放行表）。退出码：0 通过；1 规则违反；**2 检查本身失败**（git 调用失败、范围非法、对象缺失或类型不对、范围为空且带 `--require-nonempty`、放行表文件缺失或格式错误、参数错误），统一输出 `agent-trailer: check-failed`。

| 错误码 | 条件 |
|---|---|
| `missing-code-tool` | 非干净合并的提交没有任何 `Code-Tool` 行，且工具共同作者的工具数不是恰好 1、也没有报冲突（§1.7 第 3 步） |
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
| `coauthor-malformed` | 共同作者候选行不是合法共同作者行（§1.1 第 4 点），或两种解码判定不一致（§4.4） |
| `coauthor-email-not-allowed` | 合法共同作者行的邮箱不在 §2 的允许类内（含 `example.*`、`noreply<at>github.com`、他人个人邮箱） |
| `source-evidence-conflict` | §1.7 第 2 步 |
| `author-email-not-allowed` / `committer-email-not-allowed` | §2 |
| `ai-cost-or-usage-in-message` | 沿用 |
| `check-failed` | 退出码 2 的统一输出 |

r3 的 `co-authored-by-forbidden` 已删除。输出只含 `sha12 错误码`（信息文件模式为 `HEAD 错误码`），不输出邮箱。提示行 `notice <sha12> source-evidence-from-coauthor-only`（信息文件模式为 `notice source-evidence-from-coauthor-only`）写到标准输出，不是错误码，不影响退出码（§1.7 第 6 步）。

### 3.2 合并提交只有“干净合并”才免 `Code-Tool`

对 `--range`、`--range-all`、`--shas-stdin` 里的每个提交（**同一条代码路径**）：

- **非合并提交**：执行全部检查。
- **合并提交**，按序判定，第一个命中的分支决定结果：
  1. 三个及以上父提交：**不调用 `merge-tree`**，按普通提交处理。
  2. 恰好两个父提交：`git cat-file -e <P>^{commit}` 确认二者都在本地；任一缺失 → 退出码 2（浅克隆、对象缺失属于检查失败；`merge-tree` 对不存在的对象也以 1 退出，与“有冲突”相同，所以必须先查）。
  3. `git merge-base P1 P2`：退出码 1（不相关历史）→ 按普通提交处理（其根提交本来就在范围里，会被逐个检查）；其他非 0 → 退出码 2。
  4. `git merge-tree --write-tree P1 P2`（git ≥ 2.38）：stdout 第一行必须是树对象 id，否则退出码 2。退出码 0：该树**等于** `<merge>^{tree}` → 干净合并；不等 → 夹带改动 → 普通提交。退出码 1：有冲突 → 普通提交。其他退出码 → 退出码 2。
  - 退化合并（一个父提交是另一个的祖先）：树与另一个父提交相同则干净，否则按普通提交。
- **干净合并上的检查**：`coauthor-malformed` / `coauthor-email-not-allowed`、author / committer 邮箱、`ai-cost-or-usage-in-message`；`Code-Tool` / `Code-Source` / `Agent` 写了就照常检查计数、位置、语法与 `code-source-not-allowed`；**有证据时 `source-evidence-conflict` 照常检查**（相对 r3 唯一变严之处）；只有 `missing-code-tool`、`missing-code-source`、`git-only-not-allowed` 不适用，也不输出 notice。
- **普通提交**（含所有“按普通提交处理”的合并）：要求来源证据（`Code-Tool`，或单一工具共同作者，§1.7）；合并提交不许 `git-only`。
- 已知误判：GitHub 服务端合并与本地 `merge-tree` 的结果理论上可能因策略不同而树不一致，被保守地要求 `Code-Tool`；由项目负责人明确豁免，不改脚本绕过。
- `--range-all`：与 `--range` 同一套判定（含干净合并免填），区别只在调用约定（用于 `main`、不默认带 `--require-nonempty`）。`main` 的旧历史没有 `Code-Tool`，启用前必须先设定起点提交；本次不启用。

### 3.3 其他模式

- `--message-file FILE`（commit-msg 钩子）：先清理，再执行与 CI 相同的检查；邮箱取自 `git var GIT_AUTHOR_IDENT` / `GIT_COMMITTER_IDENT`（取不到 → 退出码 2）。
  - 注释前缀：`git config core.commentString`（可为多字符）→ `core.commentChar` → `#`。显式前缀：丢弃从剪刀线（`<前缀> ------------------------ >8 ------------------------`）起的所有行，再丢弃以该前缀开头的行。
  - `auto`：git 会从 `#;@!$%^&|:` 里挑一个信息里没有以它开头的行的字符，不一定是 `#`。剪刀线用 `^[#;@!$%^&|:] -{24} >8 -{24}$` 识别并截断；然后**只剥离文件末尾那段连续的注释块**（块内每行是空行或以同一候选字符开头，该字符取自最后一个非空行的首字符），不剥离正文里以候选字符开头的行。已知限制：没有 git 生成的注释块（例如 `-m`）且正文最后一行恰以候选字符开头时，该行会被当成注释剥掉——只影响钩子。
  - `--cleanup=verbatim` 或 `-m` 时钩子清理后的文本可能与最终提交信息不一致；**钩子只是本地便利，pre-push 与 CI 为准**。
  - 信息文件按字节读取，清理时用 `latin1` 视图逐字节保留（`cleanMessageBytes`），清理后的字节交给 §4.4 的两种解码分析。
  - 合并状态：用 `git rev-parse --git-path MERGE_HEAD` 判断（worktree 里 `.git` 是文件），合并中只做 `MERGE_IN_PROGRESS_CHECKS` 里的检查：`coauthor-malformed`、`coauthor-email-not-allowed`、`source-evidence-conflict`（三项只依赖信息文本）、author / committer 邮箱与花费字样；合并中不输出 notice（是否“只靠共同作者合格”交给 pre-push 与 CI 按完整规则判断）。
- `--shas-stdin`：stdin 每行一个提交 id（40 或 64 位小写十六进制；其他格式、不存在或不是提交 → 退出码 2），逐个按 §3.2 检查。空输入 → 通过（除非带 `--require-nonempty`）。供 pre-push 与 push main 报告步骤使用。
- `--require-nonempty`：提交数为 0 时退出码 2。PR 模式的 CI 调用与 push main 报告步骤带它；pre-push 不带。
- 所有 git 调用失败一律退出码 2，不把 git 错误当作“0 个提交通过”。

### 3.4 提交信息的取得方式与解码（两个脚本输入一致）

- `agent-trailer` 的 `--range` / `--range-all` / `--shas-stdin`、`privacy-scan` 的 `--commits` / `--shas-stdin` 取提交信息**一律用 `git cat-file commit <sha>` 的原始字节**（第一个空行之后的部分），交给 `analyzeCoauthors` 与 `normalizeMessage`；不用 `git show` 的格式化正文输出（它会被 `i18n.logOutputEncoding` 重新编码，与 `privacy-scan` 读到的原始字节不同）。`--message-file` 模式两者读同一文件。author / committer 邮箱的取法不变（`%ae` / `%ce`）。
- **解码固定**：`agent-trailer` 的非共同作者检查（`Code-Tool` / `Code-Source` / `Agent` 的计数、位置、语法，trailer 块判定，花费字样）**固定用 `utf8` 解码**；`latin1` 解码只用于 `analyzeCoauthors` 的并集与一致性判断，以及 `privacy-scan` 的逐行扫描。`utf8` 下块不成立时，`Code-Tool` 等照常报「在块之外」之类错误（失败闭合）。测试 73、82、83、85 固定该性质。

## 4. `privacy-scan.mjs`

邮箱分类、工具表、trailer 解析与共同作者行分析都在共享模块 `scripts/commit-email-policy.mjs` 里（§2），`agent-trailer.mjs` 与 `privacy-scan.mjs` 只调用它，各自不重新实现。

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
5. 正文：`scanText` 全部规则，**任何非 noreply / 非 `example.*` 的邮箱——包括 `<owner-gmail>` 与工具邮箱——都命中**，哈希表与工具表对正文不生效；**唯一例外**是 trailer 块内合法共同作者行的邮箱字段（§4.4：重建后扫描，不允许的邮箱记 `coauthor-email`）；**不认 `privacy-scan: allow` 行内豁免**（`scanText(text, where, { honorAllow: false })`，共同作者行也一样）；正文里的装甲块一律不豁免；含 `\x01` 之类字节不影响解析。
6. `PRIVACY_WORDLIST` 对名字、头与正文同样生效。
7. 输出只含 `sha12:字段[:行号] 规则名`（字段：`author`、`committer`、`author-name`、`committer-name`、`header`、`gpgsig`、`gpgsig-sha256`、`mergetag`、`message`、`tag-header`、`tagger`、`tagger-name`、`tag-message`；`message` 的行号是**提交信息内**按 `\n` 分行的 1 起行号，与 `analyzeCoauthors().line` 相同，其他字段的行号仍是对象内行号；规则名新增 `coauthor-email`），stdout 另有 `privacy-scan: 已扫描 N 个对象` 与 `privacy-scan: N 处命中`；有命中退出码 1；检查失败输出 `privacy-scan: check-failed`，退出码 2。

#### 4.1a 附注标签对象

`git cat-file tag <id>`：头部 `object`、`type`、`tag`、`tagger`，空行，标签说明；签名（若有）直接接在说明末尾。

- `object`、`type`：跳过。`tag` 头与其他头整行交给 `scanText`（字段 `tag-header`）。
- `tagger`：恰好一行（缺少或重复 → `tag-ident-malformed`），严格正则 `^tagger ([^<>]*) <([^<>]*)> \d+ [+-]\d{4}$`，不匹配 → `tag-ident-malformed`；邮箱按 author 同一套规则（D5）→ 不允许为 `tag-meta-email`；名字交给 `scanText`。
- 标签说明：`honorAllow=false`，装甲块规则同上。
- 标签链：`type tag` 时沿 `object` 继续读下一层标签对象，每层都按本节处理；指向 commit 时，该提交的范围由调用方另行给出；指向 tree / blob 时到此为止。

### 4.2 `--message-file FILE`

与 agent-trailer 同样按字节清理（§3.3）后，按 §4.4 对两种解码逐行扫描（`honorAllow=false`），命中输出为 `HEAD:message:行号 规则名`。`--files` / `--diff` / `--stdin` 保持原行为（认行内豁免），文件内容规则与 `ALLOW_EMAIL` 一行不改：文件内容里的工具邮箱、`<owner-gmail>` 仍命中。

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
- 工具在 `commit-msg` 之前注入共同作者行，所以本地钩子与 CI 看到的信息一致；钩子、`pre-push` 与工作流 YAML 不因 r5.1 改动。

### 4.4 提交信息里的共同作者行（唯一的检查面放宽）

**接口**（写死）：`scripts/commit-email-policy.mjs` 导出

- `analyzeCoauthors(messageBuffer, hashes)` → 数组，**每个共同作者候选行一项** `{ line, status, tool, nameUtf8, nameLatin1 }`：`line` 是**原文按 `\n` 分行的 1 起行号**；`status` ∈ `'ok' | 'email' | 'malformed'`；`tool` 是工具名（仅当 `status === 'ok'` 且邮箱属工具表，否则 `null`）；`nameUtf8` / `nameLatin1` 是两种解码下按 §1.1 第 4 点正则取到的名字组（匹配不到为 `null`），供重建用。参数也接受字符串（按 utf8 编码成字节）。
- `rebuildCoauthorLine(nameGroup)` → `` `Co-authored-by: ${nameGroup} ${PLACEHOLDER}` ``，**只做模板拼接**，函数体里没有 `replace` / `replaceAll` / 正则字面量；`PLACEHOLDER = '<coauthor-email>'`（不含 `@`，不含主机名样式）同模块导出。

**`analyzeCoauthors` 的算法**：
1. 原始字节分别用 `utf8` 与 `latin1` 解码，各自归一化（`normalizeMessage`）、求 trailer 块。**候选行 = 任一解码下匹配 `^\s*co-authored-by\s*:` 的行，按行号取并集**：行首单字节 `0xA0` 只在 `latin1` 下是空白（U+00A0）；行首 BOM（EF BB BF）只在 `utf8` 下是空白（U+FEFF）。只在一种解码下有候选的行，在另一种解码下视为「无此行」。两种解码的行号一致（换行字节 0x0A 不变）。
2. 每个候选行在每种解码下：不在该解码的 trailer 块内、或不匹配 §1.1 第 4 点正则 → 该解码下不合法；合法则取邮箱做 §2 的类判断。
3. 最终状态：**两种解码都合法、取到相同的（归一化）邮箱、且邮箱被放行** → `ok`；**两种解码都合法但邮箱不被放行** → `email`；**其余任何情形**（含两种解码判定不一致、只在一种解码下有候选）→ `malformed`（失败闭合）。例：名字末尾单字节 `0xA0`（`utf8` 下是替换字符、名字合法；`latin1` 下是 U+00A0、名字末尾非法）；名字含字节 `E2 80 A8`（`utf8` 下是 U+2028，`.` 不匹配它，该段在 `utf8` 下不再是 trailer 块，`latin1` 下仍是）。

**`privacy-scan` 的扫描方式**：对**每种解码各自的原文**（按 `\n` 分行，**不是**归一化后的文本）逐行扫描；只把 `analyzeCoauthors` 判为 `ok` / `email` 的行（按 `line` 行号）换成 `rebuildCoauthorLine(该解码下的名字组)` 的结果，再交给 `scanText`；`email` 行先记 `coauthor-email`（字段 `message`、行号），重建后不会再附带 `email` 命中；`malformed` 行与其他一切行**整行照常扫描**。

- **禁止**对整行或整条信息做 `replace` / `replaceAll` / 正则替换去「抹掉邮箱」：那会把名字里以同一放行邮箱为后缀的另一个地址一并抹掉（例：邮箱字段 `noreply<at>openai.com`，名字写 `john.noreply<at>openai.com`）；重建法下名字原样进入 `scanText`，其中的邮箱、IPv4、token 样式、词表词照常命中。测试 69、76–78、80、84 拦截这类错误写法。
- 其余位置不变：trailer 块之外的共同作者行、`Reviewed-by: x <邮箱>` 等其他键行、正文句子里的邮箱、标签对象 / `mergetag` 说明文字里的 `Co-authored-by`（那不是提交信息）仍命中 `email`。
- 名字通道里的自由文本（电话、住址、他人姓名）`scanText` 本来就覆盖不到，与正文其他位置风险相同。
- 正文里放行的邮箱（例如 `<owner-gmail>` 的共同作者行）会随 `git log`、GitHub 提交页、生成的 changelog 扩散；元数据里本就公开，项目负责人已接受。

## 5. 测试（`tests/workflow-scripts.mjs`）

测试替身：放行表用 `owner` + `<at>` + `example.com` 与 `owner` + `<at>` + `fixture.invalid` 的运行时 sha256（r5.1 起新用例的 owner 夹具一律用后者，见 §2），工具邮箱以本地部分与域名运行时拼接，CLI 用 `--allow-hashes-file` 注入临时文件；邮箱样例运行时拼接（`fixture.invalid`、`example.com`、noreply 占位 `12345+fixture-bot`）；伪造的装甲块用 `crypto.randomBytes` 运行时生成；真实签名在临时目录里用 `ssh-keygen` / `gpg`（临时 `GNUPGHOME`）现签；对象用 `git hash-object -t <type> -w --literally` 构造；合并判定的 git 调用用 PATH 上的 git 包装器记录 / 模拟。

- A（trailer 解析）：1–11、5a–5c。
- B（合并提交）：12–18、18a–18c；18c 含改写自旧测试的三条。
- C（邮箱）：19–25、50，及 D5 的纯函数用例。
- D（`--commits` / `--shas-stdin`）：26–37、30a–30c、34a、35a，及 D5 的标签对象 / `mergetag` 用例（tagger 为放行哈希邮箱 → 通过；表外非 noreply 邮箱 → 拒绝；tagger 恰好一行）与标签链用例。
- E（信息文件与钩子）：38–41、38a、41a–41f，及 R1 的两条（已在另一个远程上的提交推到 origin 仍被扫描；推送目标为 URL 时只检查 tip）。
- F（工作流与文档一致性）：42–48（47 含 R2 的 `rev-list` 退出状态检查与 `--require-nonempty`、R5 的 `github.ref` 条件；48 含 R6 的 `.github/**`）。
- r5.1 改写的既有用例（8 项）：4、10（示例档位改 `high`）；8（`Co-authored-by` 不再禁止）；17（干净合并上的共同作者邮箱与冲突）；37（输出不泄露，含工具邮箱与 `coauthor-email`）；40（合并中的三项新检查，普通仓库与 worktree）；45（`docs/workflow-v2.md` 的新条款与示例围栏）；50（工具表邮箱作 author 在空放行表下通过）。
- G（r5.1 新增，51–86，共 36 项）：51–54 工具表与四类放行；55–57 共同作者行解析、邮箱规则与畸形；58 各工具的 trailer 落点（Codex 按实测填入，Claude Code「待实测填入」但期望已固定）；59–63 来源证据、冲突与决策表 T1–T15、合并提交；64 档位白名单与文档示例（`r5-history` 成对标记）；65–68 信息文件、真实提交与 pre-push；69–72 `privacy-scan` 正文放宽与不放宽的位置；73–75 实现约束、源码自扫与工具表一致性；76–80、84 名字夹带与重建方式；81 已知冲突；82、83、85 两个脚本一致、输入编码与解码分叉；86 notice。总数 149。
- **R7**：30c 的真实签名用例在 `CI=true` 时缺 `gpg` / `ssh-keygen` 判失败；本地缺工具时明确输出 `SKIPPED  30c …`，不计入通过数。合成签名的 30c 用例不依赖外部工具，始终运行。

## 6. 与 workflow v2 的关系

- 分类：重大改动中的**权限或安全边界变更**（隐私检查对提交元数据放宽；新增 `pull_request_target` 工作流）。合并须项目负责人明确批准，且批准时需知道本改动新增了一个 `pull_request_target` 工作流（§9）。
- 角色分离：实现为 Cursor Agent（`claude-opus-5-5`）；设计评审为 claude.ai 网页（Opus 5.5），评审者没有读到仓库文件；独立代码评审另行进行，工具与档位如实记录（由项目负责人决定）。档位按提交如实记录，追加的提交 C、D 为 `high`（D9）；r3 时期的档位记录保留如下：
<!-- r5-history:begin -->
  - 提交 A、B 的实现档位为 medium（trailer 为 `Code-Tool: cursor-agent claude-opus-5-5 medium`，历史事实，不改写）；设计评审 r1–r3 为 Opus 5.5 Medium，r4 起为 High。
<!-- r5-history:end -->
- **先红后绿（R3）**：A（红，r3 测试）、B（绿，r3 实现）、C（红，r5.1 测试）、D（绿，r5.1 实现与文档）。提交 A 只含测试——新测试**和按新语义改写的旧测试**都在提交 A 里，对旧脚本为红；提交 B 实现脚本、工作流、钩子、文档与本 spec。提交 C 只改 `tests/workflow-scripts.mjs`（改写 8 项、新增 51–86），对 B 的脚本为红；提交 D 实现共享模块、两个脚本与两份文档。旧测试 → 新测试对应关系：

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
| r3 的 8（`Co-authored-by` 一律 → `co-authored-by-forbidden`） | 8（块内合法行通过）、55–57（解析与畸形）、59–63（来源证据与冲突） |
| r3 的 17（干净合并上 `Co-authored-by` 被拒） | 17（noreply 共同作者通过；他人邮箱 → `coauthor-email-not-allowed`；声明与工具共同作者冲突） |
| r3 的 40（合并中只查邮箱、`co-authored-by-forbidden` 与花费字样） | 40（合并中查三项新检查，普通仓库与 worktree） |
| r3 的 37（输出不泄露） | 37（另含工具邮箱片段与 `coauthor-email` 命中） |

- 合并方式：`docs/workflow-v2.md` 规定只用 “Create a merge commit”；不用 squash；“Rebase and merge” 不推荐。关闭 squash 属于仓库设置（§10）。

## 7. 迁移

- 引入本规范的 PR，其提交须同时满足新规则（noreply 身份 + `Code-Tool`；用 `commit-tree` 重建时写 `git-only` 并补 `Code-Source`）。该 PR 上 base 版放行表为空，只能用 noreply 身份；**不要点 “Update branch”**，也不要用项目负责人自己的身份在本地合并 `main`（测试 50 固定）。
- **「新旧共存期」**：这里的「旧脚本」指 **`main` 上现行的 v2 脚本**（r3 从未合入 `main`）。`448216b` 上的 `scripts/agent-trailer.mjs` 与 `docs/workflow-v2.md` 都没有任何共同作者规则；`scripts/privacy-scan.mjs` 只有 `--stdin`、`--diff`、全树三种模式，`.github/workflows/privacy.yml` 也不扫提交信息。所以合入前，提交信息里的 owner 共同作者行既不会被 `agent-trailer` 拒，也不会被 `privacy-scan` 拦。对 `main` 全部历史与 PR #3–#5 的提交清点共同作者行，结果为 0 行。
- 合入后，PR #3、#4、#5 的下一次 CI 运行会用新脚本，对只有 `Agent:` 的旧提交报 `missing-code-tool`（不会出现 `coauthor-*` 或冲突类错误），属于预期过渡状态。决定（D4）：新规则评审通过后重建这三个 PR 的旧提交、补新 trailer、`--force-with-lease` 推送，不合并；每个 PR 重建前先回报新旧 SHA 对照与单独的扫描结果；扫描与推送分开执行，任何命中先停下报告。不做按日期 / SHA 的宽限。
- **Cursor VM 的 hook（限定为「合入之后」）**：云 agent 注入的共同作者行是项目负责人本人（owner 哈希类），author 是工具邮箱。**合入之后**，这种提交只要带 `Code-Tool` 就通过，不再需要为绕开注入行而在 box 上重建；box 重建只服务 D4、`Agent:` 与 SHA 对照。**在本 PR 内**，提交仍不要带 owner 共同作者行：PR 的 base 放行表为空（`agent-trailer` 以 base 版放行表判 owner 邮箱），不是 v2 有规则；若注入了，由 box 用 `commit-tree` 重建时去掉。
- 给工具的提示词（实现与派活时必须包含）：提交信息的**最后一段**必须是 trailer 块（每行 `Key: value`，不夹普通文字、空行、链接、会话地址或 “Generated with …” 之类的行），其中写 `Code-Tool: <tool> <model> <effort>`；提交后运行 `git log -1 --format=%B` 核对最后一段；不要伪造其他工具的邮箱。Claude Code：自动加的 `Co-Authored-By` 必须与 `Code-Tool` 同在最后一段，不允许为通过检查而删掉 `Code-Tool`；用 `attribution.commit` 自带完整 trailer 仅限项目级配置且同时固定模型，每次提交后核对。Codex：exec 模式不写任何来源信息，必须显式写 `Code-Tool: codex <model> <effort>`。Cursor：注入行默认保留，`Code-Tool` 与它位于同一最后一段；落到新段时提交后 `git commit --amend` 并核对。
- 工具归属关闭 + 提交后核对：做纯 git 操作或代提交人工代码时关闭归属（§1.7 已知冲突场景的缓解）。
- 由工具自动注入的 trailer 推导来源：已部分采纳——邮箱匹配工具表的共同作者行算来源证据（§1.7 的 C）；由它推导 `Code-Source` 的 model / effort 仍不做。

## 8. 实现记录

- 提交 A（只含测试）对旧脚本：113 项里 91 项失败（全部新测试与改写的旧测试），22 项通过（原有的 privacy / spec / go.sum 用例 21 项，及纯回归测试 36）。
- 提交 B 后全部通过；完整日志随 PR 说明提供。
- 提交 C（r5.1 测试）对提交 B 的脚本：149 项里 40 项失败、109 项通过；只改字面量的 4、10 为绿，50 因含新断言为红；新增用例里 70（非共同作者位置照常命中，回归）与 74（源码与文档自扫）对 B 也为绿。
- 提交 D 后 149 项全部通过。

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

r5.1 增补：

1. **署名伪装**：任何人都能把 author 或共同作者写成工具邮箱；`Code-Tool` 与共同作者都是自述文本，冲突检查只发现不一致，不证明真实。这是署名伪装风险，不是泄露风险，接受。
2. **工具表与 base 版放行表不对称**：PR 里修改 `TOOL_IDENTITIES` 会在当次 CI 立即生效，只有 `policy-gate` 发出红灯信号；放行 JSON 有 base 版兜底。与「PR 改脚本」同级，可以接受；评审时按 D7 要求 diff 附核实来源。
3. **名字通道的自由文本**：`scanText` 覆盖不到电话、住址、他人姓名，与正文其他位置风险相同。Cursor 注入的名字来自账户显示名，若 `PRIVACY_WORDLIST` 里有项目负责人的实名会被误报（由词表维护者处理，不放宽规则）。
4. **豁免的实际形态**：`policy-gate` 与 CI 变红不阻断合并，但 `pre-push` 会拦，需要 `--no-verify`；今后启用 `--range-all` 时同一批提交会再次报出。豁免必须记录 sha 清单（谁、何时、哪几个 sha、原因），不通过改脚本绕过；`source-evidence-conflict` 的豁免只作为 OQ2 的最后手段。
5. **Gmail 扩散面变大**：出现在正文共同作者行后，会随 `git log`、GitHub 提交页、生成的 changelog 扩散；元数据里本就公开，项目负责人已接受。
6. **Claude Code 的 trailer 落段未实测**（登录过期）：补测列为后续项（含 `sessionUrl`），补测前靠 §7 的提示词要求兜底；补测完成前，本仓库里 Claude Code 产生的每个提交都要做提交后核对（最后一段每行都是 `Key: value`、`Code-Tool` 存在且与实际模型 / 档位一致）。**反向激励**：落点 B（共同作者另起一段或带附加行）下，删掉 `Code-Tool`、只留工具共同作者反而能通过；不改通过 / 失败结果，加 notice（§1.7 第 6 步），靠提示词与评审兜底。
7. **Codex 的归属文本在 exec 模式下不出现**：`noreply<at>openai.com` 作为证据的实际用处有限，用 Codex 的提交基本要靠显式 `Code-Tool`；该邮箱保持「部分核实」。
8. **`privacy-scan` 正文放宽一处**（§4.4）：是 r5.1 唯一的检查面放宽；伪造 trailer 块、折行、大小写 / 空白 / 字形变体、名字夹带、整条信息替换、跨行引用由测试 57、76–80、84、85 固定。
9. **共同作者在合法工作流下的误伤**：见 §1.7「已知冲突场景」。
10. **实现约束**：`rebuildCoauthorLine` 只做模板拼接、`privacy-scan` 对原文按行号处理、候选行取两种解码并集、块边界分叉失败闭合、`agent-trailer` 非共同作者检查固定 `utf8`——代码评审时专门核对。

## 10. 决定与待拍板的仓库设置

**已定**

- **D1**：他人邮箱须先经项目负责人明确批准（并经本人同意）才能进放行哈希表。
- **D2**：允许在 trailer、评审记录和 PR 说明里写模型与档位；`docs/workflow-v2.md` 的对应禁令同步修改。
- **D3**：加 `Code-Source:`；非合并的 `git-only` 提交必须带它。
- **D4**：新规则评审通过后重建 PR #3、#4、#5 的旧提交补新 trailer，`--force-with-lease` 推送，不合并；属于合入后的后续步骤。
- **D5**：附注标签（含 `mergetag` 内嵌标签）的 tagger 邮箱与 author / committer 同一套规则：GitHub noreply、其哈希在 `scripts/allowed-author-hashes.json` 里，或（r5.1 起）工具表邮箱。每个标签恰好一行 tagger，缺少或重复判为畸形（§2）。他人邮箱进放行表仍须按 D1 先经项目负责人明确批准。
- **D6**：取消禁止 `Co-authored-by`；`Code-Tool` / `Code-Source` / 邮箱匹配工具表的共同作者行任一能明确识别工具即合格（§1.7）。
- **D7**：工具表增删须项目负责人批准并附核实来源（§2.3）。
- **D8**：声明与共同作者证据冲突 → 硬错误 `source-evidence-conflict`，不静默裁决。
- **D9**：effort = `high`：今后的示例与新提交写 `high`；已有提交的档位按当时实际情况保留（§6）。
- **OQ1**：author / committer 邮箱不算工具证据（它们表示谁执行了提交，与 `git-only` 区分代码来源的语义冲突；committer 在 rebase / 重建时还会变）。
- **OQ2**：多工具提交拆开；拆不开就只写主工具，删掉次要工具的共同作者行，并在正文说明；不增设 `Code-Also:`。
- **OQ3**：`Made-with: Cursor` 不算证据。
- **OQ4**：`267193182+codex<at>users.noreply.github.com` 不入工具表。

**未决 / 后续**：Claude Code trailer 落点实测（含 `sessionUrl`），以及 Codex 交互 / App / Cloud、Cursor IDE / CLI 的补测；Codex 的 `commit_attribution_enabled` 配置位置未核实；`noreply<at>openai.com` 是否降为候选待补测后再议（走 D7）。

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

### 11.1 r5.1 的实现取舍（提交 C、D）

- `checkAgentTrailer` 保留原签名、仍返回错误码数组，内部调用新增的 `analyzeAgentTrailer`（另返回 notices）；`message` 同时接受字符串与原始字节。
- 合并进行中（`--message-file` 且存在 `MERGE_HEAD`）不输出 notice：合并中只做部分检查，是否“只靠共同作者合格”交给 pre-push 与 CI 判断。
- `privacy-scan --message-file` 也按 utf8 与 latin1 两种解码扫描（与对象扫描一致）；信息文件按字节清理（`cleanMessageBytes`）。
- `privacy-scan` 的 `message` 字段行号改为提交信息内的行号（§4.1 第 7 步、§4.4），其他字段仍是对象内行号。
- 共享模块另导出 `isAllowedCoauthorEmail`（= author 规则），供文档与后续使用；两个脚本只通过 `analyzeCoauthors` 判定共同作者行。
- 测试 57 的「缺邮箱」畸形行没有邮箱可命中，断言 `agent-trailer` 报错、`privacy-scan` 0 命中（与测试 82 的单向例外一致）。
- 测试 71：严格解析下 `privacy-scan: allow` 写在行末会让整行畸形（照常扫描、命中 `email`），所以「豁免标记不生效」用写进名字组的标记来验证。
- 测试 64 除围栏示例外，另要求 `r5-history` 标记之外不出现同时写实现所用模型名与旧档位的行（让该用例在旧文档上可被验证为红）。
