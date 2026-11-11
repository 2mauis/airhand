#!/usr/bin/env node
// 移植自 2mauis 的 passkey-demo 项目（workflow v2，MIT，Copyright (c) 2026 2mauis），按署名规范 v3 重写；许可见 docs/workflow-v2.md「来源与许可」。
// 提交署名与 trailer 检查（本地 commit-msg / pre-push 钩子与 CI 共用），规则见 docs/specs/commit-signature-v3.md。
// 用法：node scripts/agent-trailer.mjs (--message-file FILE | --range BASE..HEAD | --range-all BASE..HEAD | --shas-stdin)
//         [--require-nonempty] [--allow-hashes-file FILE]
// 退出码：0 通过；1 规则违反；2 检查本身失败。输出只含 sha12 与错误码，不输出邮箱。
import { spawnSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { isAllowedAuthorEmail, isAllowedCommitterEmail, loadAllowedHashes, readCommentPrefix, cleanMessage, DEFAULT_ALLOW_FILE } from './commit-email-policy.mjs';

const TOOL = '(codex|claude-code|cursor-agent) ([a-z0-9][a-z0-9.-]{0,63}) (none|low|medium|high|xhigh|max|ultra|unknown)';
const CODE_TOOL = new RegExp(`^Code-Tool: (?:${TOOL}|git-only|human)$`);
const CODE_SOURCE = new RegExp(`^Code-Source: (?:${TOOL}|human|unknown)$`);
const AGENT = /^Agent: [a-z0-9][a-z0-9-]{0,39}$/;
const TRAILER_LINE = /^[A-Za-z][A-Za-z0-9-]*: \S.*$/;
const COST = /\b(token usage|tokens used|cost:\s*\$|api cost)\b/i;
// 合并进行中（钩子里树尚未定型）只做这几项，完整判定交给 pre-push 与 CI
export const MERGE_IN_PROGRESS_CHECKS = new Set(['co-authored-by-forbidden', 'ai-cost-or-usage-in-message', 'author-email-not-allowed', 'committer-email-not-allowed']);

export function normalizeMessage(message) {
  const lines = String(message).replace(/\r\n/g, '\n').split('\n').map((l) => l.replace(/\s+$/, ''));
  while (lines.length && lines[lines.length - 1] === '') lines.pop();
  return lines;
}
// trailer 块 = 最后一个空行分隔的段落；不能是第一段，且每一行都必须是 `Key: value`
export function trailerBlock(lines) {
  let start = lines.length; while (start > 0 && lines[start - 1] !== '') start--;
  if (start === 0 || start === lines.length) return new Set();
  const idx = []; for (let i = start; i < lines.length; i++) { if (!TRAILER_LINE.test(lines[i])) return new Set(); idx.push(i); }
  return new Set(idx);
}
function single(lines, block, re, { duplicate, outside, invalid }, valid, errors) {
  const found = lines.map((l, i) => [l, i]).filter(([l]) => re.test(l));
  if (found.length === 0) return { count: 0, value: null };
  if (found.length > 1) { errors.push(duplicate); return { count: found.length, value: null }; }
  const [line, i] = found[0];
  if (!block.has(i)) { errors.push(outside); return { count: 1, value: null }; }
  if (!valid.test(line)) { errors.push(invalid); return { count: 1, value: null }; }
  return { count: 1, value: line.replace(/^[^:]+: /, '') };
}

export function checkAgentTrailer({ message, authorEmail, committerEmail, isMerge = false, isCleanMerge = false, allowedHashes = new Set() }) {
  const errors = [];
  const lines = normalizeMessage(message);
  const block = trailerBlock(lines);
  const tool = single(lines, block, /^\s*code-tool\s*:/i, { duplicate: 'duplicate-code-tool', outside: 'code-tool-outside-trailer-block', invalid: 'invalid-code-tool' }, CODE_TOOL, errors);
  if (tool.count === 0 && !isCleanMerge) errors.push('missing-code-tool');
  if (tool.value === 'git-only' && isMerge && !isCleanMerge) errors.push('git-only-not-allowed');
  const source = single(lines, block, /^\s*code-source\s*:/i, { duplicate: 'duplicate-code-source', outside: 'code-source-outside-trailer-block', invalid: 'invalid-code-source' }, CODE_SOURCE, errors);
  if (source.count > 0 && (tool.count === 0 || (tool.value !== null && tool.value !== 'git-only'))) errors.push('code-source-not-allowed');
  if (source.count === 0 && tool.value === 'git-only' && !isMerge) errors.push('missing-code-source');
  const agents = lines.filter((l) => /^\s*agent\s*:/i.test(l));
  if (agents.length > 1 || (agents.length === 1 && !AGENT.test(agents[0]))) errors.push('invalid-agent-trailer');
  if (lines.some((l) => /^\s*co-authored-by\s*:/i.test(l))) errors.push('co-authored-by-forbidden');
  if (authorEmail !== undefined && !isAllowedAuthorEmail(authorEmail, allowedHashes)) errors.push('author-email-not-allowed');
  if (committerEmail !== undefined && !isAllowedCommitterEmail(committerEmail, allowedHashes)) errors.push('committer-email-not-allowed');
  if (COST.test(lines.join('\n'))) errors.push('ai-cost-or-usage-in-message');
  return errors;
}

class CheckFailed extends Error {}
function git(args, { allow = [0], input } = {}) {
  const r = spawnSync('git', args, { encoding: 'utf8', input, maxBuffer: 64 << 20, stdio: [input === undefined ? 'ignore' : 'pipe', 'pipe', 'pipe'] });
  if (r.error || !allow.includes(r.status)) throw new CheckFailed(`git ${args[0]}`);
  return { status: r.status, out: r.stdout };
}
export const RANGE = /^[^\s.-][^\s]*(?<!\.)\.\.(?!\.)[^\s]+$/;
const OID = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const isCommit = (oid) => git(['cat-file', '-e', `${oid}^{commit}`], { allow: [0, 1, 128] }).status === 0;

// 合并判定流程（spec §3.2）：返回 { isMerge, isCleanMerge }；无法下结论时抛 CheckFailed
export function classifyCommit(sha, parents) {
  if (parents.length < 2) return { isMerge: false, isCleanMerge: false };
  if (parents.length > 2) return { isMerge: true, isCleanMerge: false };
  const [p1, p2] = parents;
  if (!isCommit(p1) || !isCommit(p2)) throw new CheckFailed('missing-parent');
  if (git(['merge-base', p1, p2], { allow: [0, 1] }).status === 1) return { isMerge: true, isCleanMerge: false };
  const mt = git(['merge-tree', '--write-tree', p1, p2], { allow: [0, 1] });
  const tree = mt.out.split('\n')[0];
  if (!OID.test(tree)) throw new CheckFailed('merge-tree-output');
  if (mt.status === 1) return { isMerge: true, isCleanMerge: false };
  const own = git(['rev-parse', `${sha}^{tree}`]).out.trim();
  return { isMerge: true, isCleanMerge: own === tree };
}
function checkCommit(sha, allowedHashes) {
  const raw = git(['cat-file', 'commit', sha]).out;
  const end = raw.indexOf('\n\n');
  const parents = (end < 0 ? raw : raw.slice(0, end)).split('\n').filter((l) => l.startsWith('parent ')).map((l) => l.slice(7));
  const [ae, ce, msg = ''] = git(['show', '-s', '--format=%ae%x00%ce%x00%B', sha]).out.split('\0');
  return checkAgentTrailer({ message: msg, authorEmail: ae, committerEmail: ce, ...classifyCommit(sha, parents), allowedHashes });
}
function parseArgs(argv) {
  const o = { mode: null, value: null, requireNonempty: false, allowFile: DEFAULT_ALLOW_FILE };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--require-nonempty') o.requireNonempty = true;
    else if (a === '--allow-hashes-file' && i + 1 < argv.length) o.allowFile = argv[++i];
    else if (!o.mode && a === '--shas-stdin') o.mode = a;
    else if (!o.mode && ['--message-file', '--range', '--range-all'].includes(a) && i + 1 < argv.length) { o.mode = a; o.value = argv[++i]; }
    else throw new CheckFailed('arguments');
  }
  if (!o.mode) throw new CheckFailed('arguments');
  return o;
}
function identEmail(name) {
  const m = /^.* <([^<>]*)> \d+ [+-]\d+$/.exec(git(['var', name]).out.trim());
  if (!m) throw new CheckFailed('identity');
  return m[1];
}
function main() {
  const o = parseArgs(process.argv.slice(2));
  const allowedHashes = loadAllowedHashes(o.allowFile);
  let failed = false;
  const report = (who, errors) => { for (const e of errors) console.error(`${who} ${e}`); if (errors.length) failed = true; };
  if (o.mode === '--message-file') {
    const message = cleanMessage(readFileSync(o.value, 'utf8'), readCommentPrefix());
    const authorEmail = identEmail('GIT_AUTHOR_IDENT'), committerEmail = identEmail('GIT_COMMITTER_IDENT');
    const mergeHead = git(['rev-parse', '--git-path', 'MERGE_HEAD']).out.trim();
    let errors = checkAgentTrailer({ message, authorEmail, committerEmail, allowedHashes });
    if (existsSync(mergeHead)) errors = errors.filter((e) => MERGE_IN_PROGRESS_CHECKS.has(e));
    report('HEAD', errors);
    if (!failed) console.log('agent-trailer: 提交信息通过');
    return failed ? 1 : 0;
  }
  let shas;
  if (o.mode === '--shas-stdin') {
    shas = readFileSync(0, 'utf8').split('\n'); if (shas.at(-1) === '') shas.pop();
    if (!shas.every((s) => OID.test(s)) || !shas.every((s) => git(['cat-file', '-t', s], { allow: [0, 128] }).out.trim() === 'commit')) throw new CheckFailed('shas');
  } else {
    if (!RANGE.test(o.value)) throw new CheckFailed('range');
    shas = git(['rev-list', o.value, '--']).out.split('\n').filter(Boolean);
  }
  if (o.requireNonempty && shas.length === 0) throw new CheckFailed('empty');
  const results = shas.map((sha) => [sha, checkCommit(sha, allowedHashes)]);
  for (const [sha, errors] of results) report(sha.slice(0, 12), errors);
  if (!failed) console.log(`agent-trailer: ${shas.length} 个提交通过`);
  return failed ? 1 : 0;
}
if (import.meta.url === `file://${process.argv[1]}`) {
  try { process.exitCode = main(); } catch { console.error('agent-trailer: check-failed'); process.exitCode = 2; }
}
