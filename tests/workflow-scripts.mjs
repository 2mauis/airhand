// 工作流脚本单元测试：privacy-scan 规则、agent-trailer、spec-requires-tests（纯函数 + 临时 git 仓库，不需要网络）
// 移植自 2mauis 的 passkey-demo 项目（workflow v2，MIT，Copyright (c) 2026 2mauis）：privacy / trailer 用例沿用原文，
// spec 用例按 airhand 的测试形态改写；删除了与 Cloudflare 部署、管理 token、发布快照相关的用例。
// 用法：node tests/workflow-scripts.mjs
import { scanText, shouldSkip } from '../scripts/privacy-scan.mjs';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { checkAgentTrailer } from '../scripts/agent-trailer.mjs';
import { checkSpecTests, addedInlineRustTests } from '../scripts/check-spec-tests.mjs';
const R = []; const check = (n, ok) => { R.push(ok); console.log(`${ok ? 'PASS' : 'FAIL'}  ${n}`); };
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
const msg = 'feat: x\n\nbody\n\nAgent: grok-code-assistant\n';
const A = 'Grok Bot Friday (for 2mauis)', E = '2908005+2mauis@users.noreply.github.com';
check('trailer：正确署名通过', checkAgentTrailer({ message: msg, authorName: A, authorEmail: E }).length === 0);
check('trailer：缺 Agent 失败', checkAgentTrailer({ message: 'feat: x\n', authorName: A, authorEmail: E }).includes('missing-or-duplicate-agent-trailer'));
check('trailer：非 noreply 邮箱失败', checkAgentTrailer({ message: msg, authorName: A, authorEmail: 'a@example.com' }).includes('author-email-not-noreply'));
check('trailer：author 与 Agent 不匹配失败', checkAgentTrailer({ message: msg, authorName: 'Someone', authorEmail: E }).includes('author-does-not-match-agent'));
const humanCommit = { message: 'fix: manual change\n\nAgent: human\n', authorName: 'Human Contributor', authorEmail: 'human@example.com', committerEmail: 'human@example.com' };
check('trailer：人工提交允许本人普通邮箱', checkAgentTrailer(humanCommit).length === 0);
check('trailer：人工提交仍可自选 noreply', checkAgentTrailer({ ...humanCommit, authorEmail: E, committerEmail: E }).length === 0);
check('trailer：AI 名称不能伪装 human', checkAgentTrailer({ ...humanCommit, authorName: 'Codex (for 2mauis)' }).includes('ai-author-marked-human'));
check('trailer：AI committer 普通邮箱失败', checkAgentTrailer({ message: msg, authorName: A, authorEmail: E, committerEmail: 'human@example.com' }).includes('committer-email-not-noreply'));
check('trailer：AI 提交允许 GitHub 服务 committer', checkAgentTrailer({ message: msg, authorName: A, authorEmail: E, committerEmail: 'noreply@' + 'github.com' }).length === 0);
const attributionDir = mkdtempSync(join(tmpdir(), 'attribution-cli-'));
try {
  const messageFile = join(attributionDir, 'message');
  writeFileSync(messageFile, 'fix: AI change\n\nAgent: codex\n');
  const cli = fileURLToPath(new URL('../scripts/agent-trailer.mjs', import.meta.url));
  const env = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', GIT_AUTHOR_NAME: 'Codex (for 2mauis)', GIT_AUTHOR_EMAIL: E, GIT_COMMITTER_NAME: 'Codex (for 2mauis)', GIT_COMMITTER_EMAIL: 'human@example.com' };
  const run = (args, overrides = {}) => spawnSync(process.execPath, [cli, ...args], { cwd: attributionDir, env: { ...env, ...overrides }, encoding: 'utf8' });
  check('trailer CLI：message-file 拒绝 AI 普通 committer 邮箱', run(['--message-file', messageFile]).status !== 0);
  check('trailer CLI：message-file 接受 AI noreply committer', run(['--message-file', messageFile], { GIT_COMMITTER_EMAIL: E }).status === 0);
  writeFileSync(messageFile, 'fix: human change\n\nAgent: human\n');
  check('trailer CLI：message-file 接受人工姓名与普通邮箱', run(['--message-file', messageFile], { GIT_AUTHOR_NAME: 'Human', GIT_AUTHOR_EMAIL: 'human@example.com' }).status === 0);
  const gitTest = (...args) => {
    const result = spawnSync('git', args, { cwd: attributionDir, env: { ...env, GIT_COMMITTER_EMAIL: E }, encoding: 'utf8' });
    if (result.status !== 0) throw new Error('synthetic-git-operation-failed');
    return result.stdout.trim();
  };
  gitTest('init', '-q', '-b', 'main');
  gitTest('commit', '-q', '--allow-empty', '-m', 'base\n\nAgent: codex');
  const base = gitTest('rev-parse', 'HEAD');
  gitTest('checkout', '-qb', 'feature');
  gitTest('commit', '-q', '--allow-empty', '-m', 'feature\n\nAgent: codex');
  gitTest('checkout', '-q', 'main');
  gitTest('merge', '--no-ff', 'feature', '-m', 'synthetic merge without attribution');
  check('trailer CLI：PR 范围保留原非 merge 检查语义', run(['--range', base + '..HEAD']).status === 0);
  check('trailer CLI：main 检查包含缺标识的 merge 提交', run(['--range-all', base + '..HEAD']).status !== 0);
  gitTest('commit', '--amend', '-q', '-m', 'synthetic merge\n\nAgent: codex');
  check('trailer CLI：main 正确署名的所有提交通过', run(['--range-all', base + '..HEAD']).status === 0);
} finally { rmSync(attributionDir, { recursive: true, force: true }); }
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
const f = R.filter((x) => !x).length; console.log(`结果：${R.length - f}/${R.length} 通过`); process.exit(f ? 1 : 0);
