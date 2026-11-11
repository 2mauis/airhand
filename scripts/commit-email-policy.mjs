// 提交元数据邮箱规则与提交信息清理（agent-trailer.mjs 与 privacy-scan.mjs 共用），规则见 docs/specs/commit-signature-v3.md §2、§3.3。
// 放行表只存小写邮箱的 sha256（scripts/allowed-author-hashes.json），不存邮箱明文；他人邮箱须先经项目负责人批准（D1）。
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

export const DEFAULT_ALLOW_FILE = fileURLToPath(new URL('./allowed-author-hashes.json', import.meta.url));
// 整串锚定的 GitHub noreply：可选数字 id + 登录名（不含 _ 与 .）+ 可选 [bot]
const NOREPLY = /^(?:\d+\+)?[a-z0-9](?:[a-z0-9-]{0,38})(?:\[bot\])?@users\.noreply\.github\.com$/;
const GITHUB_WEB_COMMITTER = /^noreply@github\.com$/;

export const normalizeEmail = (email) => String(email ?? '').trim().toLowerCase();
export const sha256Hex = (s) => createHash('sha256').update(String(s), 'utf8').digest('hex');

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
export function isAllowedAuthorEmail(email, allowedHashes = new Set()) {
  const e = base(email);
  return e !== null && (NOREPLY.test(e) || allowedHashes.has(sha256Hex(e)));
}
export function isAllowedCommitterEmail(email, allowedHashes = new Set()) {
  const e = base(email);
  return e !== null && (GITHUB_WEB_COMMITTER.test(e) || isAllowedAuthorEmail(e, allowedHashes));
}
// D5：附注标签与 mergetag 内嵌标签的 tagger 与 author 同一套规则
export const isAllowedTaggerEmail = isAllowedAuthorEmail;

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
