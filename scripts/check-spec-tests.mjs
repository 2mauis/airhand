#!/usr/bin/env node
// 移植自 2mauis 的 passkey-demo 项目（workflow v2，MIT，Copyright (c) 2026 2mauis），按 airhand 的测试形态改写：
// 原文只认 tests/*.mjs；airhand 的测试是 Rust / Go / Kotlin / TypeScript，所以“测试文件”的判定改为下面的 TEST_FILE，
// 并额外把 diff 新增行里的 Rust 内联测试（#[test] / #[tokio::test]）算作触及测试。其余语义与原文一致。
//
// spec-requires-tests：新增或修改 docs/specs/*.md（TEMPLATE.md、README.md、review/ 下的评审记录除外）时，
// 同一 diff 必须触及测试。删除 spec 不触发；重命名按删除 + 新增处理。
// 只检查文件集合，测试质量由评审把关。用法：node scripts/check-spec-tests.mjs BASE_SHA
import { execFileSync } from 'node:child_process';
const TEST_FILE = new RegExp([
  String.raw`^tests/[^/]+\.mjs$`, // 工作流脚本的测试
  String.raw`^(?:crates/[^/]+|server|web|client/src-tauri|android/native|p2p)/(?:tests|examples)/[^/]+\.rs$`, // Rust 集成测试
  String.raw`^client/tests/[^/]+\.test\.ts$`, // 桌面前端测试
  String.raw`(?:^|/)[^/]+_test\.go$`, // Go（p2p helper）
  String.raw`^android/app/src/test/.+\.kt$`, // Android 单元测试
].join('|'));
export function checkSpecTests(changed, addedOrModified = changed, hasInlineRustTests = false) {
  const hasSpec = addedOrModified.some((f) => /^docs\/specs\/[^/]+\.md$/.test(f) && !/^docs\/specs\/(TEMPLATE|README)\.md$/.test(f));
  const hasTest = hasInlineRustTests || changed.some((f) => TEST_FILE.test(f));
  return !hasSpec || hasTest;
}
export function addedInlineRustTests(diffText) {
  let file = '';
  for (const line of diffText.split('\n')) {
    if (line.startsWith('+++ ')) { file = line.slice(4); continue; }
    if (line.startsWith('+') && /\.rs$/.test(file) && /^\+\s*#\[(?:cfg\(test\)|test|tokio::test)[\](]/.test(line)) return true;
  }
  return false;
}
if (import.meta.url === `file://${process.argv[1]}`) {
  const [base, ...rest] = process.argv.slice(2);
  if (!base || base.startsWith('-') || rest.length) { console.error('Usage: node scripts/check-spec-tests.mjs BASE'); process.exit(1); }
  try {
    const diff = (flt) => execFileSync('git', ['diff', '--no-ext-diff', '--no-renames', '--name-only', '-z', flt, `${base}...HEAD`, '--'], { encoding: 'utf8' }).split('\0').filter(Boolean);
    const body = execFileSync('git', ['diff', '--no-ext-diff', '--no-renames', '-U0', '--diff-filter=AM', `${base}...HEAD`, '--'], { encoding: 'utf8', maxBuffer: 64 << 20 });
    if (!checkSpecTests(diff('--diff-filter=ACDMRTUXB'), diff('--diff-filter=AM'), addedInlineRustTests(body))) { console.error('spec-requires-tests: 新增或修改 docs/specs/*.md 时，同一 diff 必须触及测试（见 docs/workflow-v2.md）'); process.exitCode = 1; }
    else console.log('spec-requires-tests: passed');
  } catch { console.error('spec-requires-tests: 无法计算 BASE...HEAD 的 diff（需要完整历史）'); process.exitCode = 1; }
}
