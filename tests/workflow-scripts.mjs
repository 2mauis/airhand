// 工作流脚本单元测试：privacy-scan 规则、agent-trailer、spec-requires-tests、署名规范 v3（纯函数 + 临时 git 仓库，不需要网络）
// 移植自 2mauis 的 passkey-demo 项目（workflow v2，MIT，Copyright (c) 2026 2mauis）：privacy 用例沿用原文，
// spec 用例按 airhand 的测试形态改写；删除了与 Cloudflare 部署、管理 token、发布快照相关的用例。
// 署名规范 v3 的用例见 docs/specs/commit-signature-v3.md §5（编号与 spec 一致）。
// 夹具身份一律在运行时拼接（noreply 占位、example.com、fixture.invalid），源码里不出现完整的邮箱样式。
// 用法：node tests/workflow-scripts.mjs（CI=true 时缺 gpg / ssh-keygen 判失败；本地缺工具时输出 SKIPPED）
import { scanText, shouldSkip } from '../scripts/privacy-scan.mjs';
import { mkdtempSync, writeFileSync, rmSync, readFileSync, mkdirSync, chmodSync, existsSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkAgentTrailer } from '../scripts/agent-trailer.mjs';
import { checkSpecTests, addedInlineRustTests } from '../scripts/check-spec-tests.mjs';
const policy = await import('../scripts/commit-email-policy.mjs').catch(() => ({}));
const R = []; const check = (n, ok) => { R.push(ok); console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}`); };
const t = (n, fn) => { let ok = false; try { ok = fn() === true; } catch { ok = false; } check(n, ok); };
const hit = (t) => scanText(t, 'x').map((h) => h.split(' ')[1]);
check('privacy：普通邮箱命中', hit('mail a.b@' + 'gmail.com').includes('email'));
check('privacy：noreply / example.com 邮箱放行', hit('x@users.noreply.github.com a@example.com').length === 0);
check('privacy：公网 IPv4 命中', hit('ip 8.8.' + '4.4').includes('ipv4'));
check('privacy：文档保留段 IP 放行', hit('192.0.2.1 198.51.100.7 203.0.113.9 127.0.0.1').length === 0);
check('privacy：真实 workers.dev 子域命中，example 放行', hit('https://app.someone' + '.workers.dev').includes('workers-dev-subdomain') && hit('https://app.example.workers.dev').length === 0);
check('privacy：32 位十六进制 id 命中，全 0 放行', hit('id = "' + 'ab'.repeat(16) + '"').includes('kv-or-account-id') && hit('0'.repeat(32)).length === 0);
check('privacy：Turnstile 生产 sitekey 命中', hit('0x4' + 'A'.repeat(20)).includes('turnstile-sitekey'));
check('privacy：GitHub token 样式命中', hit('ghp_' + 'a1B2'.repeat(9)).includes('github-token'));
check('privacy：tailnet 主机名命中；JS 属性 flags.local 不误报', hit('box.tail1234' + '.ts.net').includes('tailnet-or-lan-host') && hit('flags.local ? 1 : 2').length === 0);
check('privacy：显式豁免注释生效', hit('8.8.' + '4.4 // privacy-scan: allow').length === 0);

// ---------- 署名规范 v3：公共夹具 ----------
const AT = '@';
const FIX = 'fixture' + '.invalid', EXC = 'example' + '.com', NRD = 'users.noreply.github.com';
const NR = '12345+fixture-bot' + AT + NRD;
const OWNER = 'owner' + AT + EXC, SOMEONE = 'someone' + AT + EXC, STRANGER = 'someone' + AT + FIX;
const GH_WEB = 'noreply' + AT + 'github.com';
const JACK_HASH = '8296231a7b969061e4a162230595f1238eead503866e86a022ce6f726cd70e17';
const sha256 = (s) => createHash('sha256').update(s, 'utf8').digest('hex');
const HASHES = new Set([sha256(OWNER)]);
const M = (...lines) => lines.join('\n') + '\n';
const ct = (message, extra = {}) => checkAgentTrailer({ message, authorEmail: NR, committerEmail: NR, allowedHashes: HASHES, ...extra });
const ok = (message, extra) => ct(message, extra).length === 0;
const has = (message, code, extra) => ct(message, extra).includes(code);
const ROOT = fileURLToPath(new URL('..', import.meta.url));
const TRAILER_CLI = join(ROOT, 'scripts/agent-trailer.mjs'), SCAN_CLI = join(ROOT, 'scripts/privacy-scan.mjs');
const tmpRoot = mkdtempSync(join(tmpdir(), 'wf-v3-'));
const allowFile = join(tmpRoot, 'allow.json');
writeFileSync(allowFile, JSON.stringify({ version: 1, sha256_lowercase_email: [{ sha256: sha256(OWNER), label: 'owner' }] }));
const emptyAllow = join(tmpRoot, 'empty-allow.json');
writeFileSync(emptyAllow, JSON.stringify({ version: 1, sha256_lowercase_email: [] }));
const gitEnv = (extra = {}) => ({
  ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', HOME: tmpRoot, GIT_EDITOR: 'true', GIT_MERGE_AUTOEDIT: 'no',
  GIT_AUTHOR_NAME: 'Fixture Bot', GIT_AUTHOR_EMAIL: NR, GIT_COMMITTER_NAME: 'Fixture Bot', GIT_COMMITTER_EMAIL: NR,
  GIT_AUTHOR_DATE: '1700000000 +0000', GIT_COMMITTER_DATE: '1700000000 +0000', ...extra,
});
const OUTS = [];
const runNode = (cli, args, { cwd, input, env } = {}) => {
  const r = spawnSync(process.execPath, [cli, ...args], { cwd, env: gitEnv(env), input, encoding: 'utf8' });
  const res = { status: r.status, out: (r.stdout || '') + (r.stderr || ''), err: r.stderr || '', stdout: r.stdout || '' };
  OUTS.push(res.out); return res;
};
const trailer = (cwd, args, o = {}) => runNode(TRAILER_CLI, [...args, '--allow-hashes-file', o.allow ?? allowFile], { cwd, ...o });
const pscan = (cwd, args, o = {}) => runNode(SCAN_CLI, [...args, '--allow-hashes-file', o.allow ?? allowFile], { cwd, ...o });
function repo(name, { bare = false } = {}) {
  const dir = join(tmpRoot, name); mkdirSync(dir, { recursive: true });
  const g = (args, o = {}) => {
    const r = spawnSync('git', args, { cwd: dir, env: gitEnv(o.env), encoding: 'utf8', input: o.input });
    if (o.full) return r;
    if (r.status !== 0) throw new Error(`git ${args[0]} failed`);
    return r.stdout.trim();
  };
  g(bare ? ['init', '-q', '--bare'] : ['init', '-q', '-b', 'main']);
  const r = {
    dir, g,
    commit(msg, file = 'f.txt', content = String(Math.random()), env) {
      writeFileSync(join(dir, file), content); g(['add', file]); g(['commit', '-q', '--no-verify', '-m', msg], { env }); return g(['rev-parse', 'HEAD']);
    },
    blob: (content) => g(['hash-object', '-w', '--stdin'], { input: content }),
    tree(from, files = {}) {
      const entries = from ? g(['ls-tree', from]).split('\n').filter(Boolean).filter((l) => !(l.split('\t')[1] in files)) : [];
      for (const [name, content] of Object.entries(files)) entries.push(`100644 blob ${r.blob(content)}\t${name}`);
      return g(['mktree'], { input: entries.join('\n') + (entries.length ? '\n' : '') });
    },
    commitTree: (tree, parents, msg, env) => g(['commit-tree', tree, ...parents.flatMap((p) => ['-p', p]), '-m', msg], { env }),
    raw: (type, text) => g(['hash-object', '-t', type, '-w', '--literally', '--stdin'], { input: text }),
  };
  return r;
}
const sha12 = (s) => s.slice(0, 12);
const hitsOf = (r) => r.err.split('\n').map((l) => /^(?:[0-9a-f]{12}|HEAD):([a-z0-9-]+)(?::\d+)? ([a-z0-9-]+)$/.exec(l)).filter(Boolean).map((m) => ({ field: m[1], rule: m[2] }));
const hasHit = (r, rule, field) => hitsOf(r).some((h) => h.rule === rule && (!field || h.field === field));
const scanned = (r) => /已扫描 \d+ 个对象/.test(r.stdout);
const scanClean = (r) => r.status === 0 && scanned(r) && hitsOf(r).length === 0;

// ---------- A. trailer 解析 ----------
t('1 缺 Code-Tool → missing-code-tool', () => has(M('feat: x', '', 'body'), 'missing-code-tool'));
t('2 两行 Code-Tool（含 code-tool:、行首空白、`Code-Tool :` 变体）→ duplicate-code-tool', () => [
  M('feat: x', '', 'Code-Tool: human', 'Code-Tool: human'),
  M('feat: x', '', 'Code-Tool: human', 'code-tool: human'),
  M('feat: x', '', '  Code-Tool: human', '', 'Code-Tool: human'),
  M('feat: x', '', 'Code-Tool : human', '', 'Code-Tool: human'),
].every((m) => has(m, 'duplicate-code-tool')));
t('3 未知 tool、缺字段、非法档位、model 含大写 / 下划线 → invalid-code-tool', () => ['gemini x high', 'codex gpt-x', 'codex gpt-x turbo', 'codex GPT-x high', 'codex gpt_x high', 'codex', '?']
  .every((v) => has(M('feat: x', '', `Code-Tool: ${v}`), 'invalid-code-tool')));
t('4 合法 Code-Tool 取值与 git-only + 合法 Code-Source 通过', () =>
  ['cursor-agent claude-opus-5-5 medium', 'codex gpt-x high', 'claude-code claude-opus-5-5 max', 'codex unknown ultra', 'codex unknown unknown', 'human']
    .every((v) => ok(M('feat: x', '', 'body', '', `Code-Tool: ${v}`)))
  && ['cursor-agent claude-opus-5-5 medium', 'cursor-agent claude-opus-5-5 unknown', 'human', 'unknown']
    .every((s) => ok(M('chore: replay', '', 'Code-Tool: git-only', `Code-Source: ${s}`))));
t('5 git-only / human 带多余字段 → invalid-code-tool', () => has(M('x', '', 'Code-Tool: git-only extra'), 'invalid-code-tool') && has(M('x', '', 'Code-Tool: human x'), 'invalid-code-tool'));
t('5a 非合并 git-only 缺 Code-Source → missing-code-source', () => has(M('x', '', 'Code-Tool: git-only'), 'missing-code-source'));
t('5a Code-Source 写 git-only、缺字段、非法档位 → invalid-code-source', () => ['git-only', 'cursor-agent claude-opus-5-5', 'codex gpt-x turbo', 'unknown unknown']
  .every((s) => has(M('x', '', 'Code-Tool: git-only', `Code-Source: ${s}`), 'invalid-code-source')));
t('5a 两行 Code-Source（含行首空白小写变体）→ duplicate-code-source', () => has(M('x', '', '  code-source: human', '', 'Code-Tool: git-only', 'Code-Source: unknown'), 'duplicate-code-source')
  && has(M('x', '', 'Code-Tool: git-only', 'Code-Source: human', 'Code-Source: unknown'), 'duplicate-code-source'));
t('5a Code-Source 在 trailer 块外 → code-source-outside-trailer-block', () => has(M('x', '', 'Code-Source: human', '', 'Code-Tool: git-only'), 'code-source-outside-trailer-block'));
t('5a Code-Source 与 codex… / human 同时出现，或没有 Code-Tool → code-source-not-allowed', () =>
  has(M('x', '', 'Code-Tool: codex gpt-x high', 'Code-Source: human'), 'code-source-not-allowed')
  && has(M('x', '', 'Code-Tool: human', 'Code-Source: human'), 'code-source-not-allowed')
  && has(M('x', '', 'Code-Source: human'), 'code-source-not-allowed'));
const CLEAN = { isMerge: true, isCleanMerge: true };
t('5b 干净合并不写 Code-Tool / Code-Source → 通过；rebase 保留的原信息 → 通过', () => ok("Merge branch 'feature'\n", CLEAN) && ok(M('feat: x', '', 'Code-Tool: codex gpt-x high')));
t('5c 干净合并写 git-only 不带 Code-Source → 通过；带合法 Code-Source → 通过', () => ok(M('Merge', '', 'Code-Tool: git-only'), CLEAN) && ok(M('Merge', '', 'Code-Tool: git-only', 'Code-Source: human'), CLEAN));
t('5c 干净合并上写了就照常检查：非法 / 重复 / 块外 / not-allowed / 花费 / Agent', () =>
  has(M('Merge', '', 'Code-Tool: codex'), 'invalid-code-tool', CLEAN)
  && has(M('Merge', '', 'Code-Tool: human', 'Code-Tool: human'), 'duplicate-code-tool', CLEAN)
  && has(M('Merge', '', 'Code-Tool: human', '', 'more text'), 'code-tool-outside-trailer-block', CLEAN)
  && has(M('Merge', '', 'Code-Tool: human', 'Code-Source: human'), 'code-source-not-allowed', CLEAN)
  && has(M('Merge', '', 'token usage: 12'), 'ai-cost-or-usage-in-message', CLEAN)
  && has(M('Merge', '', 'Agent: Bad_Name'), 'invalid-agent-trailer', CLEAN)
  && ct(M('Merge', '', 'body'), CLEAN).length === 0);
t('6 Code-Tool 在正文中间、单行信息、末段混有普通文字 → code-tool-outside-trailer-block', () =>
  has(M('feat: x', '', 'Code-Tool: human', '', 'more body'), 'code-tool-outside-trailer-block')
  && has('Code-Tool: human\n', 'code-tool-outside-trailer-block')
  && has(M('feat: x', '', 'see notes', 'Code-Tool: human'), 'code-tool-outside-trailer-block'));
t('7 CRLF：合法信息通过；CRLF 的重复行仍识别', () => ok('feat: x\r\n\r\nbody\r\n\r\nCode-Tool: human\r\n') && has('feat: x\r\n\r\nCode-Tool: human\r\nCode-Tool: human\r\n', 'duplicate-code-tool'));
t('8 Co-authored-by（大小写、行首空白、noreply 邮箱、任意位置）→ co-authored-by-forbidden', () => [
  M('feat: x', '', 'Code-Tool: human', `Co-authored-by: Fixture <${NR}>`),
  M('feat: x', '', `co-authored-by: Fixture <${NR}>`, '', 'Code-Tool: human'),
  M('feat: x', '', `  Co-Authored-By: Fixture <${NR}>`, '', 'Code-Tool: human'),
].every((m) => has(m, 'co-authored-by-forbidden')) && has(M('Merge', '', `Co-authored-by: F <${NR}>`), 'co-authored-by-forbidden', CLEAN));
t('9 Agent 可选、与任意 author 名组合通过；两行或值含大写 → invalid-agent-trailer', () =>
  ct(M('feat: x', '', 'Code-Tool: human', 'Agent: grok-code-assistant'), { authorName: 'Anyone At All' }).length === 0
  && has(M('feat: x', '', 'Code-Tool: human', 'Agent: a', 'Agent: b'), 'invalid-agent-trailer')
  && has(M('feat: x', '', 'Code-Tool: human', 'Agent: Grok'), 'invalid-agent-trailer'));
t('10 合法 Code-Tool 行对 scanText / cost 0 命中；默认 git revert 信息只缺 Code-Tool', () => {
  const lines = ['Code-Tool: cursor-agent claude-opus-5-5 medium', 'Code-Tool: codex gpt-5.5-codex high', 'Code-Tool: codex unknown unknown', 'Code-Source: cursor-agent claude-opus-5-5 unknown'];
  const revert = M('Revert "feat: x"', '', 'This reverts commit ' + 'a1b2c3d4e5'.repeat(4) + '.');
  return lines.every((l) => hit(l).length === 0) && lines.slice(0, 3).every((l) => ok(M('feat: x', '', l)))
    && hit(revert).length === 0 && JSON.stringify(ct(revert)) === '["missing-code-tool"]';
});
t('11 花费 / token 字样仍拒绝（且是唯一错误）', () => JSON.stringify(ct(M('feat: x', '', 'token usage: 5', '', 'Code-Tool: human'))) === '["ai-cost-or-usage-in-message"]');

// ---------- C. 邮箱 ----------
t('19 author / committer 为放行哈希邮箱合法；表外邮箱 → *-email-not-allowed（改写自旧“非 noreply 邮箱失败”“AI committer 普通邮箱失败”）', () => {
  const m = M('feat: x', '', 'Code-Tool: human');
  return ct(m, { authorEmail: OWNER, committerEmail: OWNER }).length === 0
    && ct(m, { authorEmail: SOMEONE }).includes('author-email-not-allowed')
    && ct(m, { committerEmail: SOMEONE }).includes('committer-email-not-allowed');
});
t('19 Code-Tool: human 也没有“任意邮箱”例外（改写自旧“人工提交允许本人普通邮箱”）', () => ct(M('fix: manual', '', 'Code-Tool: human'), { authorEmail: SOMEONE, committerEmail: SOMEONE }).includes('author-email-not-allowed'));
t('20 GitHub 网页 committer 地址：committer 合法、author 非法（改写自旧“AI 提交允许 GitHub 服务 committer”）', () =>
  ok(M('x', '', 'Code-Tool: human'), { committerEmail: GH_WEB }) && has(M('x', '', 'Code-Tool: human'), 'author-email-not-allowed', { authorEmail: GH_WEB }));
t('21 noreply 夹带 / 畸形 → 拒绝', () => [STRANGER + AT + NRD, 'a b' + AT + NRD, '-bad' + AT + NRD, 'a_b' + AT + NRD, 'a..' + AT + NRD, 'x' + AT + 'evil.' + NRD, 'x' + AT + NRD + '.' + EXC]
  .every((e) => has(M('x', '', 'Code-Tool: human'), 'author-email-not-allowed', { authorEmail: e })));
t('22 noreply 形态：<id>+<name>、无 id、[bot] → 通过（改写自旧“人工提交仍可自选 noreply”）', () => ['123+name' + AT + NRD, 'name' + AT + NRD, '123+name[bot]' + AT + NRD]
  .every((e) => ok(M('x', '', 'Code-Tool: human'), { authorEmail: e, committerEmail: e })));
t('23 大小写与首尾空白归一化后命中哈希；点号 / +tag 变体不命中', () =>
  ok(M('x', '', 'Code-Tool: human'), { authorEmail: '  Owner' + AT + 'Example.COM ' })
  && has(M('x', '', 'Code-Tool: human'), 'author-email-not-allowed', { authorEmail: 'ow.ner' + AT + EXC })
  && has(M('x', '', 'Code-Tool: human'), 'author-email-not-allowed', { authorEmail: 'owner+tag' + AT + EXC }));
t('24 空放行表时放行邮箱被拒', () => has(M('x', '', 'Code-Tool: human'), 'author-email-not-allowed', { authorEmail: OWNER, allowedHashes: new Set() }));
t('D5 tagger 与 author 同规则：noreply 或放行哈希通过，其他拒绝', () => policy.isAllowedTaggerEmail(OWNER, HASHES) === true && policy.isAllowedTaggerEmail(NR, new Set()) === true
  && policy.isAllowedTaggerEmail(STRANGER, HASHES) === false && policy.isAllowedTaggerEmail(GH_WEB, HASHES) === false);
t('25 默认放行表：可解析、64 位小写十六进制、无重复、label 无 @、含 owner 条目、不含测试替身哈希、自身 0 命中', () => {
  const file = join(ROOT, 'scripts/allowed-author-hashes.json');
  const j = JSON.parse(readFileSync(file, 'utf8')); const list = j.sha256_lowercase_email.map((e) => e.sha256);
  return j.version === 1 && list.every((h) => /^[0-9a-f]{64}$/.test(h)) && new Set(list).size === list.length
    && j.sha256_lowercase_email.every((e) => typeof e.label === 'string' && !e.label.includes('@'))
    && list.includes(JACK_HASH) && !list.includes(sha256(OWNER))
    && runNode(SCAN_CLI, ['--files', file], { cwd: ROOT }).status === 0
    && [...policy.loadAllowedHashes()].includes(JACK_HASH);
});
t('50 本 PR 的提交（noreply + Code-Tool）在空放行表下通过；owner 邮箱在空表下被拒', () =>
  ct(M('feat: v3', '', 'Code-Tool: cursor-agent claude-opus-5-5 medium'), { allowedHashes: new Set() }).length === 0
  && has(M('feat: v3', '', 'Code-Tool: cursor-agent claude-opus-5-5 medium'), 'author-email-not-allowed', { authorEmail: OWNER, allowedHashes: new Set() }));

// ---------- B. 合并提交（临时仓库） ----------
const realGit = spawnSync('sh', ['-c', 'command -v git'], { encoding: 'utf8' }).stdout.trim();
const wrapDir = join(tmpRoot, 'gitwrap'); mkdirSync(wrapDir);
writeFileSync(join(wrapDir, 'git'), [
  '#!/bin/sh',
  'printf \'%s\\n\' "$*" >> "$GIT_WRAP_LOG"',
  'if [ "$1" = merge-tree ] && [ -n "${FAKE_MT:-}" ]; then',
  '  case "$FAKE_MT" in bad1) echo not-a-tree; exit 1 ;; code3) exit 3 ;; esac',
  'fi',
  `exec '${realGit}' "$@"`, '',
].join('\n'));
chmodSync(join(wrapDir, 'git'), 0o755);
const wrapEnv = (log, extra = {}) => ({ PATH: `${wrapDir}:${process.env.PATH}`, GIT_WRAP_LOG: log, ...extra });
const mr = repo('merges');
const base = mr.commit(M('base', '', 'Code-Tool: human'), 'a.txt', 'a\n');
mr.g(['checkout', '-qb', 'feature']); const f1 = mr.commit(M('feature', '', 'Code-Tool: human'), 'f.txt', 'f\n');
mr.g(['checkout', '-q', 'main']); const m1 = mr.commit(M('main work', '', 'Code-Tool: human'), 'm.txt', 'm\n');
mr.g(['merge', '-q', '--no-ff', 'feature', '-m', "Merge branch 'feature'"]); const clean = mr.g(['rev-parse', 'HEAD']);
const evil = mr.commitTree(mr.tree(m1, { 'extra.txt': 'evil\n' }), [base, m1], 'Merge with extra file');
const evilTrailer = mr.commitTree(mr.tree(m1, { 'extra.txt': 'evil\n' }), [base, m1], M('Merge with extra file', '', 'Code-Tool: human'));
const degenerateClean = mr.commitTree(mr.g(['rev-parse', `${m1}^{tree}`]), [base, m1], 'Merge base into main');
const c1 = mr.commitTree(mr.tree(base, { 'a.txt': 'x\n' }), [base], M('c1', '', 'Code-Tool: human'));
const c2 = mr.commitTree(mr.tree(base, { 'a.txt': 'y\n' }), [base], M('c2', '', 'Code-Tool: human'));
const resolved = mr.tree(c1, { 'a.txt': 'xy\n' });
const conflictMerge = (msg) => mr.commitTree(resolved, [c1, c2], msg);
const conflict0 = conflictMerge("Merge branch 'c2'");
const oct = (msg) => mr.commitTree(mr.g(['rev-parse', `${clean}^{tree}`]), [f1, m1, c1], msg);
const oct0 = oct('Octopus merge');
const orphan = mr.commitTree(mr.tree(null, { 'o.txt': 'o\n' }), [], M('orphan root', '', 'Code-Tool: human'));
const orphanBad = mr.commitTree(mr.tree(null, { 'o.txt': 'ob\n' }), [], 'orphan root without trailer');
const unrel = (msg, root = orphan) => mr.commitTree(mr.tree(m1, { 'o.txt': root === orphan ? 'o\n' : 'ob\n' }), [m1, root], msg);
const unrel0 = unrel('Merge unrelated');
const ts = (shas, o = {}) => trailer(mr.dir, ['--shas-stdin', ...(o.args || [])], { input: shas.join('\n') + '\n', env: o.env });
t('12 干净的 git merge：不要求 Code-Tool，通过', () => { const r = ts([clean]); return r.status === 0 && /1 个提交通过/.test(r.stdout); });
t('13 退化合并 + 夹带文件（树 ≠ merge-tree）→ missing-code-tool', () => { const r = ts([evil]); return r.status === 1 && r.err.includes(`${sha12(evil)} missing-code-tool`); });
t('14 冲突解决型合并：要求 Code-Tool；git-only（即使带 Code-Source）→ git-only-not-allowed；human 通过', () => {
  const a = ts([conflict0]), b = ts([conflictMerge(M('Merge', '', 'Code-Tool: git-only', 'Code-Source: human'))]), c = ts([conflictMerge(M('Merge', '', 'Code-Tool: human'))]);
  return a.status === 1 && a.err.includes('missing-code-tool') && b.status === 1 && b.err.includes('git-only-not-allowed') && c.status === 0;
});
t('15 三个父提交：按普通提交（missing-code-tool / git-only-not-allowed，退出码 1），且不调用 merge-tree', () => {
  const log = join(tmpRoot, 'oct.log'); writeFileSync(log, '');
  const a = ts([oct0], { env: wrapEnv(log) }), b = ts([oct(M('Octopus', '', 'Code-Tool: git-only', 'Code-Source: human'))], { env: wrapEnv(log) });
  const calls = readFileSync(log, 'utf8');
  return a.status === 1 && a.err.includes('missing-code-tool') && b.status === 1 && b.err.includes('git-only-not-allowed') && calls.length > 0 && !/^merge-tree/m.test(calls);
});
t('16 退化合并且树与另一个父提交相同 → 视为干净', () => ts([degenerateClean]).status === 0);
t('17 干净合并带 Co-authored-by → 失败；干净合并的 author 邮箱非法 → 失败', () => {
  const tree = mr.g(['rev-parse', `${clean}^{tree}`]);
  const a = ts([mr.commitTree(tree, [m1, f1], M('Merge', '', `Co-authored-by: F <${NR}>`))]);
  const b = ts([mr.commitTree(tree, [m1, f1], 'Merge by stranger', { GIT_AUTHOR_EMAIL: STRANGER })]);
  return a.status === 1 && a.err.includes('co-authored-by-forbidden') && b.status === 1 && b.err.includes('author-email-not-allowed');
});
t('18 父提交对象缺失 → 退出码 2（check-failed）', () => {
  const tree = mr.g(['rev-parse', `${m1}^{tree}`]);
  const missing = mr.raw('commit', `tree ${tree}\nparent ${m1}\nparent ${'1'.repeat(40)}\nauthor Fixture Bot <${NR}> 1700000000 +0000\ncommitter Fixture Bot <${NR}> 1700000000 +0000\n\nMerge\n`);
  const r = ts([missing]); return r.status === 2 && r.err.includes('check-failed');
});
t('18a 不相关历史的合并：按普通提交（missing-code-tool，退出码 1，不是 2）；写了 Code-Tool 通过；根提交被检查', () => {
  const a = ts([unrel0]), b = trailer(mr.dir, ['--range', `${base}..${unrel(M('Merge unrelated', '', 'Code-Tool: human'))}`]);
  const badRoot = unrel(M('Merge unrelated', '', 'Code-Tool: human'), orphanBad);
  const c = trailer(mr.dir, ['--range', `${base}..${badRoot}`]);
  return a.status === 1 && a.err.includes('missing-code-tool') && !a.err.includes('check-failed') && b.status === 0
    && c.status === 1 && c.err.includes(`${sha12(orphanBad)} missing-code-tool`) && !c.err.includes(sha12(badRoot));
});
t('18b merge-tree 返回 1 但首行不是树 id → 退出码 2；返回其他码 → 退出码 2', () => {
  const log = join(tmpRoot, 'mt.log');
  return ts([clean], { env: wrapEnv(log, { FAKE_MT: 'bad1' }) }).status === 2 && ts([clean], { env: wrapEnv(log, { FAKE_MT: 'code3' }) }).status === 2;
});
t('18c --range 与 --range-all 在同一批夹具上结果一致（干净 / 夹带 / 冲突 / 章鱼 / 不相关历史）', () =>
  [[clean, 0], [evil, 1], [conflict0, 1], [oct0, 1], [unrel0, 1]].every(([tip, want]) => {
    const a = trailer(mr.dir, ['--range', `${base}..${tip}`]), b = trailer(mr.dir, ['--range-all', `${base}..${tip}`]);
    return a.status === want && b.status === want && a.err === b.err;
  }));
t('18c（改写自旧“PR 范围保留原非 merge 检查语义”）--range 检查合并提交：干净合并免填，夹带合并缺 Code-Tool 报错', () =>
  trailer(mr.dir, ['--range', `${base}..${clean}`]).status === 0 && trailer(mr.dir, ['--range', `${base}..${evil}`]).err.includes(`${sha12(evil)} missing-code-tool`));
t('18c（改写自旧“main 检查包含缺标识的 merge 提交”）--range-all 放行干净合并、拒绝缺 Code-Tool 的非干净合并', () =>
  trailer(mr.dir, ['--range-all', `${base}..${clean}`]).status === 0 && trailer(mr.dir, ['--range-all', `${base}..${evil}`]).status === 1);
t('18c（改写自旧“main 正确署名的所有提交通过”）--range-all：全部带 Code-Tool 的提交通过', () => {
  const r = trailer(mr.dir, ['--range-all', `${base}..${evilTrailer}`]); return r.status === 0 && /2 个提交通过/.test(r.stdout);
});

// ---------- D. privacy-scan --commits / --shas-stdin ----------
const sc = repo('scan');
const EMPTY_TREE = sc.g(['hash-object', '-t', 'tree', '-w', '--stdin'], { input: '' });
const ident = (name, email) => `${name} <${email}> 1700000000 +0000`;
const hdr = (key, lines) => `${key} ${lines[0]}` + lines.slice(1).map((l) => `\n ${l}`).join('');
function rawCommit({ headers = [], author = ident('Fixture Bot', NR), committer = ident('Fixture Bot', NR), message = M('x', '', 'Code-Tool: human'), parents = [], extraAuthor = [], extraCommitter = [] } = {}) {
  const lines = [`tree ${EMPTY_TREE}`, ...parents.map((p) => `parent ${p}`)];
  if (author !== null) lines.push(`author ${author}`); lines.push(...extraAuthor.map((a) => `author ${a}`));
  if (committer !== null) lines.push(`committer ${committer}`); lines.push(...extraCommitter.map((c) => `committer ${c}`));
  return sc.raw('commit', [...lines, ...headers].join('\n') + '\n\n' + message);
}
function rawTag({ object = rawCommit(), type = 'commit', tag = 'v1', tagger = [ident('Fixture Bot', NR)], message = 'release notes\n' } = {}) {
  return [`object ${object}`, `type ${type}`, `tag ${tag}`, ...tagger.map((x) => `tagger ${x}`)].join('\n') + '\n\n' + message;
}
const b64lines = (n = 256) => randomBytes(n).toString('base64').match(/.{1,64}/g);
const armor = (type = 'PGP SIGNATURE', middle = b64lines(), { end = true, after = [] } = {}) =>
  [`-----BEGIN ${type}-----`, ...(type === 'PGP SIGNATURE' ? [''] : []), ...middle, ...(type === 'PGP SIGNATURE' ? ['=' + randomBytes(3).toString('base64')] : []), ...(end ? [`-----END ${type}-----`] : []), ...after];
const scanObj = (id, o = {}) => pscan(sc.dir, ['--shas-stdin'], { input: id + '\n', ...o });
const TOKEN40 = () => { let s; do { s = randomBytes(30).toString('base64').replace(/[+/=]/g, '').slice(0, 40); } while (s.length < 40 || !/[0-9]/.test(s) || !/[A-Z]/.test(s) || !/[a-z]/.test(s)); return s; };
t('26 放行哈希邮箱的 author / committer 通过；陌生邮箱、元数据里的 example.com（不在表内）→ commit-meta-email', () =>
  scanClean(scanObj(rawCommit({ author: ident('Owner', OWNER), committer: ident('Owner', OWNER) })))
  && hasHit(scanObj(rawCommit({ author: ident('S', STRANGER) })), 'commit-meta-email', 'author')
  && hasHit(scanObj(rawCommit({ committer: ident('S', SOMEONE) })), 'commit-meta-email', 'committer'));
t('27 正文里的邮箱即使其哈希在表里仍命中 email', () => {
  const ownerFix = 'owner' + AT + FIX; const f = join(tmpRoot, 'allow-fix.json');
  writeFileSync(f, JSON.stringify({ version: 1, sha256_lowercase_email: [{ sha256: sha256(ownerFix), label: 'owner' }] }));
  return hasHit(scanObj(rawCommit({ message: M('x', '', 'mail ' + ownerFix, '', 'Code-Tool: human') }), { allow: f }), 'email', 'message');
});
t('28 正文里的 privacy-scan: allow 不豁免；--files 里仍有效（回归）', () => {
  const line = 'ip 8.8.' + '4.4 privacy-scan: allow'; const f = join(tmpRoot, 'allow-line.txt'); writeFileSync(f, line + '\n');
  return hasHit(scanObj(rawCommit({ message: M('x', '', line, '', 'Code-Tool: human') })), 'ipv4', 'message') && runNode(SCAN_CLI, ['--files', f], { cwd: tmpRoot }).status === 0;
});
t('29 名字字段含邮箱样式 → 命中', () => hasHit(scanObj(rawCommit({ author: ident('Fixture ' + STRANGER, NR) })), 'email', 'author-name'));
t('30a(i) 正文里的伪装甲块藏邮箱 → 命中', () => hasHit(scanObj(rawCommit({ message: ['x', '', ...armor('PGP SIGNATURE', [...b64lines(), 'mail ' + STRANGER]), '', 'Code-Tool: human'].join('\n') + '\n' })), 'email', 'message')
  && hasHit(scanObj(rawCommit({ message: ['x', '', 'Code-Tool: human', '', ...armor('PGP SIGNATURE', [STRANGER])].join('\n') + '\n' })), 'email', 'message'));
t('30a(ii) 未知头里的伪装甲块藏邮箱 → 命中', () => hasHit(scanObj(rawCommit({ headers: [hdr('x-foo', armor('PGP SIGNATURE', [...b64lines(), STRANGER]))] })), 'email', 'header'));
t('30a(iii) 标签说明里、真实签名之前的伪装甲块 → 命中', () => {
  const msg = [...armor('PGP SIGNATURE', [STRANGER]), 'notes', ...armor('PGP SIGNATURE')].join('\n') + '\n';
  return hasHit(scanObj(sc.raw('tag', rawTag({ message: msg }))), 'email', 'tag-message');
});
t('30b(i) gpgsig 的 Comment: 行含邮箱 → 命中', () => {
  const a = armor(); a.splice(1, 0, 'Comment: ' + STRANGER);
  return hasHit(scanObj(rawCommit({ headers: [hdr('gpgsig', a)] })), 'email', 'gpgsig');
});
t('30b(ii) gpgsig / gpgsig-sha256 的 BEGIN 与 END 之间混入含邮箱的普通行 → 命中', () =>
  hasHit(scanObj(rawCommit({ headers: [hdr('gpgsig', armor('PGP SIGNATURE', [...b64lines(), 'contact ' + STRANGER, ...b64lines()]))] })), 'email', 'gpgsig')
  && hasHit(scanObj(rawCommit({ headers: [hdr('gpgsig-sha256', armor('SSH SIGNATURE', [...b64lines(), STRANGER]))] })), 'email', 'gpgsig-sha256'));
const mergetagLines = (tagText) => tagText.replace(/\n$/, '').split('\n');
t('30b(iii) mergetag 内嵌签名混入含邮箱的行 → 命中', () => {
  const tagText = rawTag({ message: ['notes', ...armor('PGP SIGNATURE', [...b64lines(), STRANGER])].join('\n') + '\n' });
  return hasHit(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(tagText))] })), 'email', 'mergetag');
});
t('30b(iv) 只有 BEGIN 无 END、END 之后还有行、类型不配对 → 整段照扫（token 样式行命中）；结构合法时纯 base64 行被跳过（§9 残余）', () => {
  const tok = TOKEN40(), gh = 'ghp_' + TOKEN40();
  const noEnd = scanObj(rawCommit({ headers: [hdr('gpgsig', armor('PGP SIGNATURE', [tok, gh], { end: false }))] }));
  const afterEnd = scanObj(rawCommit({ headers: [hdr('gpgsig', armor('PGP SIGNATURE', [tok], { after: ['trailing'] }))] }));
  const mismatch = armor('PGP SIGNATURE', [tok]); mismatch[mismatch.length - 1] = '-----END SSH SIGNATURE-----';
  const mis = scanObj(rawCommit({ headers: [hdr('gpgsig', mismatch)] }));
  const valid = scanObj(rawCommit({ headers: [hdr('gpgsig', armor('PGP SIGNATURE', [tok]))] }));
  return hasHit(noEnd, 'cloudflare-token-like', 'gpgsig') && hasHit(noEnd, 'github-token', 'gpgsig') && hasHit(afterEnd, 'cloudflare-token-like', 'gpgsig')
    && hasHit(mis, 'cloudflare-token-like', 'gpgsig') && scanClean(valid);
});
t('30b(v) mergetag 标签说明里的伪装甲块、其后又跟着签名 → 伪装甲块里的邮箱命中', () => {
  const tagText = rawTag({ message: [...armor('PGP SIGNATURE', [STRANGER]), 'notes', ...armor('PGP SIGNATURE')].join('\n') + '\n' });
  return hasHit(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(tagText))] })), 'email', 'mergetag');
});
t('30c 合成的合法结构签名（gpgsig、gpgsig-sha256、SSH、mergetag、标签对象）→ 0 命中', () => {
  const tagText = rawTag({ message: ['notes', ...armor('PGP SIGNATURE')].join('\n') + '\n' });
  return [rawCommit({ headers: [hdr('gpgsig', armor())] }), rawCommit({ headers: [hdr('gpgsig-sha256', armor())] }), rawCommit({ headers: [hdr('gpgsig', armor('SSH SIGNATURE'))] }),
    rawCommit({ headers: [hdr('mergetag', mergetagLines(tagText))] }), sc.raw('tag', rawTag({ message: ['notes', ...armor('SSH SIGNATURE')].join('\n') + '\n' }))]
    .every((id) => scanClean(scanObj(id)));
});
t('D5 标签对象：tagger 为放行哈希邮箱 → 通过；tagger 为表外非 noreply 邮箱 → tag-meta-email', () =>
  scanClean(scanObj(sc.raw('tag', rawTag({ tagger: [ident('Owner', OWNER)] }))))
  && hasHit(scanObj(sc.raw('tag', rawTag({ tagger: [ident('S', STRANGER)] }))), 'tag-meta-email', 'tagger')
  && hasHit(scanObj(sc.raw('tag', rawTag({ tagger: [ident('S', SOMEONE)] }))), 'tag-meta-email', 'tagger'));
t('D5 mergetag：tagger 为放行哈希邮箱 → 通过；表外非 noreply 邮箱 → commit-meta-email', () =>
  scanClean(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(rawTag({ tagger: [ident('Owner', OWNER)] })))] })))
  && hasHit(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(rawTag({ tagger: [ident('S', STRANGER)] })))] })), 'commit-meta-email', 'mergetag'));
t('D5 tagger 恰好一行：重复或缺少 → tag-ident-malformed（mergetag 内 → commit-ident-malformed）；名字含 <> → 畸形', () =>
  hasHit(scanObj(sc.raw('tag', rawTag({ tagger: [ident('A', NR), ident('B', NR)] }))), 'tag-ident-malformed')
  && hasHit(scanObj(sc.raw('tag', rawTag({ tagger: [] }))), 'tag-ident-malformed')
  && hasHit(scanObj(sc.raw('tag', rawTag({ tagger: ['Name <x> <' + NR + '> 1700000000 +0000'] }))), 'tag-ident-malformed')
  && hasHit(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(rawTag({ tagger: [] })))] })), 'commit-ident-malformed', 'mergetag')
  && hasHit(scanObj(rawCommit({ headers: [hdr('mergetag', mergetagLines(rawTag({ tagger: [ident('A', NR), ident('B', NR)] })))] })), 'commit-ident-malformed', 'mergetag'));
t('标签链：标签指向标签时逐层扫描；指向 tree 时只扫标签对象', () => {
  const inner = sc.raw('tag', rawTag({ message: 'inner ' + STRANGER + '\n' }));
  const outer = sc.raw('tag', rawTag({ object: inner, type: 'tag', tag: 'outer' }));
  const treeTag = sc.raw('tag', rawTag({ object: EMPTY_TREE, type: 'tree', tag: 'tree' }));
  return hasHit(scanObj(outer), 'email', 'tag-message') && scanClean(scanObj(treeTag));
});
// 30c 真实签名：ubuntu-24.04 runner 自带 gpg / ssh-keygen；CI=true 时缺工具判失败，本地缺工具时明确输出 SKIPPED
const toolOk = (cmd, args) => { const r = spawnSync(cmd, args, { encoding: 'utf8' }); return !r.error; };
const haveSsh = toolOk('ssh-keygen', ['-?']), haveGpg = toolOk('gpg', ['--version']);
const realSig = (name, have, fn) => {
  if (have) t(name, fn);
  else if (process.env.CI === 'true') check(`${name}（CI=true 时缺工具判失败）`, false);
  else console.log(`SKIPPED  ${name}（本机缺少所需工具；CI=true 时此项判失败）`);
};
const sg = repo('signed');
const sgBase = sg.commit(M('base', '', 'Code-Tool: human'), 'a.txt', 'a\n');
realSig('30c 真实 SSH 签名提交、SSH 签名标签对象 → 0 命中', haveSsh, () => {
  const key = join(tmpRoot, 'sshkey'); if (!existsSync(key)) spawnSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-C', '', '-f', key]);
  const cfg = ['-c', 'gpg.format=ssh', '-c', 'gpg.ssh.program=ssh-keygen', '-c', `user.signingkey=${key}`];
  sg.g([...cfg, 'commit', '-q', '-S', '--allow-empty', '-m', M('ssh signed', '', 'Code-Tool: human')]);
  const c = sg.g(['rev-parse', 'HEAD']); sg.g([...cfg, 'tag', '-s', '-m', 'ssh release', 'v-ssh', c]);
  const tagId = sg.g(['rev-parse', 'v-ssh']);
  return /\ngpgsig -----BEGIN SSH SIGNATURE-----/.test(sg.g(['cat-file', 'commit', c])) && sg.g(['cat-file', 'tag', tagId]).includes('-----BEGIN SSH SIGNATURE-----')
    && scanClean(pscan(sg.dir, ['--shas-stdin'], { input: `${c}\n${tagId}\n` }));
});
const GNUPGHOME = join(tmpRoot, 'gnupg');
realSig('30c 真实 GPG 签名提交、GPG 签名标签、带签名标签的合并（mergetag）→ 0 命中；owner 签的标签 → 0 命中；陌生 tagger 的签名标签 → tag-meta-email', haveGpg, () => {
  mkdirSync(GNUPGHOME, { mode: 0o700, recursive: true });
  const genv = { GNUPGHOME };
  const gen = spawnSync('gpg', ['--batch', '--pinentry-mode', 'loopback', '--passphrase', '', '--quick-gen-key', 'Fixture Signer', 'ed25519', 'sign', 'never'], { env: { ...process.env, ...genv }, encoding: 'utf8' });
  if (gen.status !== 0) return false;
  const fpr = /^fpr:+([0-9A-F]{40,64}):/m.exec(spawnSync('gpg', ['--batch', '--with-colons', '--list-secret-keys'], { env: { ...process.env, ...genv }, encoding: 'utf8' }).stdout)[1];
  const cfg = ['-c', 'gpg.format=openpgp', '-c', 'gpg.program=gpg', '-c', `user.signingkey=${fpr}`];
  sg.g([...cfg, 'commit', '-q', '-S', '--allow-empty', '-m', M('gpg signed', '', 'Code-Tool: human')], { env: genv });
  const c = sg.g(['rev-parse', 'HEAD']);
  sg.g(['checkout', '-qb', 'side', sgBase]); const side = sg.commit(M('side', '', 'Code-Tool: human'), 's.txt', 's\n');
  sg.g([...cfg, 'tag', '-s', '-m', 'side release', 'v-side', side], { env: genv });
  sg.g([...cfg, 'tag', '-s', '-m', 'owner release', 'v-owner', side], { env: { ...genv, GIT_COMMITTER_EMAIL: OWNER } });
  sg.g([...cfg, 'tag', '-s', '-m', 'stranger release', 'v-stranger', side], { env: { ...genv, GIT_COMMITTER_EMAIL: STRANGER } });
  sg.g(['checkout', '-q', 'main']); sg.g(['merge', '-q', '--no-ff', '--no-edit', '-m', "Merge tag 'v-side'", 'v-side']);
  const mergeC = sg.g(['rev-parse', 'HEAD']);
  const ids = [c, mergeC, sg.g(['rev-parse', 'v-side']), sg.g(['rev-parse', 'v-owner'])];
  const okAll = scanClean(pscan(sg.dir, ['--shas-stdin'], { input: ids.join('\n') + '\n' }));
  const strangerTag = pscan(sg.dir, ['--shas-stdin'], { input: sg.g(['rev-parse', 'v-stranger']) + '\n' });
  return /\ngpgsig -----BEGIN PGP SIGNATURE-----/.test(sg.g(['cat-file', 'commit', c])) && /\nmergetag object /.test(sg.g(['cat-file', 'commit', mergeC]))
    && sg.g(['cat-file', 'commit', mergeC]).includes(' -----BEGIN PGP SIGNATURE-----') && okAll && hasHit(strangerTag, 'tag-meta-email', 'tagger');
});
t('31 信息含 \\x01 字节：正常解析，扫描数量等于 rev-list --count', () => {
  const r1 = rawCommit(); const r2 = rawCommit({ parents: [r1], message: M('bin \x01 data', '', 'Code-Tool: human') });
  const r = pscan(sc.dir, ['--commits', `${r1}..${r2}`]);
  return scanClean(r) && /已扫描 1 个对象/.test(r.stdout) && sc.g(['rev-list', '--count', `${r1}..${r2}`]) === '1';
});
t('32 非 UTF-8 提交信息（带 encoding 头）里的邮箱 → 命中', () => {
  const id = spawnSync('git', ['hash-object', '-t', 'commit', '-w', '--literally', '--stdin'], { cwd: sc.dir, env: gitEnv(), input: Buffer.concat([
    Buffer.from(`tree ${EMPTY_TREE}\nauthor ${ident('Fixture Bot', NR)}\ncommitter ${ident('Fixture Bot', NR)}\nencoding ISO-8859-1\n\ncaf`), Buffer.from([0xe9]), Buffer.from(` ${STRANGER}\n\nCode-Tool: human\n`)]) }).stdout.toString().trim();
  return hasHit(scanObj(id), 'email', 'message');
});
t('33 提交信息里的 token / IPv4 / tailnet 主机名样式命中', () => {
  const r = scanObj(rawCommit({ message: M('x', '', 'ghp_' + TOKEN40(), 'ip 8.8.' + '4.4', 'host box.tail1234' + '.ts.net', '', 'Code-Tool: human') }));
  return hasHit(r, 'github-token', 'message') && hasHit(r, 'ipv4', 'message') && hasHit(r, 'tailnet-or-lan-host', 'message');
});
t('34 畸形 author 行 → commit-ident-malformed', () => hasHit(scanObj(rawCommit({ author: 'Fixture Bot noangle 1700000000 +0000' })), 'commit-ident-malformed'));
t('34a 缺 / 重复 author、committer 头，名字含 < 或 > → commit-ident-malformed；spec 写明 git 取第一个 <…>', () =>
  [{ author: null }, { committer: null }, { extraAuthor: [ident('B', NR)] }, { extraCommitter: [ident('B', NR)] }, { author: `Name <${STRANGER}> <${NR}> 1700000000 +0000` }, { author: `Na>me <${NR}> 1700000000 +0000` }]
    .every((o) => hasHit(scanObj(rawCommit(o)), 'commit-ident-malformed'))
  && /%ae[^\n]*第一个/.test(readFileSync(join(ROOT, 'docs/specs/commit-signature-v3.md'), 'utf8')));
t('35 范围非法 / base 不存在 / 空范围 + --require-nonempty / 放行表缺失或格式错误 → 退出码 2（两个脚本）', () => {
  const r1 = rawCommit(), r2 = rawCommit({ parents: [r1] }), bad = join(tmpRoot, 'bad-allow.json'); writeFileSync(bad, '{"version":1,"sha256_lowercase_email":[{"sha256":"XYZ"}]}');
  return [[TRAILER_CLI, '--range'], [SCAN_CLI, '--commits']].every(([cli, mode]) =>
    ['a b..c', '--x..y', `${r1}...${r2}`, `${'2'.repeat(40)}..${r2}`].every((rg) => runNode(cli, [mode, rg, '--allow-hashes-file', allowFile], { cwd: sc.dir }).status === 2)
    && runNode(cli, [mode, `${r2}..${r2}`, '--require-nonempty', '--allow-hashes-file', allowFile], { cwd: sc.dir }).status === 2
    && runNode(cli, [mode, `${r2}..${r2}`, '--allow-hashes-file', allowFile], { cwd: sc.dir }).status === 0
    && runNode(cli, [mode, `${r1}..${r2}`, '--allow-hashes-file', join(tmpRoot, 'nope.json')], { cwd: sc.dir }).status === 2
    && runNode(cli, [mode, `${r1}..${r2}`, '--allow-hashes-file', bad], { cwd: sc.dir }).status === 2);
});
t('35a --shas-stdin：非法行 / 长度不对 / 不存在 / 类型不对 → 2；空输入 → 0，带 --require-nonempty → 2', () => {
  const c = rawCommit(), tag = sc.raw('tag', rawTag({ object: c }));
  return [TRAILER_CLI, SCAN_CLI].every((cli) => {
    const run = (input, extra = []) => runNode(cli, ['--shas-stdin', ...extra, '--allow-hashes-file', allowFile], { cwd: sc.dir, input }).status;
    return run('xyz\n') === 2 && run(c.slice(0, 39) + '\n') === 2 && run(c + '0\n') === 2 && run('3'.repeat(40) + '\n') === 2 && run(EMPTY_TREE + '\n') === 2
      && run('') === 0 && run('', ['--require-nonempty']) === 2 && run(c + '\n') === 0;
  }) && runNode(TRAILER_CLI, ['--shas-stdin', '--allow-hashes-file', allowFile], { cwd: sc.dir, input: tag + '\n' }).status === 2;
});
check('36 回归：scanText 对文件内容的邮箱规则不变（陌生邮箱命中，noreply / example.com 放行）', hit('x ' + STRANGER).includes('email') && hit(NR + ' ' + OWNER).length === 0);

// ---------- E. 信息文件与钩子 ----------
const mf = repo('msgfile');
mf.commit(M('base', '', 'Code-Tool: human'), 'a.txt', 'a\n');
const msgFile = join(tmpRoot, 'COMMIT_EDITMSG');
const both = (text, cwd = mf.dir) => { writeFileSync(msgFile, text); return [trailer(cwd, ['--message-file', msgFile]), pscan(cwd, ['--message-file', msgFile])]; };
const bothOk = (text, cwd) => both(text, cwd).every((r) => r.status === 0) && /提交信息通过/.test(OUTS.at(-2)) && /已扫描 1 个对象/.test(OUTS.at(-1));
const SCISSORS = (p) => `${p} ------------------------ >8 ------------------------`;
const commentBlock = (p) => [`${p} Please enter the commit message for your changes.`, `${p} Author:    Fixture <${STRANGER}>`, p, SCISSORS(p), `${p} Do not modify or remove the line above.`, 'diff --git a/x b/x', '+ ' + STRANGER];
t('38 注释行、# Author 行与 git commit -v 剪刀线 + diff：清理后通过', () => bothOk(M('feat: x', '', 'body', '', 'Code-Tool: human', ...commentBlock('#'))));
t('38a core.commentChar=; 与 core.commentString=// 时同样成立', () => {
  mf.g(['config', 'core.commentChar', ';']); const a = bothOk(M('feat: x', '', '#7 body line', '', 'Code-Tool: human', ...commentBlock(';')));
  mf.g(['config', '--unset', 'core.commentChar']); mf.g(['config', 'core.commentString', '//']);
  const b = bothOk(M('feat: x', '', 'body', '', 'Code-Tool: human', ...commentBlock('//')));
  mf.g(['config', '--unset', 'core.commentString']); return a && b;
});
t('38a core.commentChar=auto：保留 # 开头的正文行，剥离 ; 注释块并截断剪刀线', () => {
  mf.g(['config', 'core.commentChar', 'auto']);
  const a = bothOk(M('feat: x', '', '#12 is the issue', '', 'Code-Tool: human', ...commentBlock(';')));
  const [, b] = both(M('feat: x', '', '#12 mail ' + STRANGER, '', 'Code-Tool: human', ...commentBlock(';')));
  const c = bothOk(M('feat: x', '', '#1 and ;2', '', 'Code-Tool: human', ...commentBlock('@')));
  mf.g(['config', '--unset', 'core.commentChar']); return a && b.status === 1 && hasHit(b, 'email', 'message') && c;
});
t('38a（已知行为）auto 下没有注释块且正文末行以候选字符开头时，该行被当注释剥掉', () => {
  mf.g(['config', 'core.commentChar', 'auto']); const r = bothOk(M('feat: x', '', 'Code-Tool: human', '; ' + STRANGER)); mf.g(['config', '--unset', 'core.commentChar']); return r;
});
t('39 清理后仍有真正的违规 → 失败；privacy-scan: allow 在信息文件里不豁免', () => {
  const [a1, a2] = both(M('feat: x', '', 'mail ' + STRANGER, '', 'Code-Tool: human', ...commentBlock('#')));
  const [, b2] = both(M('feat: x', '', 'ip 8.8.' + '4.4 privacy-scan: allow', '', 'Code-Tool: human'));
  return a1.status === 0 && a2.status === 1 && hasHit(a2, 'email', 'message') && b2.status === 1 && hasHit(b2, 'ipv4', 'message');
});
t('E 信息文件模式：缺 Code-Tool、陌生 committer → 拒绝（改写自旧 message-file 的三条 CLI 用例）', () => {
  writeFileSync(msgFile, M('fix: change', '', 'Code-Tool: codex gpt-x high'));
  const okR = trailer(mf.dir, ['--message-file', msgFile]);
  const badCommitter = trailer(mf.dir, ['--message-file', msgFile], { env: { GIT_COMMITTER_EMAIL: SOMEONE } });
  const ownerAuthor = trailer(mf.dir, ['--message-file', msgFile], { env: { GIT_AUTHOR_NAME: 'Human', GIT_AUTHOR_EMAIL: OWNER } });
  const strangerAuthor = trailer(mf.dir, ['--message-file', msgFile], { env: { GIT_AUTHOR_NAME: 'Human', GIT_AUTHOR_EMAIL: SOMEONE } });
  writeFileSync(msgFile, M('fix: change', '', 'Agent: codex'));
  const noTool = trailer(mf.dir, ['--message-file', msgFile]);
  return okR.status === 0 && badCommitter.status === 1 && badCommitter.err.includes('committer-email-not-allowed') && ownerAuthor.status === 0
    && strangerAuthor.status === 1 && strangerAuthor.err.includes('author-email-not-allowed') && noTool.status === 1 && noTool.err.includes('missing-code-tool');
});
t('40 合并中（普通仓库与 git worktree，用 git rev-parse --git-path MERGE_HEAD 判断）只查邮箱、Co-authored-by 与花费字样', () => {
  mf.g(['checkout', '-qb', 'other']); mf.commit(M('other', '', 'Code-Tool: human'), 'o.txt', 'o\n'); mf.g(['checkout', '-q', 'main']);
  writeFileSync(msgFile, "Merge branch 'other'\n"); const before = trailer(mf.dir, ['--message-file', msgFile]);
  mf.g(['merge', '--no-commit', '--no-ff', '-q', 'other']);
  const during = trailer(mf.dir, ['--message-file', msgFile]);
  writeFileSync(msgFile, M("Merge branch 'other'", '', `Co-authored-by: F <${NR}>`)); const coauth = trailer(mf.dir, ['--message-file', msgFile]);
  mf.g(['merge', '--abort']);
  const wt = join(tmpRoot, 'msgfile-wt'); mf.g(['worktree', 'add', '-q', '-b', 'wtb', wt, 'main']);
  writeFileSync(msgFile, "Merge branch 'other'\n"); const wtBefore = trailer(wt, ['--message-file', msgFile]);
  spawnSync('git', ['merge', '--no-commit', '--no-ff', '-q', 'other'], { cwd: wt, env: gitEnv() });
  const wtDuring = trailer(wt, ['--message-file', msgFile]);
  spawnSync('git', ['merge', '--abort'], { cwd: wt, env: gitEnv() });
  return before.status === 1 && before.err.includes('missing-code-tool') && during.status === 0 && coauth.status === 1 && coauth.err.includes('co-authored-by-forbidden')
    && existsSync(join(wt, '.git')) && wtBefore.status === 1 && wtDuring.status === 0;
});

// 41 pre-push：用真实 git push 对本地裸仓库实跑仓库里的 .githooks/pre-push
const HOOKS = join(ROOT, '.githooks');
const bareA = repo('remote-a.git', { bare: true }).dir, bareB = repo('remote-b.git', { bare: true }).dir;
const pw = repo('pushwork'); pw.g(['config', 'core.hooksPath', HOOKS]); pw.g(['remote', 'add', 'origin', bareA]); pw.g(['remote', 'add', 'mirror', bareB]);
const push = (...args) => { const r = pw.g(['push', ...args], { full: true }); return { status: r.status, err: r.stderr }; };
const hookRan = (r) => /pre-push:/.test(r.err);
const remoteHas = (ref) => spawnSync('git', ['rev-parse', '-q', '--verify', ref], { cwd: bareA, env: gitEnv() }).status === 0;
const p0 = pw.commit(M('init', '', 'Code-Tool: human'), 'a.txt', '1\n');
t('41e 没有任何远程跟踪分支：只检查 tip 并提示“范围无法确定”，退出码 0', () => { const r = push('origin', 'main'); return r.status === 0 && hookRan(r) && /范围无法确定/.test(r.err); });
t('41a 新分支、无新提交 → 通过（空范围不失败）', () => { const r = push('origin', 'main:newb'); return r.status === 0 && hookRan(r) && remoteHas('refs/heads/newb'); });
t('41b 附注标签指向已推送的提交 → 通过且标签对象被扫描；标签说明含陌生邮箱 → 阻断', () => {
  pw.g(['tag', '-a', 't-ok', '-m', 'release notes', p0]); pw.g(['tag', '-a', 't-bad', '-m', 'notes ' + STRANGER, p0]);
  const a = push('origin', 'refs/tags/t-ok'), b = push('origin', 'refs/tags/t-bad');
  return a.status === 0 && hookRan(a) && b.status !== 0 && hookRan(b) && !remoteHas('refs/tags/t-bad');
});
t('41c remote_sha 本地不存在（他人推进后强推）→ 不报 git 错误，只检查本地未在远程跟踪分支里的提交', () => {
  const other = join(tmpRoot, 'other-clone'); spawnSync('git', ['clone', '-q', bareA, other], { env: gitEnv() });
  writeFileSync(join(other, 'x.txt'), 'x\n'); spawnSync('git', ['add', 'x.txt'], { cwd: other, env: gitEnv() });
  spawnSync('git', ['commit', '-q', '-m', M('other', '', 'Code-Tool: human')], { cwd: other, env: gitEnv() });
  spawnSync('git', ['push', '-q', 'origin', 'main'], { cwd: other, env: gitEnv() });
  const p1 = pw.commit(M('mine', '', 'Code-Tool: human'), 'b.txt', '2\n'); const a = push('-f', 'origin', 'main');
  pw.commit('no trailer here', 'c.txt', '3\n'); const b = push('-f', 'origin', 'main'); pw.g(['reset', '-q', '--hard', p1]);
  return a.status === 0 && hookRan(a) && !/check-failed|fatal/.test(a.err) && b.status !== 0 && /missing-code-tool/.test(b.err);
});
t('41d 删除分支跳过；标签指向标签逐层扫描；标签指向 tree 只扫标签对象', () => {
  const del = push('origin', ':refs/heads/newb');
  pw.g(['tag', '-a', 'inner', '-m', 'inner ' + STRANGER, 'HEAD']); pw.g(['tag', '-a', 'outer', '-m', 'outer', 'inner']);
  const nested = push('origin', 'refs/tags/outer');
  pw.g(['tag', '-a', 'treetag', '-m', 'tree', 'HEAD^{tree}']); pw.g(['tag', '-a', 'treebad', '-m', 'tree ' + STRANGER, 'HEAD^{tree}']);
  const tt = push('origin', 'refs/tags/treetag'), tb = push('origin', 'refs/tags/treebad');
  return del.status === 0 && !remoteHas('refs/heads/newb') && nested.status !== 0 && hookRan(nested) && tt.status === 0 && hookRan(tt) && tb.status !== 0;
});
t('41f 有违规提交（缺 Code-Tool 或陌生邮箱）→ 阻断', () => {
  const keep = pw.g(['rev-parse', 'HEAD']);
  pw.commit(M('stranger', '', 'Code-Tool: human'), 'd.txt', '4\n', { GIT_AUTHOR_EMAIL: STRANGER }); const a = push('origin', 'main'); pw.g(['reset', '-q', '--hard', keep]);
  pw.commit('missing tool', 'd.txt', '5\n'); const b = push('origin', 'main'); pw.g(['reset', '-q', '--hard', keep]);
  return a.status !== 0 && /author-email-not-allowed|commit-meta-email/.test(a.err) && b.status !== 0 && /missing-code-tool/.test(b.err);
});
t('41（R1）同一提交已在另一个远程的跟踪分支上，推到 origin 时仍被扫描', () => {
  const keep = pw.g(['rev-parse', 'HEAD']);
  pw.commit('mirror only, no trailer', 'e.txt', '6\n'); const m = push('--no-verify', 'mirror', 'main');
  const a = push('origin', 'main'); pw.g(['reset', '-q', '--hard', keep]);
  return m.status === 0 && pw.g(['rev-parse', 'refs/remotes/mirror/main']) !== keep && a.status !== 0 && /missing-code-tool/.test(a.err);
});
t('41（R1）推送目标是 URL（没有远程名）→ 按“范围无法确定”只检查 tip', () => {
  pw.commit(M('via url', '', 'Code-Tool: human'), 'g.txt', '7\n'); const r = push(bareA, 'main'); return r.status === 0 && /范围无法确定/.test(r.err);
});

// ---------- F. 工作流与文档一致性 ----------
const wf = (name) => readFileSync(join(ROOT, '.github/workflows', name), 'utf8');
const steps = (text) => text.split(/^(?=      - )/m);
const PUSH_STEP = 'push main 报告';
const BASE_ALLOW = '"$RUNNER_TEMP/base-allowed-author-hashes.json"';
const GIT_SHOW_BASE = 'git show "origin/${BASE_REF}:scripts/allowed-author-hashes.json"';
t('42 --allow-hashes-file 实参只能是 base 版放行表（push main 报告步骤除外），且前面有 git show 步骤', () => ['workflow-v2.yml', 'privacy.yml'].every((name) => {
  const withFlag = steps(wf(name)).filter((s) => s.includes('--allow-hashes-file'));
  return withFlag.length > 0 && withFlag.every((s) => {
    const args = [...s.matchAll(/--allow-hashes-file (\S+)/g)].map((m) => m[1]);
    if (s.includes(PUSH_STEP)) return !s.includes("github.event_name == 'pull_request'") && args.every((a) => a === 'scripts/allowed-author-hashes.json');
    return args.every((a) => a === BASE_ALLOW) && s.indexOf(GIT_SHOW_BASE) >= 0 && s.indexOf(GIT_SHOW_BASE) < s.indexOf('--allow-hashes-file');
  });
}));
t('43 privacy.yml：--commits + --require-nonempty，条件 !cancelled() && pull_request；不含 policy-change-gate', () => {
  const s = steps(wf('privacy.yml')).find((x) => x.includes('--commits'));
  return !!s && s.includes('--require-nonempty') && s.includes("if: ${{ !cancelled() && github.event_name == 'pull_request' }}") && !/policy-change-gate|policy-gate/.test(wf('privacy.yml') + wf('workflow-v2.yml'));
});
t('44 agent-trailer job 保持 --range 并带 --require-nonempty', () => /node scripts\/agent-trailer\.mjs --range "origin\/\$\{BASE_REF\}\.\.\$\{HEAD_SHA\}" --require-nonempty/.test(wf('workflow-v2.yml')));
t('45 docs/workflow-v2.md：Code-Tool / Code-Source 语法、合并方式、Web 操作说明；无 Agent: human 表格行；第 11 行只禁花费 / token / 耗时', () => {
  const d = readFileSync(join(ROOT, 'docs/workflow-v2.md'), 'utf8');
  const costLine = d.split('\n').find((l) => l.includes('AI 花费')) || '';
  return d.includes('Code-Tool: <tool> <model> <effort>') && d.includes('Code-Source:') && d.includes('Create a merge commit') && d.includes('Commit suggestion')
    && !/\|\s*`Agent: human`\s*\|/.test(d) && costLine.includes('token 用量') && costLine.includes('运行耗时') && !/不写[^。]*模型档位/.test(costLine) && costLine.includes('模型与档位允许')
    && d.includes('pre-push') && d.includes('policy-gate');
});
t('46 policy-gate.yml：只有 pull_request_target；权限恰为两项读；无 uses / checkout；run 里无 ${{ }} 插值', () => {
  const g = wf('policy-gate.yml');
  const block = (key) => { const m = new RegExp(`^${key}:\\n((?:  .*\\n)+)`, 'm').exec(g); return m ? m[1].split('\n').filter((l) => /^  \S/.test(l)).map((l) => l.trim()) : []; };
  const runs = [...g.matchAll(/run: \|\n((?:(?: {10,}.*)?\n)+)/g)].map((m) => m[1]).join('\n');
  const exprs = [...g.matchAll(/\$\{\{[^}]*\}\}/g)].map((m) => m[0].replace(/\s+/g, ' '));
  return JSON.stringify(block('on').map((l) => l.replace(/:.*/, ''))) === '["pull_request_target"]' && JSON.stringify(block('permissions')) === '["contents: read","pull-requests: read"]'
    && !/write|id-token/.test(g) && !/uses:|actions\/checkout/.test(g) && runs.length > 0 && !runs.includes('${{')
    && exprs.every((e) => ['${{ github.token }}', '${{ github.repository }}', '${{ github.event.pull_request.number }}'].includes(e));
});
t('47 push main 报告步骤：无 continue-on-error、条件含 main、rev-list 失败即红、--require-nonempty、exit 1 / ::error:: / 摘要', () => {
  const all = steps(wf('privacy.yml')); const s = all.find((x) => x.includes(PUSH_STEP)) || '';
  const coe = all.filter((x) => x.includes('continue-on-error'));
  return s.includes("if: ${{ !cancelled() && github.event_name == 'push' && github.ref == 'refs/heads/main' }}") && !s.includes('continue-on-error')
    && (s.match(/if ! git rev-list /g) || []).length === 2 && (s.match(/--shas-stdin --require-nonempty/g) || []).length === 2
    && s.includes('exit 1') && s.includes('::error::') && s.includes('GITHUB_STEP_SUMMARY')
    && coe.length === 1 && coe[0].includes('全库扫描') && coe[0].includes('continue-on-error: true');
});
t('48 policy-gate 政策文件清单与假 gh 实跑（R6：.github/**）', () => {
  const g = wf('policy-gate.yml');
  const caseLine = (/^\s*(scripts\/allowed-author-hashes\.json\|.*)\)$/m.exec(g) || [])[1] || '';
  const want = ['scripts/allowed-author-hashes.json', 'scripts/commit-email-policy.mjs', 'scripts/agent-trailer.mjs', 'scripts/privacy-scan.mjs', 'scripts/check-spec-tests.mjs', 'tests/workflow-scripts.mjs', '.gitleaks.toml', '.github/**', '.githooks/*'];
  if (JSON.stringify(caseLine.split('|')) !== JSON.stringify(want) || !g.includes('previous_filename')) return false;
  const lines = g.split('\n'); const i = lines.findIndex((l) => /^\s+run: \|$/.test(l)); const ind = /^\s*/.exec(lines[i + 1])[0].length;
  const body = []; for (let j = i + 1; j < lines.length && (lines[j].trim() === '' || /^\s*/.exec(lines[j])[0].length >= ind); j++) body.push(lines[j].slice(ind));
  const gate = join(tmpRoot, 'gate'); mkdirSync(gate, { recursive: true }); writeFileSync(join(gate, 'run.sh'), body.join('\n') + '\n');
  writeFileSync(join(gate, 'gh'), '#!/bin/sh\n[ -n "${FAKE_FAIL:-}" ] && exit 1\ncase "$*" in *"/files"*) cat "$FAKE_FILES" ;; *) echo "$FAKE_TOTAL" ;; esac\n'); chmodSync(join(gate, 'gh'), 0o755);
  const run = (files, { total = String(files.length), pr = '7', fail } = {}) => {
    writeFileSync(join(gate, 'files.txt'), files.join('\n') + '\n');
    const r = spawnSync('bash', [join(gate, 'run.sh')], { encoding: 'utf8', env: { PATH: `${gate}:${process.env.PATH}`, RUNNER_TEMP: gate, GH_TOKEN: 'x', REPO: 'owner/repo', PR_NUMBER: pr, FAKE_FILES: join(gate, 'files.txt'), FAKE_TOTAL: total, ...(fail ? { FAKE_FAIL: '1' } : {}) } });
    return { status: r.status, out: r.stdout + r.stderr };
  };
  const policyFiles = ['scripts/allowed-author-hashes.json', 'scripts/commit-email-policy.mjs', 'scripts/agent-trailer.mjs', 'scripts/privacy-scan.mjs', 'scripts/check-spec-tests.mjs', 'tests/workflow-scripts.mjs', '.gitleaks.toml', '.github/workflows/policy-gate.yml', '.github/actions/x/action.yml', '.githooks/pre-push'];
  const forged = run(['src/a.rs', '.githooks/::error::forged']);
  return run(['src/a.rs', 'docs/specs/x.md']).status === 0 && policyFiles.every((f) => run(['src/a.rs', f]).status !== 0)
    && forged.status !== 0 && !forged.out.includes('::error::forged') && forged.out.includes('.githooks/??error??forged')
    && run(['src/a.rs'], { total: '3000' }).status !== 0 && run(['src/a.rs'], { pr: '7; true' }).status !== 0 && run(['src/a.rs'], { fail: true }).status !== 0;
});
t('37 输出不泄露：所有 CLI 输出不含测试邮箱的本地部分与域名，且不含 @（同时确认确有 *-meta-email 命中）', () => {
  const all = OUTS.join('\n');
  return /commit-meta-email/.test(all) && /tag-meta-email/.test(all) && !['someone', 'example.com', FIX, '@'].some((s) => all.includes(s)) && !/\bowner\b/.test(all);
});

check('spec：改 spec 不改测试失败', checkSpecTests(['docs/specs/a.md']) === false);
check('spec：改 spec 同时改测试通过（Rust 集成测试 / Go / 前端 / Kotlin / 脚本测试）', ['tests/a.mjs', 'crates/airhand-link/tests/direct.rs', 'client/src-tauri/tests/x.rs', 'p2p/gate_test.go', 'p2p/sub/helper_test.go', 'client/tests/a.test.ts', 'android/app/src/test/java/app/x/ATest.kt'].every((t) => checkSpecTests(['docs/specs/a.md', t]) === true));
check('spec：只改非测试文件不算触及测试', ['crates/airhand-link/src/host.rs', 'p2p/gate.go', 'docs/specs/review/a.md', 'tests/a.txt', 'client/tests/a.ts'].every((t) => checkSpecTests(['docs/specs/a.md', t]) === false));
check('spec：新增 Rust 内联测试算触及测试', addedInlineRustTests('+++ b/crates/airhand-link/src/host.rs\n+#[tokio::test]\n+async fn x() {}\n') === true && addedInlineRustTests('+++ b/crates/airhand-link/src/host.rs\n+#[cfg(test)]\n') === true && checkSpecTests(['docs/specs/a.md'], ['docs/specs/a.md'], true) === true);
check('spec：非 .rs 文件里的 #[test] 字样、普通代码行不算', addedInlineRustTests('+++ b/docs/specs/a.md\n+#[test]\n') === false && addedInlineRustTests('+++ b/crates/x/src/a.rs\n+fn test_like() {}\n') === false);
check('spec：TEMPLATE.md / README.md 豁免；删除 spec 不触发；子目录评审记录不是 spec', checkSpecTests(['docs/specs/TEMPLATE.md']) && checkSpecTests(['docs/specs/README.md']) && checkSpecTests(['docs/specs/a.md'], []) && checkSpecTests(['docs/specs/review/a.md']));
check('privacy：跳过 node_modules / .wrangler / package-lock.json 与图片，不误跳相似文件名', shouldSkip('node_modules/a.js') && shouldSkip('x/.wrangler/s.json') && shouldSkip('package-lock.json') && shouldSkip('shots/a.PNG') && !shouldSkip('docs/node_modules-notes.md') && !shouldSkip('src/package-lock.json.md') && !shouldSkip('src/worker.js'));
// go.sum 窄例外：夹具用字符串拼接，避免本测试文件自身被 privacy-scan 命中
const GS = 'github.com/example/mod v0.1.0/go.mod h1:Jo8rI7m0NieZyLI5e2CDlRd' + 'RqRRB4S7Xp77ukDjH+Fw=';
const GS2 = 'github.com/example/mod v0.1.0 h1:Jo8rI7m0NieZyLI5e2CDlRd' + 'RqRRB4S7Xp77ukDjH+Fw=';
const gsHit = (t, file) => scanText(t, file).map((h) => h.split(' ')[1]);
check('privacy：go.sum 校验和行（含 /go.mod 与普通两种）不命中 cloudflare-token-like', gsHit(GS, 'go.sum').length === 0 && gsHit(GS2, 'spike/p2p/helper/go.sum').length === 0 && gsHit(GS + '\r', 'a/go.sum').length === 0);
check('privacy：同样内容在非 go.sum 文件仍命中', gsHit(GS, 'notes.md').includes('cloudflare-token-like') && gsHit(GS, 'go.sum.txt').includes('cloudflare-token-like') && gsHit(GS, 'mygo.sum').includes('cloudflare-token-like') && gsHit(GS, 'x').includes('cloudflare-token-like'));
check('privacy：go.sum 里非校验和形状的行仍命中', gsHit('token ' + 'Jo8rI7m0NieZyLI5e2CDlRd' + 'RqRRB4S7Xp77ukDjH', 'go.sum').includes('cloudflare-token-like') && gsHit(GS + ' extra', 'go.sum').includes('cloudflare-token-like') && gsHit('Jo8rI7m0NieZyLI5e2CDlRd' + 'RqRRB4S7Xp77ukDjH+Fw=', 'go.sum').includes('cloudflare-token-like'));
check('privacy：go.sum 里的其他规则（邮箱、IPv4、GitHub token）仍命中', gsHit('a.b@' + 'gmail.com', 'go.sum').includes('email') && gsHit('8.8.' + '4.4', 'go.sum').includes('ipv4') && gsHit('ghp_' + 'a1B2'.repeat(9), 'go.sum').includes('github-token'));
if (existsSync(GNUPGHOME)) spawnSync('gpgconf', ['--kill', 'gpg-agent'], { env: { ...process.env, GNUPGHOME } });
rmSync(tmpRoot, { recursive: true, force: true });
const f = R.filter((x) => !x).length; console.log(`结果：${R.length - f}/${R.length} 通过`); process.exit(f ? 1 : 0);
