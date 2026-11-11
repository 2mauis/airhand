// 提交元数据邮箱规则、工具表、trailer 解析、共同作者行分析与提交信息清理（agent-trailer.mjs 与 privacy-scan.mjs 共用），
// 规则见 docs/specs/commit-signature-v3.md §1.1、§1.7、§2、§3.3、§4。两个脚本只调用这里的共同作者分析，各自不得另写匹配。
// 放行表只存小写邮箱的 sha256（scripts/allowed-author-hashes.json），不存邮箱明文；他人邮箱须先经项目负责人批准（D1）。
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const DEFAULT_ALLOW_FILE = fileURLToPath(new URL('./allowed-author-hashes.json', import.meta.url));
// 整串锚定的 GitHub noreply：可选数字 id + 登录名（不含 _ 与 .）+ 可选 [bot]
const NOREPLY = /^(?:\d+\+)?[a-z0-9](?:[a-z0-9-]{0,38})(?:\[bot\])?@users\.noreply\.github\.com$/;
const GITHUB_WEB_COMMITTER = /^noreply@github\.com$/;

// 工具 bot 邮箱表（spec §2.3）：精确匹配的邮箱既放行，也是共同作者里的工具来源证据。增删须项目负责人批准并附核实来源（D7）；
// 以本地部分与域名分开存储、运行时拼接，源码里不出现完整邮箱样式。候选邮箱（spec §2.3 末尾）一律不入表。
export const TOOL_IDENTITIES = Object.freeze([
  Object.freeze({ local: 'cursoragent', domain: 'cursor.com', tool: 'cursor-agent' }),
  Object.freeze({ local: 'noreply', domain: 'anthropic.com', tool: 'claude-code' }),
  Object.freeze({ local: 'noreply', domain: 'openai.com', tool: 'codex' }),
  Object.freeze({ local: '199175422+chatgpt-codex-connector[bot]', domain: 'users.noreply.github.com', tool: 'codex' }),
]);
const TOOL_BY_EMAIL = new Map(TOOL_IDENTITIES.map((x) => [`${x.local}@${x.domain}`, x.tool]));

export const normalizeEmail = (email) => String(email ?? '').trim().toLowerCase();
export const sha256Hex = (s) => createHash('sha256').update(String(s), 'utf8').digest('hex');
export const toolOfEmail = (email) => TOOL_BY_EMAIL.get(normalizeEmail(email)) ?? null;

export function loadAllowedHashes(file = DEFAULT_ALLOW_FILE) {
  const j = JSON.parse(readFileSync(file, 'utf8'));
  if (j?.version !== 1 || !Array.isArray(j.sha256_lowercase_email)) throw new Error('allowlist-format');
  const set = new Set();
  for (const e of j.sha256_lowercase_email) {
    if (typeof e?.sha256 !== 'string' || !/^[0-9a-f]{64}$/.test(e.sha256) || set.has(e.sha256)) throw new Error('allowlist-entry');
    if (e.label !== undefined && (typeof e.label !== 'string' || e.label.includes('@'))) throw new Error('allowlist-label');
    set.add(e.sha256);
  }
  return set;
}

function base(email) {
  const e = normalizeEmail(email);
  return e.split('@').length === 2 && !/\s/.test(e) ? e : null;
}
// 邮箱类（spec §2）：先工具表，再 noreply，再 owner 哈希；不属于任何一类返回 null（example.* 也只是不属于任何一类，没有专门的拒绝规则）
export function emailClass(email, allowedHashes = new Set()) {
  const e = base(email);
  if (e === null) return null;
  if (TOOL_BY_EMAIL.has(e)) return 'tool';
  if (NOREPLY.test(e)) return 'noreply';
  if (allowedHashes.has(sha256Hex(e))) return 'owner';
  return null;
}
// author / tagger（含 mergetag 内）/ 共同作者同一套四类中的三类；GitHub 网页 committer 只对 committer 放行
export function isAllowedAuthorEmail(email, allowedHashes = new Set()) {
  return emailClass(email, allowedHashes) !== null;
}
export function isAllowedCommitterEmail(email, allowedHashes = new Set()) {
  const e = base(email);
  return e !== null && (GITHUB_WEB_COMMITTER.test(e) || isAllowedAuthorEmail(e, allowedHashes));
}
// D5：附注标签与 mergetag 内嵌标签的 tagger 与 author 同一套规则
export const isAllowedTaggerEmail = isAllowedAuthorEmail;
export const isAllowedCoauthorEmail = isAllowedAuthorEmail;

// ---- trailer 解析（spec §1.1）----
const TRAILER_LINE = /^[A-Za-z][A-Za-z0-9-]*: \S.*$/;
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

// ---- 共同作者行（spec §1.1 第 4 点、§4.4）----
// 候选前缀不区分大小写、容许行首空白与冒号前空白；合法行：名字组首尾非空白，邮箱组仅 ASCII 可打印且不含 < > @
const CO_PREFIX = /^\s*co-authored-by\s*:/i;
const CO_LINE = /^Co-authored-by: ([^<>\s](?:[^<>\n]*[^<>\s])?) <([\x21-\x3B\x3D\x3F\x41-\x7E]+@[\x21-\x3B\x3D\x3F\x41-\x7E]+)>$/i;
export const PLACEHOLDER = '<coauthor-email>';
// 重建只做模板拼接：名字组原样保留，邮箱组换成固定占位串（不得改成对整行做替换，见 spec §4.4）
export function rebuildCoauthorLine(nameGroup) {
  return `Co-authored-by: ${nameGroup} ${PLACEHOLDER}`;
}
const asBuffer = (message) => (Buffer.isBuffer(message) ? message : Buffer.from(String(message), 'utf8'));
// 单一解码下的判定：行下标 → { legal, name, email, cls }
function analyzeText(text, hashes) {
  const lines = normalizeMessage(text), block = trailerBlock(lines), out = new Map();
  lines.forEach((l, i) => {
    if (!CO_PREFIX.test(l)) return;
    const m = block.has(i) ? CO_LINE.exec(l) : null;
    out.set(i, m ? { legal: true, name: m[1], email: normalizeEmail(m[2]), cls: emailClass(m[2], hashes) } : { legal: false, name: null });
  });
  return out;
}
// 每个候选行一项 { line, status, tool, nameUtf8, nameLatin1 }；line 为原文按 \n 分行的 1 起行号。
// 候选行取 utf8 与 latin1 两种解码的并集；两种解码都合法、邮箱相同且被放行 → ok；都合法但邮箱不被放行 → email；其余一律 malformed
export function analyzeCoauthors(message, hashes = new Set()) {
  const buf = asBuffer(message);
  const u = analyzeText(buf.toString('utf8'), hashes), l = analyzeText(buf.toString('latin1'), hashes);
  return [...new Set([...u.keys(), ...l.keys()])].sort((a, b) => a - b).map((i) => {
    const a = u.get(i), b = l.get(i);
    let status = 'malformed';
    if (a?.legal && b?.legal && a.email === b.email) status = a.cls && b.cls ? 'ok' : !a.cls && !b.cls ? 'email' : 'malformed';
    else if (a?.legal && b?.legal && !a.cls && !b.cls) status = 'email';
    return { line: i + 1, status, tool: status === 'ok' && a.cls === 'tool' ? toolOfEmail(a.email) : null, nameUtf8: a?.name ?? null, nameLatin1: b?.name ?? null };
  });
}

// ---- 提交信息清理（commit-msg 钩子收到的是未清理的文件）----
const AUTO_CANDIDATES = '#;@!$%^&|:';
const DASHES = '-'.repeat(24);
export function readCommentPrefix(cwd) {
  const get = (key) => {
    try { return execFileSync('git', ['config', '--get', key], { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).replace(/\n$/, ''); } catch { return ''; }
  };
  return get('core.commentString') || get('core.commentChar') || '#';
}
export function cleanMessage(text, prefix = '#') {
  let lines = String(text).replace(/\r\n/g, '\n').split('\n');
  if (prefix === 'auto') {
    const scissors = new RegExp(`^[${AUTO_CANDIDATES.replace(/[\^\]\\-]/g, '\\$&')}] ${DASHES} >8 ${DASHES}$`);
    const cut = lines.findIndex((l) => scissors.test(l)); if (cut >= 0) lines = lines.slice(0, cut);
    let end = lines.length; while (end > 0 && lines[end - 1].trim() === '') end--;
    const c = end > 0 ? lines[end - 1][0] : undefined;
    if (c && AUTO_CANDIDATES.includes(c)) {
      let start = end; while (start > 0 && (lines[start - 1].trim() === '' || lines[start - 1].startsWith(c))) start--;
      lines = lines.slice(0, start);
    }
    return lines.join('\n');
  }
  const cut = lines.indexOf(`${prefix} ${DASHES} >8 ${DASHES}`); if (cut >= 0) lines = lines.slice(0, cut);
  return lines.filter((l) => !l.startsWith(prefix)).join('\n');
}
// 按字节清理（--message-file）：latin1 与字节一一对应，清理后原样还原成字节，供两种解码的共同作者分析使用
export function cleanMessageBytes(buf, prefix = '#') {
  const p = prefix === 'auto' ? prefix : Buffer.from(prefix, 'utf8').toString('latin1');
  return Buffer.from(cleanMessage(Buffer.from(buf).toString('latin1'), p), 'latin1');
}
