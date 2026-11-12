#!/usr/bin/env node
// tailcat-p2p 先红门禁（docs/specs/tailcat-p2p.md「先红验收测试清单」#98、#99、#139）。
//
// 预期红名单 tests/p2p-expected-red.txt = 当前应为红的全部用例（A 级尚未实现的部分，加上已并入、尚未实现的 B 级）。
// 门禁解析 `cargo test --no-fail-fast` 的文本输出与 `go test -json`，要求：
//   - 常规作业：失败集合恰好等于名单（在本次输入覆盖的工作区 / 模块范围内；多一条意外失败、少一条意外通过都报错）；
//   - 空操作作业（--noop lenient|strict）：登记表 tests/p2p-a-registry.txt 里该极性“适用”的 A 级非元用例必须失败；
//     “不适用”的用例若仍在名单里（尚未实现）可红可绿，已不在名单里的必须通过；元用例必须通过；其余用例同常规作业；
//   - 每个失败都必须是断言失败：Rust 的 panic 信息以 `assertion ` 或 `ASSERT:` 开头，Go 的输出含 `ASSERT:` 或
//     `Error Trace:`；含“未实现”字样、构建 / 初始化失败、非断言 panic、无标记的超时一律判“错误原因的红”；
//   - 桩源码里没有 todo!() / unimplemented!()。
// 每次评估之前先跑 #139 的十一个合成场景自检（另加 (12)：ANSI 颜色码的对照与反例）（--self-test 只跑自检）。
//
// 用法：
//   node tests/p2p-expected-red.mjs [--list FILE] [--registry FILE] [--rust [WS=]LOG]... [--go [MOD=]JSON]...
//        [--noop lenient|strict] [--write-result FILE] [--print-failures]
//   node tests/p2p-expected-red.mjs --self-test | --check-registry | --check-stubs
// WS 是 cargo 工作区目录（默认 .），MOD 是 Go 模块目录（默认 p2p），都相对仓库根目录。
import { readFileSync, writeFileSync, existsSync, readdirSync, statSync, mkdirSync } from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const LIST = 'tests/p2p-expected-red.txt';
const REGISTRY = 'tests/p2p-a-registry.txt';
export const META = new Set([
  'rust crates/airhand-link/tests/direct.rs::negative_assertions_fail_on_noop_driver',
  'go p2p/gate_test.go::TestNegativeAssertionsFailOnNoopGate',
  'node tests/p2p-expected-red.mjs::expected_red_gate_rejects_unexpected_failures_passes_and_wrong_reason_red',
]);
const LEVELS = /^(?:#\d+|OQ-27[a-z]) (?:A|A PV-[123]|元用例)$/;
const ID = /^(rust|go|node) ([^\s:]+)::(\S+)$/;
const BAD = [/not (?:yet )?implemented/i, /\bNotWired\b/, /\bErrNotImplemented\b/, /\[build failed\]/, /\[setup failed\]/, /cannot find module/, /test timed out/];
export const STUB_FILES = ['crates/airhand-link/src', 'crates/airhand-proto/src', 'client/src-tauri/src/direct_config.rs', 'p2p'];
const STUB_BAD = /\btodo!\s*\(|\bunimplemented!\s*\(|panic\("not implemented/;

// ---------- 名单与登记表 ----------

export function parseList(text, name = LIST) {
  const errs = []; const ids = [];
  text.split('\n').forEach((raw, i) => {
    const line = raw.trimEnd();
    if (!line || line.startsWith('#')) return;
    if (!ID.test(line)) errs.push(`${name}:${i + 1}: 格式应为 "<rust|go|node> <路径>::<用例名>"（不允许行尾注释）：${line}`);
    else ids.push(line);
  });
  ids.forEach((id, i) => { if (i && ids[i - 1] >= id) errs.push(`${name}: 须按字节序排序且不重复：${ids[i - 1]} / ${id}`); });
  return { ids, errs };
}

export function parseRegistry(text, name = REGISTRY) {
  const errs = []; const entries = new Map();
  text.split('\n').forEach((raw, i) => {
    const line = raw.trimEnd();
    if (!line || line.startsWith('#')) return;
    const f = line.split(' | ');
    const where = `${name}:${i + 1}`;
    if (f.length !== 6) { errs.push(`${where}: 应有 6 个字段（用例 | 编号 级别 | 驱动 | lenient= | strict= | 替代正向对照）：${line}`); return; }
    const [id, level, driver, len, str, alt] = f;
    if (!ID.test(id)) errs.push(`${where}: 用例名格式错误：${id}`);
    if (!LEVELS.test(level)) errs.push(`${where}: 编号与级别应为 "#n A"、"#n A PV-1|2|3" 或 "#n 元用例"：${level}`);
    if (!driver || driver === '-') errs.push(`${where}: 没有驱动声明`);
    const pol = (s, p) => { const m = s.match(new RegExp(`^${p}=(适用|不适用)$`)); if (!m) errs.push(`${where}: 应为 ${p}=适用|不适用：${s}`); return m?.[1] === '适用'; };
    const lenient = pol(len, 'lenient'); const strict = pol(str, 'strict');
    const meta = level.endsWith('元用例');
    if (meta !== META.has(id)) errs.push(`${where}: 元用例例外名单只能是 #98、#99、#139 三条：${id}`);
    const needsAlt = !meta && (!lenient || !strict);
    if (needsAlt && !/^常规作业:/.test(alt)) errs.push(`${where}: 标“不适用”须写替代正向对照（常规作业:<用例>）`);
    if (!needsAlt && alt !== '-') errs.push(`${where}: 两种极性都适用时替代正向对照写 -：${alt}`);
    if (meta && (lenient || strict)) errs.push(`${where}: 元用例两种极性都应为“不适用”`);
    if (entries.has(id)) errs.push(`${where}: 重复登记：${id}`);
    entries.set(id, { id, level, driver, lenient, strict, meta, alt: needsAlt ? alt.slice('常规作业:'.length) : null });
  });
  return { entries, errs };
}

// 名单与登记表之间、以及与源码之间的一致性（不依赖测试结果）
export function checkConsistency(list, registry, sourceHas = defaultSourceHas) {
  const errs = [];
  for (const id of list) {
    const e = registry.get(id);
    if (e?.meta) errs.push(`名单里不得有元用例：${id}`);
    if (!sourceHas(id)) errs.push(`名单里的用例在源码中找不到：${id}`);
  }
  for (const e of registry.values()) {
    if (!sourceHas(e.id)) errs.push(`登记表里的用例在源码中找不到：${e.id}`);
    if (e.alt && !sourceHas(e.alt)) errs.push(`替代正向对照在源码中找不到：${e.id} → ${e.alt}`);
  }
  return errs;
}

function defaultSourceHas(id) {
  const m = id.match(ID); if (!m) return false;
  const [, lang, path, name] = m; const file = join(ROOT, path);
  if (!existsSync(file)) return false;
  const src = readFileSync(file, 'utf8'); const fn = name.split('::').pop();
  if (lang === 'rust') return new RegExp(`\\bfn ${fn}\\s*\\(`).test(src);
  if (lang === 'go') return new RegExp(`\\bfunc ${fn}\\s*\\(`).test(src);
  return src.includes(fn);
}

// ---------- 测试输出解析 ----------

// CI 里 CARGO_TERM_COLOR=always 会让 cargo 的 `Running …` 等行带 ANSI 颜色码；解析之前先去掉（只去 SGR：ESC [ … m）。
// go test -json 的输出字段里若夹带颜色码，JSON 里是 \u001b 转义，所以解析后的 Output 也要去一遍。
const ANSI_SGR = /\x1b\[[0-9;]*m/g;
export const stripAnsi = (t) => String(t ?? '').replace(ANSI_SGR, '');

// cargo test 的文本输出 → Map<id, {status, message}>；resolveRust(rel, bin) 把“Running <rel> (deps/<bin>-hash)”映射到仓库内路径
export function parseCargo(text, resolveRust = (rel) => rel) {
  text = stripAnsi(text);
  const results = new Map(); const fatal = [];
  let file = null; let block = null; const slow = new Set();
  const flush = () => {
    if (!block) return;
    const r = results.get(`rust ${block.file}::${block.name}`);
    if (r) r.message = panicMessage(block.lines, block.name);
    block = null;
  };
  for (const line of text.split('\n')) {
    let m;
    if ((m = line.match(/^\s+Running (?:unittests )?(\S+) \(.*?deps[\\/]([^\\/]+?)(?:-[0-9a-f]+)?(?:\.exe)?\)$/))) { flush(); file = resolveRust(m[1], m[2]); continue; }
    if ((m = line.match(/^\s+Doc-tests (\S+)$/))) { flush(); file = `doc:${m[1]}`; continue; }
    if (/^error(?:\[E\d+\])?: could not compile|^error\[E\d+\]/.test(line)) fatal.push(`编译失败：${line.trim()}`);
    if (!file) continue;
    if ((m = line.match(/^test (.+?) \.\.\. (ok|FAILED|ignored)(?:,.*)?$/))) {
      results.set(`rust ${file}::${m[1]}`, { status: m[2] === 'ok' ? 'pass' : m[2] === 'FAILED' ? 'fail' : 'ignored', message: '' });
      continue;
    }
    if ((m = line.match(/^test (.+?) has been running for over \d+ seconds$/))) { slow.add(`rust ${file}::${m[1]}`); continue; }
    if ((m = line.match(/^---- (.+?) stdout ----$/))) { flush(); block = { file, name: m[1], lines: [] }; continue; }
    if (block && line === 'failures:') { flush(); continue; }
    if (block) block.lines.push(line);
  }
  flush();
  for (const id of slow) if (!results.has(id)) results.set(id, { status: 'fail', message: '测试超时（没有结果）', timeout: true });
  return { results, fatal };
}

function panicMessage(lines, name) {
  let first = null;
  for (let i = 0; i < lines.length; i++) {
    const m = lines[i].match(/^thread '(.+?)'(?: \(\d+\))? panicked at .+:$/);
    if (!m) continue;
    const msg = lines.slice(i + 1).find((l) => l.trim() !== '') ?? '';
    if (m[1] === name || m[1].endsWith(`::${name}`)) return { msg, all: lines.join('\n') };
    first ??= { msg, all: lines.join('\n') };
  }
  return first ?? { msg: '', all: lines.join('\n') };
}

// go test -json → Map<id, {status, message}>；resolveGo(pkg, test) 返回定义该测试的 _test.go 的仓库内路径
export function parseGoJson(text, resolveGo = (pkg, test) => `${pkg}/?_test.go`) {
  text = stripAnsi(text);
  const results = new Map(); const fatal = []; const out = new Map(); const pkgFail = new Set(); const pkgOut = new Map();
  for (const raw of text.split('\n')) {
    if (!raw.trim()) continue;
    let ev; try { ev = JSON.parse(raw); } catch { fatal.push(`go test -json 输出里有非 JSON 行：${raw.slice(0, 120)}`); continue; }
    if (typeof ev.Output === 'string') ev.Output = stripAnsi(ev.Output);
    const top = ev.Test?.split('/')[0];
    if (ev.Action === 'output') {
      if (top) { const k = `${ev.Package}\0${top}`; out.set(k, (out.get(k) ?? '') + ev.Output); }
      else pkgOut.set(ev.Package, (pkgOut.get(ev.Package) ?? '') + (ev.Output ?? ''));
      continue;
    }
    if (ev.Test && ev.Test.includes('/')) continue;
    if (['pass', 'fail', 'skip'].includes(ev.Action)) {
      if (!ev.Test) { if (ev.Action === 'fail') pkgFail.add(ev.Package); continue; }
      results.set(`${ev.Package}\0${ev.Test}`, ev.Action === 'pass' ? 'pass' : ev.Action === 'fail' ? 'fail' : 'ignored');
    }
  }
  const final = new Map();
  for (const [k, status] of results) {
    const [pkg, test] = k.split('\0');
    final.set(`go ${resolveGo(pkg, test)}::${test}`, { status, message: { msg: out.get(k) ?? '', all: out.get(k) ?? '' } });
  }
  for (const pkg of pkgFail) {
    const o = pkgOut.get(pkg) ?? '';
    const anyFail = [...results].some(([k, s]) => k.startsWith(`${pkg}\0`) && s === 'fail');
    if (BAD.some((re) => re.test(o)) || /^panic: /m.test(o) || !anyFail) fatal.push(`Go 包 ${pkg} 在用例之外失败：${o.trim().split('\n').slice(-3).join(' / ')}`);
  }
  return { results: final, fatal };
}

// 失败是否为“对的红”：返回 null 表示是断言失败，否则返回原因
export function wrongReason(lang, r) {
  if (r.timeout) return '测试超时（没有断言标记）';
  const { msg = '', all = '' } = r.message || {};
  const bad = BAD.find((re) => re.test(all) || re.test(msg));
  if (bad) return `输出含错误原因模式 ${bad}`;
  if (lang === 'rust') return /^(?:assertion |ASSERT:)/.test(msg) ? null : `panic 信息不以 "assertion " 或 "ASSERT:" 开头：${msg.slice(0, 160) || '（没有 panic 信息）'}`;
  if (lang === 'go') {
    if (/^panic: /m.test(all)) return 'Go panic（非断言失败）';
    return /ASSERT:|Error Trace:/.test(all) ? null : `输出里没有 "ASSERT:" 或 "Error Trace:"：${all.trim().split('\n').slice(-2).join(' / ').slice(0, 160)}`;
  }
  return null;
}

export function scanStubs(files) {
  const errs = [];
  for (const [path, src] of files) {
    src.split('\n').forEach((line, i) => {
      if (/^\s*(?:\/\/|\*)/.test(line)) return;
      if (STUB_BAD.test(line)) errs.push(`桩不得使用 todo!() / unimplemented!() / panic("not implemented")：${path}:${i + 1}: ${line.trim()}`);
    });
  }
  return errs;
}

// ---------- 判定 ----------

// results: Map<id, {status, message}>；inScope(id) 判断名单 / 登记表条目是否属于本次输入覆盖的范围
export function evaluate({ results, list, registry = new Map(), noop = null, inScope = () => true }) {
  const errs = []; const listSet = new Set(list);
  for (const [id, r] of results) {
    if (r.status !== 'fail') continue;
    const why = wrongReason(id.split(' ')[0], r);
    if (why) errs.push(`错误原因的红：${id}：${why}`);
  }
  const expectFail = (id) => {
    const e = registry.get(id);
    if (!noop || !e) return listSet.has(id) ? 'fail' : 'pass';
    if (e.meta) return 'pass';
    if (e[noop]) return 'fail';
    return listSet.has(id) ? 'either' : 'pass';
  };
  const ids = new Set([...results.keys(), ...[...listSet].filter(inScope), ...(noop ? [...registry.keys()].filter((id) => inScope(id) && !registry.get(id).meta && !id.startsWith('node ')) : [])]);
  for (const id of [...ids].sort()) {
    const r = results.get(id); const want = expectFail(id);
    if (!r) { errs.push(`${listSet.has(id) ? '名单' : '登记表'}里的用例没有运行（编译失败、被过滤或路径不对）：${id}`); continue; }
    if (r.status === 'ignored') { if (want !== 'pass') errs.push(`应为红的用例被忽略：${id}`); continue; }
    if (want === 'fail' && r.status === 'pass') errs.push(noop && !listSet.has(id) ? `空操作（${noop}）下“适用”的用例通过了：${id}` : `意外通过（已变绿，请从名单删除）：${id}`);
    if (want === 'pass' && r.status === 'fail') errs.push(noop && registry.has(id) && !listSet.has(id) ? `空操作（${noop}）下“不适用”的用例失败：${id}` : `意外失败（不在名单里）：${id}`);
  }
  if (!noop) {
    for (const e of registry.values()) {
      if (!e.alt || !inScope(e.alt)) continue;
      const r = results.get(e.alt);
      if (!r) errs.push(`替代正向对照没有在常规作业里运行：${e.id} → ${e.alt}`);
      else if (r.status !== 'pass' && !listSet.has(e.alt)) errs.push(`替代正向对照在常规作业里没有通过：${e.id} → ${e.alt}`);
    }
  }
  return errs;
}

// ---------- #139 自检 ----------

const cargoLog = (tests, blocks = '') => `     Running tests/direct.rs (target/debug/deps/direct-0123abcd)\n${tests}\n${blocks ? `\nfailures:\n\n${blocks}\nfailures:\n` : ''}`;
const panicBlock = (name, msg, thread = name) => `---- ${name} stdout ----\nthread '${thread}' (7) panicked at crates/airhand-link/tests/direct.rs:1:1:\n${msg}\nnote: run with \`RUST_BACKTRACE=1\`\n\n`;
const rid = (n) => `rust tests/direct.rs::${n}`;

export function selfTest() {
  const cases = [];
  const run = (name, { log, list, stubs = [], ok, mention }) => {
    const { results, fatal } = parseCargo(log);
    const errs = [...fatal, ...evaluate({ results, list }), ...scanStubs(stubs)];
    const pass = ok ? errs.length === 0 : errs.length > 0 && errs.some((e) => e.includes(mention));
    cases.push({ name, pass, errs });
  };
  const red = (n, msg) => ({ t: `test ${n} ... FAILED`, b: panicBlock(n, msg) });
  const mk = (...xs) => cargoLog(xs.map((x) => x.t).join('\n'), xs.map((x) => x.b ?? '').join(''));
  const green = (n) => ({ t: `test ${n} ... ok` });
  run('(1) 失败集合等于名单 → 通过', { log: mk(red('a', 'ASSERT: 没有升级'), green('b')), list: [rid('a')], ok: true });
  run('(2) 多出一条意外失败 → 报错并点名', { log: mk(red('a', 'ASSERT: x'), red('extra', 'ASSERT: y')), list: [rid('a')], ok: false, mention: 'extra' });
  run('(3) 名单中一条已变绿 → 报错并点名', { log: mk(red('a', 'ASSERT: x'), green('done')), list: [rid('a'), rid('done')], ok: false, mention: 'done' });
  run('(3) 正向对照：变绿后从名单删除 → 通过', { log: mk(red('a', 'ASSERT: x'), green('done')), list: [rid('a')], ok: true });
  run('(4) 失败输出含错误原因模式 → 报错', { log: mk(red('a', 'ASSERT: 驱动返回 NotWired')), list: [rid('a')], ok: false, mention: 'NotWired' });
  run('(5) 桩源码含 todo!() → 报错', { log: mk(red('a', 'ASSERT: x')), list: [rid('a')], stubs: [['src/lib.rs', 'fn f() {\n    todo!()\n}']], ok: false, mention: 'todo!' });
  run('(6) unimplemented!() → 报错', { log: mk(red('a', 'not implemented: direct')), list: [rid('a')], ok: false, mention: '::a' });
  run('(6) unwrap() 失败 → 报错', { log: mk(red('a', 'called `Option::unwrap()` on a `None` value')), list: [rid('a')], ok: false, mention: '::a' });
  run('(6) 测试超时 → 报错', { log: cargoLog('test a has been running for over 60 seconds'), list: [rid('a')], ok: false, mention: '超时' });
  run('(7) 名单含已并入、尚未实现的 B 级用例且为断言失败 → 通过', { log: mk(red('a', 'ASSERT: x'), red('b_level', 'assertion `left == right` failed\n  left: 0')), list: [rid('a'), rid('b_level')], ok: true });
  run('(7) B 级并入但没加入名单 → 意外失败', { log: mk(red('a', 'ASSERT: x'), red('b_level', 'ASSERT: y')), list: [rid('a')], ok: false, mention: 'b_level' });
  run('(8) assert!(cond, "ASSERT: …") 自定义消息 → 对的红', { log: mk(red('a', 'ASSERT: 自定义消息')), list: [rid('a')], ok: true });
  run('(8) 反例：自定义消息无标记 → 报错', { log: mk(red('a', '自定义消息')), list: [rid('a')], ok: false, mention: '::a' });
  run('(9) 共享辅助函数 panic!("ASSERT: …") → 对的红', { log: mk(red('a', 'ASSERT: 控制端 0 没有完成直连升级')), list: [rid('a')], ok: true });
  run('(10) tokio::spawn 内断言经 JoinError 再 unwrap（无标记）→ 报错', {
    log: cargoLog('test a ... FAILED', `---- a stdout ----\nthread 'tokio-runtime-worker' (8) panicked at x.rs:1:1:\nASSERT: 任务内\nthread 'a' (7) panicked at x.rs:2:2:\ncalled \`Result::unwrap()\` on an \`Err\` value: JoinError::Panic(Id(3), "ASSERT: 任务内", ..)\n\n`),
    list: [rid('a')], ok: false, mention: 'a',
  });
  run('(10) 任务内共享宏、消息带回重抛 → 对的红', {
    log: cargoLog('test a ... FAILED', `---- a stdout ----\nthread 'tokio-runtime-worker' (8) panicked at x.rs:1:1:\nASSERT: 任务内\nthread 'a' (7) panicked at x.rs:2:2:\nASSERT: 任务内\n\n`),
    list: [rid('a')], ok: true,
  });
  run('(11) 等待无上限、空操作下超时 → 报错', { log: cargoLog('test a has been running for over 60 seconds'), list: [rid('a')], ok: false, mention: '超时' });
  run('(11) 等待设上限、超时后 ASSERT: 失败 → 对的红', { log: mk(red('a', 'ASSERT: 2 s 内没有等到 DirectGateOpen')), list: [rid('a')], ok: true });
  // (12) CARGO_TERM_COLOR=always：`Running …` 与 `test … FAILED` 行带 ANSI 颜色码（模仿 CI 的原始日志）
  const C = (code, t) => `\x1b[${code}m${t}\x1b[0m`;
  const colored = (t) => t.replace('     Running ', `     ${C('1;92', 'Running')} `).replace(/ FAILED$/gm, ` ${C('31', 'FAILED')}`).replace(/ \.\.\. ok$/gm, ` ... ${C('32', 'ok')}`);
  run('(12) 带颜色码的 Running 与 FAILED 行 → 照常判定（失败集合等于名单 → 通过）', { log: colored(mk(red('a', 'ASSERT: 没有升级'), green('b'))), list: [rid('a')], ok: true });
  run('(12) 带颜色码，多出一条意外失败 → 仍然报错并点名', { log: colored(mk(red('a', 'ASSERT: x'), red('extra', 'ASSERT: y'))), list: [rid('a')], ok: false, mention: 'extra' });
  {
    const plain = mk(red('a', 'ASSERT: x'), green('done')); const col = colored(plain);
    const pr = parseCargo(plain).results; const cr = parseCargo(col).results;
    const same = col !== plain && pr.size === 2 && JSON.stringify([...pr]) === JSON.stringify([...cr]);
    cases.push({ name: '(12) 对照：同一份日志有 / 无颜色码，解析结果逐项相同（颜色码确实被去掉）', pass: same, errs: same ? [] : ['有 / 无颜色码的解析结果不同'] });
  }
  {
    // 反例：没有颜色码的日志，结果不变（stripAnsi 不改动普通文本，也不吞掉形如 `[3m` 但前面没有 ESC 的内容）
    const plain = mk(red('a', 'ASSERT: 数组下标 [31m 越界'), green('b'));
    const noChange = stripAnsi(plain) === plain;
    const { results, fatal } = parseCargo(plain); const errs = [...fatal, ...evaluate({ results, list: [rid('a')] })];
    const pass = noChange && errs.length === 0;
    cases.push({ name: '(12) 反例：没有颜色码时文本与结果不变（含不带 ESC 的 `[31m` 字面量）', pass, errs: pass ? [] : ['无颜色码日志被改动或判定改变', ...errs] });
  }
  // Go 侧：Error Trace / ASSERT 放行，panic 与构建失败报错
  const goRun = (name, lines, list, ok, mention) => {
    const { results, fatal } = parseGoJson(lines.map((l) => JSON.stringify(l)).join('\n'), () => 'p2p/gate_test.go');
    const errs = [...fatal, ...evaluate({ results, list })];
    cases.push({ name, pass: ok ? errs.length === 0 : errs.some((e) => e.includes(mention)), errs });
  };
  const ev = (Action, Test, Output) => ({ Action, Package: 'airhand/p2p', Test, Output });
  goRun('Go：assertf 的 ASSERT: → 对的红', [ev('output', 'TestA', '    gate_test.go:1: ASSERT: 闸门没有关闭\n'), ev('fail', 'TestA'), ev('fail')], ['go p2p/gate_test.go::TestA'], true);
  goRun('Go：ErrNotImplemented → 报错', [ev('output', 'TestA', '    gate_test.go:1: NewHostGate: p2p: not implemented\n'), ev('fail', 'TestA'), ev('fail')], ['go p2p/gate_test.go::TestA'], false, 'TestA');
  goRun('Go：Output 里带颜色码（JSON 转义形式）→ 判定不变（对的红）', [ev('output', 'TestA', `    gate_test.go:1: \x1b[31mASSERT: 闸门没有关闭\x1b[0m\n`), ev('fail', 'TestA'), ev('fail')], ['go p2p/gate_test.go::TestA'], true);
  goRun('Go：[build failed] → 报错', [{ Action: 'output', Package: 'airhand/p2p', Output: 'FAIL\tairhand/p2p [build failed]\n' }, ev('fail')], [], false, '用例之外失败');
  return cases;
}

// ---------- 仓库内路径解析 ----------

function walkPackages(ws) {
  const out = []; const wsAbs = join(ROOT, ws);
  const visit = (dir, depth) => {
    const toml = join(dir, 'Cargo.toml');
    if (existsSync(toml)) {
      const src = readFileSync(toml, 'utf8');
      if (dir !== wsAbs && /^\[workspace\]/m.test(src)) return;
      if (/^\[package\]/m.test(src)) {
        const pkg = src.match(/^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms)?.[1];
        const lib = src.match(/^\[lib\][^[]*?^name\s*=\s*"([^"]+)"/ms)?.[1];
        const bins = [...src.matchAll(/^\[\[bin\]\][^[]*?^name\s*=\s*"([^"]+)"/gms)].map((m) => m[1]);
        out.push({ dir, names: [pkg, lib, ...bins].filter(Boolean).map((n) => n.replaceAll('-', '_')) });
      }
    }
    if (depth >= 3) return;
    for (const d of readdirSync(dir)) {
      if (['target', 'node_modules', '.git', 'spike'].includes(d) || d.startsWith('.')) continue;
      const p = join(dir, d); if (statSync(p).isDirectory()) visit(p, depth + 1);
    }
  };
  visit(wsAbs, 0);
  return out;
}

function rustResolver(ws) {
  const pkgs = walkPackages(ws);
  return (rel, bin) => {
    const hits = pkgs.filter((p) => existsSync(join(p.dir, rel)));
    const pick = hits.length === 1 ? hits[0] : hits.find((p) => p.names.includes(bin));
    return pick ? relative(ROOT, join(pick.dir, rel)).split(sep).join('/') : `${ws}/?${bin}/${rel}`;
  };
}

function goResolver(mod) {
  const modPath = readFileSync(join(ROOT, mod, 'go.mod'), 'utf8').match(/^module\s+(\S+)/m)[1];
  const cache = new Map();
  return (pkg, test) => {
    const dir = join(mod, pkg === modPath ? '' : pkg.slice(modPath.length + 1));
    if (!cache.has(dir)) {
      const files = readdirSync(join(ROOT, dir)).filter((f) => f.endsWith('_test.go'));
      cache.set(dir, files.map((f) => [f, readFileSync(join(ROOT, dir, f), 'utf8')]));
    }
    const f = cache.get(dir).find(([, src]) => new RegExp(`\\bfunc ${test}\\s*\\(`).test(src));
    return join(dir, f ? f[0] : '?_test.go').split(sep).join('/');
  };
}

// 条目所属的工作区 / 模块：离它最近的、含 [workspace] 的 Cargo.toml 目录，或含 go.mod 的目录
function homeOf(id) {
  const m = id.match(ID); if (!m) return null;
  const [, lang, path] = m;
  let dir = dirname(path);
  for (;;) {
    if (lang === 'rust' && existsSync(join(ROOT, dir, 'Cargo.toml')) && /^\[workspace\]/m.test(readFileSync(join(ROOT, dir, 'Cargo.toml'), 'utf8'))) return `rust ${dir}`;
    if (lang === 'go' && existsSync(join(ROOT, dir, 'go.mod'))) return `go ${dir}`;
    if (dir === '.' || dir === '') return `${lang} .`;
    dir = dirname(dir);
  }
}

function stubSources() {
  const files = [];
  const add = (p) => {
    const abs = join(ROOT, p);
    if (!existsSync(abs)) return;
    if (statSync(abs).isDirectory()) { for (const f of readdirSync(abs)) add(join(p, f)); return; }
    if (/\.rs$/.test(p) || (/\.go$/.test(p) && !/_test\.go$/.test(p))) files.push([p, readFileSync(abs, 'utf8')]);
  };
  STUB_FILES.forEach(add);
  return files;
}

// ---------- 命令行 ----------

function main(argv) {
  const opt = { rust: [], go: [], list: LIST, registry: REGISTRY, noop: null, write: null, print: false };
  let mode = 'evaluate';
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]; const v = () => { const x = argv[++i]; if (x === undefined) throw new Error(`${a} 缺少参数`); return x; };
    if (a === '--rust') opt.rust.push(v());
    else if (a === '--go') opt.go.push(v());
    else if (a === '--list') opt.list = v();
    else if (a === '--registry') opt.registry = v();
    else if (a === '--noop') { opt.noop = v(); if (!['lenient', 'strict'].includes(opt.noop)) throw new Error('--noop 只能是 lenient 或 strict'); }
    else if (a === '--write-result') opt.write = v();
    else if (a === '--print-failures') opt.print = true;
    else if (a === '--self-test') mode = 'self-test';
    else if (a === '--check-registry') mode = 'check-registry';
    else if (a === '--check-stubs') mode = 'check-stubs';
    else throw new Error(`未知参数 ${a}`);
  }

  const cases = selfTest();
  const failed = cases.filter((c) => !c.pass);
  if (mode === 'self-test') {
    for (const c of cases) console.log(`${c.pass ? 'PASS' : 'FAIL'}  #139 ${c.name}${c.pass ? '' : `\n      ${c.errs.join('\n      ') || '（没有报错）'}`}`);
    console.log(`p2p-expected-red self-test: ${cases.length - failed.length}/${cases.length} passed`);
    return failed.length ? 1 : 0;
  }
  if (failed.length) { console.error(`p2p-expected-red: 门禁自检（#139）失败：${failed.map((c) => c.name).join('；')}`); return 1; }

  const errs = [];
  const read = (p) => readFileSync(join(ROOT, p), 'utf8');
  const { ids: list, errs: le } = parseList(read(opt.list), opt.list);
  const { entries: registry, errs: re } = existsSync(join(ROOT, opt.registry)) ? parseRegistry(read(opt.registry), opt.registry) : { entries: new Map(), errs: [`缺少登记表 ${opt.registry}`] };
  errs.push(...le, ...re, ...checkConsistency(list, registry), ...scanStubs(stubSources()));
  if (mode === 'check-registry' || mode === 'check-stubs') {
    const shown = mode === 'check-stubs' ? scanStubs(stubSources()) : errs;
    shown.forEach((e) => console.error(`p2p-expected-red: ${e}`));
    if (!shown.length) console.log(`p2p-expected-red: ${mode === 'check-stubs' ? '桩扫描通过' : `名单 ${list.length} 条、登记表 ${registry.size} 条，自洽`}`);
    return shown.length ? 1 : 0;
  }
  if (!opt.rust.length && !opt.go.length) throw new Error('至少给一个 --rust 或 --go 输入');

  const results = new Map(); const homes = new Set();
  for (const spec of opt.rust) {
    const [ws, log] = spec.includes('=') ? spec.split('=') : ['.', spec];
    const { results: r, fatal } = parseCargo(readFileSync(resolve(log), 'utf8'), rustResolver(ws));
    if (!r.size && !fatal.length) errs.push(`${log} 里没有任何测试结果`);
    errs.push(...fatal); r.forEach((v, k) => results.set(k, v)); homes.add(`rust ${ws}`);
  }
  for (const spec of opt.go) {
    const [mod, json] = spec.includes('=') ? spec.split('=') : ['p2p', spec];
    const { results: r, fatal } = parseGoJson(readFileSync(resolve(json), 'utf8'), goResolver(mod));
    if (!r.size && !fatal.length) errs.push(`${json} 里没有任何测试结果`);
    errs.push(...fatal); r.forEach((v, k) => results.set(k, v)); homes.add(`go ${mod}`);
  }
  const inScope = (id) => homes.has(homeOf(id));
  errs.push(...evaluate({ results, list, registry, noop: opt.noop, inScope }));

  const fails = [...results].filter(([, r]) => r.status === 'fail').map(([id]) => id).sort();
  if (opt.print) fails.forEach((id) => console.log(id));
  if (opt.write) {
    mkdirSync(dirname(resolve(opt.write)), { recursive: true });
    const out = { polarity: opt.noop ?? 'regular', scope: [...homes].sort(), errors: errs, results: Object.fromEntries([...results].sort().map(([id, r]) => [id, { status: r.status, assert: r.status === 'fail' ? !wrongReason(id.split(' ')[0], r) : null }])) };
    writeFileSync(resolve(opt.write), `${JSON.stringify(out, null, 1)}\n`);
  }
  const passed = [...results.values()].filter((r) => r.status === 'pass').length;
  const ignored = [...results.values()].filter((r) => r.status === 'ignored').length;
  const label = opt.noop ? `空操作 ${opt.noop}` : '常规';
  if (errs.length) { errs.forEach((e) => console.error(`p2p-expected-red: ${e}`)); console.error(`p2p-expected-red（${label}，${[...homes].join('、')}）：${errs.length} 处不符`); return 1; }
  console.log(`p2p-expected-red（${label}，${[...homes].join('、')}）：通过——红 ${fails.length}（全部为断言失败）、绿 ${passed}、忽略 ${ignored}；名单 ${list.length} 条，登记表 ${registry.size} 条；自检 ${cases.length}/${cases.length}`);
  return 0;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try { process.exitCode = main(process.argv.slice(2)); } catch (e) { console.error(`p2p-expected-red: ${e.message}`); process.exitCode = 2; }
}
