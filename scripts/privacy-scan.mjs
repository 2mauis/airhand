#!/usr/bin/env node
// 移植自 2mauis 的 passkey-demo 项目（workflow v2，MIT，Copyright (c) 2026 2mauis）；airhand 增加了 go.sum 窄例外与
// 署名规范 v3 的提交 / 标签对象扫描（docs/specs/commit-signature-v3.md §4）；许可见 docs/workflow-v2.md「来源与许可」。
// 隐私规则扫描（确定性正则，不调用任何外部服务）。
// 用法：
//   node scripts/privacy-scan.mjs                扫描 git ls-files 列出的全部已跟踪文本文件
//   node scripts/privacy-scan.mjs --diff BASE    只扫 BASE...HEAD 新增行
//   node scripts/privacy-scan.mjs --files a b    扫指定文件
//   node scripts/privacy-scan.mjs --stdin        扫标准输入（如 PR 正文）
//   node scripts/privacy-scan.mjs (--commits BASE..HEAD | --shas-stdin | --message-file FILE) [--require-nonempty] [--allow-hashes-file FILE]
//                                                扫提交 / 标签对象的原始字节（元数据与信息），或钩子收到的提交信息文件
// 可选 PRIVACY_WORDLIST=<不入库的词表文件>，每行一个禁止出现的词（大小写不敏感）。
// 命中时只输出 文件:行号（或 sha12:字段[:行号]）与规则名，不输出命中值本身。正则不能发现任意姓名或自然语言地址，仍需人工审查。
// 退出码：0 无命中；1 有命中；2 检查本身失败（仅对象扫描模式）。
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, existsSync } from 'node:fs';
import { isAllowedAuthorEmail, isAllowedCommitterEmail, isAllowedTaggerEmail, loadAllowedHashes, readCommentPrefix, cleanMessage, DEFAULT_ALLOW_FILE } from './commit-email-policy.mjs';

const ALLOW_EMAIL = /(@users\.noreply\.github\.com|@example\.(com|org|net)|@localhost|^git@github\.com)$/i;
const DOC_IP = /^(192\.0\.2\.|198\.51\.100\.|203\.0\.113\.|127\.|0\.0\.0\.0$|255\.255\.255\.)/;
export const RULES = [
  { id: 'email', re: /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g, ok: (m) => ALLOW_EMAIL.test(m) },
  { id: 'ipv4', re: /\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b/g, ok: (m) => DOC_IP.test(m) },
  { id: 'workers-dev-subdomain', re: /\b[a-z0-9-]+\.[a-z0-9-]+\.workers\.dev\b/gi, ok: (m) => /\.example\.workers\.dev$|^<[^>]*>/i.test(m) || /\.(your-subdomain|example)\.workers\.dev$/i.test(m) },
  { id: 'kv-or-account-id', re: /\b[0-9a-f]{32}\b/g, ok: (m) => /^0+$/.test(m) },
  { id: 'turnstile-sitekey', re: /\b0x4[A-Za-z0-9_-]{18,}\b/g, ok: () => false },
  { id: 'private-key', re: /-----BEGIN [A-Z ]*PRIVATE KEY-----/g, ok: () => false },
  { id: 'github-token', re: /\b(gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,})\b/g, ok: () => false },
  { id: 'cloudflare-token-like', re: /\b[A-Za-z0-9_-]{40}\b/g, ok: (m) => !/[0-9]/.test(m) || !/[A-Z]/.test(m) || !/[a-z]/.test(m) },
  { id: 'slack-token', re: /\bxox[abprs]-[A-Za-z0-9-]{10,}\b/g, ok: () => false },
  { id: 'aws-key', re: /\bAKIA[0-9A-Z]{16}\b/g, ok: () => false },
  { id: 'tailnet-or-lan-host', re: /(?<![\w.])[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(?:ts\.net|internal|home\.arpa)\b|(?<=\/\/|@)[a-z0-9-]+\.(?:local|lan)\b/gi, ok: () => false },
];
const words = process.env.PRIVACY_WORDLIST && existsSync(process.env.PRIVACY_WORDLIST)
  ? readFileSync(process.env.PRIVACY_WORDLIST, 'utf8').split('\n').map((s) => s.trim()).filter(Boolean) : [];
const SKIP_DIRS = new Set(['node_modules', '.wrangler']);
const BINARY_EXT = /\.(png|jpe?g|gif|webp|ico|woff2?)$/i;
// 按路径段判断，避免未锚定的正则分支误跳相似文件名（如 my-node_modules-notes.md）
export function shouldSkip(file) {
  const parts = String(file).split('/');
  return parts.slice(0, -1).some((p) => SKIP_DIRS.has(p)) || parts[parts.length - 1] === 'package-lock.json' || BINARY_EXT.test(file);
}

// 窄例外：只对文件名为 go.sum 的路径、且整行是 Go 模块校验和形状（`<module> <version>[/go.mod] h1:<base64>=`）的行，
// 豁免 cloudflare-token-like 这一条规则（模块哈希恰好含 40 位字母数字串会误报）；其他规则、其他文件、其他形状的行不放宽。
const GOSUM_LINE = /^\S+ v[^\s/]+(?:\/go\.mod)? h1:[A-Za-z0-9+/]{43}=$/;
export const isGoSumChecksumLine = (where, line) => String(where).split('/').pop() === 'go.sum' && GOSUM_LINE.test(line.replace(/\r$/, ''));

export function scanText(text, where, { honorAllow = true } = {}) {
  const hits = [];
  text.split('\n').forEach((line, i) => {
    if (honorAllow && /privacy-scan:\s*allow/.test(line)) return; // 显式豁免（需在 PR 中说明理由） // nosemgrep: ajinabraham.njsscan.dos.regex_dos.regex_dos -- 固定字面量正则，无嵌套量词
    const goSumLine = isGoSumChecksumLine(where, line);
    for (const r of RULES) { if (goSumLine && r.id === 'cloudflare-token-like') continue; for (const m of line.matchAll(r.re)) if (!r.ok(m[0])) hits.push(`${where}:${i + 1} ${r.id}`); }
    for (const w of words) if (line.toLowerCase().includes(w.toLowerCase())) hits.push(`${where}:${i + 1} wordlist`);
  });
  return hits;
}
const git = (...a) => execFileSync('git', a, { encoding: 'utf8', maxBuffer: 64 << 20 });

// ---- 提交 / 标签对象扫描（署名规范 v3 §4.1、§4.1a）----
class CheckFailed extends Error {}
const OID = /^(?:[0-9a-f]{40}|[0-9a-f]{64})$/;
const RANGE = /^[^\s.-][^\s]*(?<!\.)\.\.(?!\.)[^\s]+$/;
const IDENT = /^([^<>\n]*) <([^<>\n]*)> \d+ [+-]\d{4}$/;
const ARMOR_BEGIN = /^-----BEGIN (PGP SIGNATURE|SSH SIGNATURE|SIGNED MESSAGE)-----$/;
const BASE64_LINE = /^[A-Za-z0-9+/]+={0,2}$/, CHECKSUM_LINE = /^=[A-Za-z0-9+/]{4}$/;
function gitRaw(args, allow = [0]) {
  const r = spawnSync('git', args, { maxBuffer: 64 << 20, stdio: ['ignore', 'pipe', 'pipe'] });
  if (r.error || !allow.includes(r.status)) throw new CheckFailed(`git ${args[0]}`);
  return r;
}
// 只在 gpgsig / gpgsig-sha256 / mergetag 的值与标签说明里调用：取最后一个 BEGIN，END 必须同类型且是最后一个非空行；
// 满足时只跳过其间的纯 base64 行与校验和行（及 BEGIN / END 本身），其余照扫；不满足时整段照扫
export function armorSkip(lines) {
  const skip = new Set();
  let begin = -1; lines.forEach((l, i) => { if (ARMOR_BEGIN.test(l)) begin = i; });
  if (begin < 0) return skip;
  let end = lines.length - 1; while (end >= 0 && lines[end].trim() === '') end--;
  if (end <= begin || lines[end] !== `-----END ${ARMOR_BEGIN.exec(lines[begin])[1]}-----`) return skip;
  skip.add(begin); skip.add(end);
  for (let i = begin + 1; i < end; i++) if (BASE64_LINE.test(lines[i]) || CHECKSUM_LINE.test(lines[i])) skip.add(i);
  return skip;
}
// 头部在第一个空行之前；以一个空格开头的行是上一个头的续行（去掉一个前导空格）
export function parseObject(text) {
  const lines = text.split('\n');
  let h = lines.indexOf(''); if (h < 0) h = lines.length;
  const headers = [];
  for (let i = 0; i < h; i++) {
    const l = lines[i];
    if (l.startsWith(' ') && headers.length) { headers[headers.length - 1].lines.push(l.slice(1)); continue; }
    const sp = l.indexOf(' ');
    headers.push({ key: sp < 0 ? l : l.slice(0, sp), lines: [sp < 0 ? '' : l.slice(sp + 1)], line: i + 1 });
  }
  return { headers, body: lines.slice(h + 1), bodyLine: h + 2 };
}
const ruleIds = (line) => scanText(line, '', { honorAllow: false }).map((h) => h.slice(h.lastIndexOf(' ') + 1));
function scanLines(hits, id12, field, lines, firstLine, skip = new Set()) {
  lines.forEach((l, i) => { if (!skip.has(i)) for (const r of ruleIds(l)) hits.add(`${id12}:${field}:${firstLine + i} ${r}`); });
}
const singleHex = (h) => h.lines.length === 1 && OID.test(h.lines[0]);
function scanIdent(hits, id12, h, offset, { field, nameField, emailRule, malformedRule, allowed }) {
  const m = h.lines.length === 1 ? IDENT.exec(h.lines[0]) : null;
  if (!m) { hits.add(`${id12}:${field} ${malformedRule}`); return; }
  if (!allowed(m[2])) hits.add(`${id12}:${field} ${emailRule}`);
  scanLines(hits, id12, nameField, [m[1]], offset + h.line);
}
const TAG_CTX = { header: 'tag-header', tagger: 'tagger', taggerName: 'tagger-name', message: 'tag-message', emailRule: 'tag-meta-email', malformedRule: 'tag-ident-malformed' };
const MERGETAG_CTX = { header: 'mergetag', tagger: 'mergetag', taggerName: 'mergetag', message: 'mergetag', emailRule: 'commit-meta-email', malformedRule: 'commit-ident-malformed' };
// 标签对象（或 mergetag 内嵌的标签全文）：tagger 恰好一行（缺少或重复 → *-ident-malformed），邮箱按 author 同一套规则（D5）
function scanTag(hits, id12, text, offset, ctx, hashes) {
  const { headers, body, bodyLine } = parseObject(text);
  if (headers.filter((h) => h.key === 'tagger').length !== 1) hits.add(`${id12}:${ctx.tagger} ${ctx.malformedRule}`);
  let next = null;
  for (const h of headers) {
    if (h.key === 'object' && singleHex(h)) { next = h.lines[0]; continue; }
    if (h.key === 'type' && h.lines.length === 1 && /^[a-z]+$/.test(h.lines[0])) continue;
    if (h.key === 'tagger') {
      scanIdent(hits, id12, h, offset, { field: ctx.tagger, nameField: ctx.taggerName, emailRule: ctx.emailRule, malformedRule: ctx.malformedRule, allowed: (e) => isAllowedTaggerEmail(e, hashes) });
      continue;
    }
    scanLines(hits, id12, ctx.header, [`${h.key} ${h.lines[0]}`, ...h.lines.slice(1)], offset + h.line);
  }
  scanLines(hits, id12, ctx.message, body, offset + bodyLine, armorSkip(body));
  const type = headers.find((h) => h.key === 'type')?.lines[0];
  return type === 'tag' ? next : null;
}
function scanCommit(hits, id12, text, hashes) {
  const { headers, body, bodyLine } = parseObject(text);
  for (const role of ['author', 'committer']) if (headers.filter((h) => h.key === role).length !== 1) hits.add(`${id12}:${role} commit-ident-malformed`);
  for (const h of headers) {
    if ((h.key === 'tree' || h.key === 'parent') && singleHex(h)) continue;
    if (h.key === 'author' || h.key === 'committer') {
      const allowed = h.key === 'author' ? (e) => isAllowedAuthorEmail(e, hashes) : (e) => isAllowedCommitterEmail(e, hashes);
      scanIdent(hits, id12, h, 0, { field: h.key, nameField: `${h.key}-name`, emailRule: 'commit-meta-email', malformedRule: 'commit-ident-malformed', allowed });
    } else if (h.key === 'gpgsig' || h.key === 'gpgsig-sha256') scanLines(hits, id12, h.key, h.lines, h.line, armorSkip(h.lines));
    else if (h.key === 'mergetag') scanTag(hits, id12, h.lines.join('\n'), h.line - 1, MERGETAG_CTX, hashes);
    else scanLines(hits, id12, 'header', [`${h.key} ${h.lines[0]}`, ...h.lines.slice(1)], h.line);
  }
  scanLines(hits, id12, 'message', body, bodyLine);
}
// 同一对象按 utf8 与 latin1 各解码一遍（非 UTF-8 的 encoding 头不漏匹配），命中取并集；返回标签链的下一层对象
function scanObject(hits, id, type, hashes) {
  const buf = gitRaw(['cat-file', type, id]).stdout;
  let next = null;
  for (const enc of ['utf8', 'latin1']) {
    const text = buf.toString(enc);
    if (type === 'commit') scanCommit(hits, id.slice(0, 12), text, hashes);
    else next = scanTag(hits, id.slice(0, 12), text, 0, TAG_CTX, hashes);
  }
  return next;
}
function objectModes(argv) {
  const o = { mode: null, value: null, requireNonempty: false, allowFile: DEFAULT_ALLOW_FILE };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--require-nonempty') o.requireNonempty = true;
    else if (a === '--allow-hashes-file' && i + 1 < argv.length) o.allowFile = argv[++i];
    else if (!o.mode && a === '--shas-stdin') o.mode = a;
    else if (!o.mode && (a === '--commits' || a === '--message-file') && i + 1 < argv.length) { o.mode = a; o.value = argv[++i]; }
    else throw new CheckFailed('arguments');
  }
  const hashes = loadAllowedHashes(o.allowFile);
  const hits = new Set();
  let count = 0;
  if (o.mode === '--message-file') {
    const text = cleanMessage(readFileSync(o.value, 'utf8'), readCommentPrefix());
    for (const h of scanText(text, 'HEAD:message', { honorAllow: false })) hits.add(h);
    count = 1;
  } else {
    let queue;
    if (o.mode === '--shas-stdin') {
      queue = readFileSync(0, 'utf8').split('\n'); if (queue.at(-1) === '') queue.pop();
      if (!queue.every((s) => OID.test(s))) throw new CheckFailed('shas');
    } else {
      if (!RANGE.test(o.value)) throw new CheckFailed('range');
      queue = gitRaw(['rev-list', o.value, '--']).stdout.toString('utf8').split('\n').filter(Boolean);
    }
    const n = queue.length;
    if (o.requireNonempty && n === 0) throw new CheckFailed('empty');
    const seen = new Set();
    for (let i = 0; i < queue.length; i++) {
      const id = queue[i]; if (seen.has(id)) continue; seen.add(id);
      const type = gitRaw(['cat-file', '-t', id], [0, 128]).stdout.toString('utf8').trim();
      if (type !== 'commit' && !(type === 'tag' && o.mode === '--shas-stdin')) throw new CheckFailed('object-type');
      const next = scanObject(hits, id, type, hashes); count++;
      if (next) queue.push(next);
    }
    if (o.mode === '--commits' && count !== n) throw new CheckFailed('count');
  }
  for (const h of hits) console.error(h);
  console.log(`privacy-scan: 已扫描 ${count} 个对象`);
  console.log(`privacy-scan: ${hits.size} 处命中`);
  return hits.size ? 1 : 0;
}

function main() {
  const [mode, ...rest] = process.argv.slice(2);
  if (['--commits', '--shas-stdin', '--message-file'].includes(mode)) {
    try { process.exitCode = objectModes(process.argv.slice(2)); } catch { console.error('privacy-scan: check-failed'); process.exitCode = 2; }
    return;
  }
  let hits = [];
  if (mode === '--stdin') hits = scanText(readFileSync(0, 'utf8'), 'stdin');
  else if (mode === '--files') for (const f of rest) hits.push(...scanText(readFileSync(f, 'utf8'), f));
  else if (mode === '--diff') {
    let file = '', ln = 0;
    for (const line of git('diff', '--no-ext-diff', '-U0', `${rest[0]}...HEAD`).split('\n')) {
      if (line.startsWith('+++ ')) { file = line.slice(6); continue; }
      const h = /^@@ -\S+ \+(\d+)/.exec(line); if (h) { ln = Number(h[1]); continue; }
      if (line.startsWith('+') && !shouldSkip(file)) { hits.push(...scanText(line.slice(1), `${file}`).map((x) => x.replace(/:1 /, `:${ln} `))); ln++; }
    }
  } else {
    for (const f of git('ls-files', '-z').split('\0').filter(Boolean)) {
      if (shouldSkip(f) || !existsSync(f)) continue;
      const buf = readFileSync(f); if (buf.includes(0)) continue;
      hits.push(...scanText(buf.toString('utf8'), f));
    }
  }
  for (const h of hits) console.error(h);
  console.log(`privacy-scan: ${hits.length} 处命中`);
  process.exitCode = hits.length ? 1 : 0;
}
if (import.meta.url === `file://${process.argv[1]}`) main();
