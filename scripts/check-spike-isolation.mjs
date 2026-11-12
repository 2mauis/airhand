#!/usr/bin/env node
// spike 隔离检查（docs/specs/tailcat-p2p.md OQ-23d）：spike/ 下的测量代码不得被产品代码复用。
//   (a) 各 Cargo 工作区的 Cargo.toml 里，workspace members / path 依赖不得指向 spike/；
//   (b) p2p/go.mod 与 go.work（若有）的 replace / use 不得引用 spike/，p2p/ 的 Go import 不得引用 spike；
//   (c) crates/、server/、client/、p2p/ 的源码里（注释除外）不得出现 "spike/"。
// 用法：node scripts/check-spike-isolation.mjs（在仓库根目录运行；只读文件，不需要网络）
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve, dirname, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SKIP = new Set(['target', 'node_modules', '.git', 'dist', 'build', '.gradle']);
const SRC = /\.(?:rs|go|ts|tsx|js|mjs|kt|toml|json)$/;

function walk(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const d of readdirSync(dir)) {
    if (SKIP.has(d)) continue;
    const p = join(dir, d);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

const rel = (p) => relative(ROOT, p).split(sep).join('/');
const stripComment = (line, file) => {
  if (/\.toml$/.test(file)) return line.replace(/#.*$/, '');
  if (/\.json$/.test(file)) return line;
  return line.replace(/\/\/.*$/, '').replace(/^\s*\*.*$/, '');
};

export function scanFile(file, text) {
  const hits = [];
  let block = false;
  text.split('\n').forEach((raw, i) => {
    let line = raw;
    if (!/\.(?:toml|json)$/.test(file)) {
      if (block) { const end = line.indexOf('*/'); if (end < 0) return; line = line.slice(end + 2); block = false; }
      line = line.replace(/\/\*.*?\*\//g, '');
      const start = line.indexOf('/*');
      if (start >= 0) { line = line.slice(0, start); block = true; }
    }
    line = stripComment(line, file);
    const spike = /(?:^|[\s"'`(=:/.])spike\//.test(line) || (/\.go$/.test(file) && /"[^"]*\/spike(?:\/[^"]*)?"/.test(line));
    if (spike) hits.push(`${file}:${i + 1}: ${raw.trim()}`);
  });
  return hits;
}

function main() {
  const hits = [];
  const manifests = ['Cargo.toml', 'client/src-tauri/Cargo.toml', 'android/native/Cargo.toml', 'web/Cargo.toml', 'p2p/go.mod', 'go.work'];
  for (const m of manifests) {
    const p = join(ROOT, m);
    if (existsSync(p)) hits.push(...scanFile(m, readFileSync(p, 'utf8')));
  }
  for (const dir of ['crates', 'server', 'client', 'p2p']) {
    for (const f of walk(join(ROOT, dir))) {
      if (!SRC.test(f)) continue;
      hits.push(...scanFile(rel(f), readFileSync(f, 'utf8')));
    }
  }
  const unique = [...new Set(hits)];
  if (unique.length) {
    for (const h of unique) console.error(`spike-isolation: 产品代码引用了 spike/：${h}`);
    return 1;
  }
  console.log('spike-isolation: passed（Cargo 工作区、Go 模块与 crates/ server/ client/ p2p/ 源码均未引用 spike/）');
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) process.exitCode = main();
