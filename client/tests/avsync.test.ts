// 音画同步控制逻辑的仿真测试：`npm test`
import assert from "node:assert/strict";
import { test } from "node:test";
import { AvSync, DISPLAY_MS, MAX_VIDEO_DELAY_MS } from "../src/lib/avsync.ts";

/**
 * 仿真：被控端时钟 host（ms），本地时钟 = host + CLOCK（两台电脑的时钟差，任意值）。
 * 声音：采集后 audioMs 被听到（再加上同步要求的额外延迟）；
 * 画面：采集后 videoMs 到达，解码 DECODE_MS。
 */
const CLOCK = -123_456.789;
const DECODE_MS = 5;

interface Scenario {
  seconds: number;
  audioMs: number;
  /** 画面到达延迟（可随时间变化） */
  videoMs: (t: number) => number;
  enabled: boolean;
  /** 仿真开始的时刻（多段仿真接着上一段的时间继续） */
  startMs?: number;
}

function run(sc: Scenario, sync = new AvSync()) {
  sync.enabled = sc.enabled;
  /** 已生效的声音额外延迟（播放器以一定速度追上目标） */
  let applied = sync.extraMs;
  const residuals: { t: number; ms: number }[] = [];
  const delays: number[] = [];
  const start = sc.startMs ?? 0;
  for (let t = start; t < start + sc.seconds * 1000; t += 1) {
    const local = t + CLOCK;
    // 声音时钟：每 100 ms 报告一次
    if (t % 100 === 0) {
      applied += Math.max(-20, Math.min(20, sync.extraMs - applied));
      const hostHeard = t - sc.audioMs - applied;
      sync.onAudioClock(hostHeard * 1000, local, applied);
      sync.update(local);
    }
    // 画面：60 fps
    if (t % 16 === 0) {
      const host = t - sc.videoMs(t);
      const due = sync.onVideo(host * 1000, local, DECODE_MS);
      assert.ok(due >= local - 1e-6, "不能早于到达时刻");
      const shown = due + DECODE_MS + DISPLAY_MS;
      // 同一时刻采集的声音被听到的本地时间
      const heard = host + CLOCK + sc.audioMs + applied;
      residuals.push({ t, ms: shown - heard });
      delays.push(due - local);
    }
  }
  return { sync, residuals, delays, applied };
}

const tail = (r: { t: number; ms: number }[], fromMs: number) => r.filter((x) => x.t >= fromMs).map((x) => x.ms);
const maxAbs = (v: number[]) => Math.max(...v.map(Math.abs));

test("关闭同步：画面立即显示，统计出声音比画面晚多少", () => {
  const { residuals, delays, sync } = run({ seconds: 5, audioMs: 95, videoMs: () => 20, enabled: false });
  assert.ok(delays.every((d) => d === 0));
  // 画面 20 + 5 + 8 = 33 ms 显示，声音 95 ms 才听到
  const r = tail(residuals, 1000);
  assert.ok(r.every((x) => Math.abs(x - (33 - 95)) < 1e-6));
  assert.ok(Math.abs(sync.stats().skewMs! - 62) < 1, `skew ${sync.stats().skewMs}`);
});

test("打开同步：画面推迟到与声音对齐，与两台电脑的时钟差无关", () => {
  const { residuals, sync } = run({ seconds: 5, audioMs: 95, videoMs: () => 20, enabled: true });
  const r = tail(residuals, 500);
  assert.ok(maxAbs(r) < 1, `残差 ${maxAbs(r)} ms`);
  assert.equal(sync.extraMs, 0, "画面更快时声音不需要等");
  assert.ok(Math.abs(sync.stats().videoDelayMs - 62) < 2);
});

test("网络抖动：画面到达时间忽快忽慢，同步后残差仍很小", () => {
  let seed = 3;
  const rand = () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31);
  const { residuals, sync } = run({ seconds: 10, audioMs: 120, videoMs: () => 15 + rand() * 40, enabled: true });
  const r = tail(residuals, 1000);
  assert.ok(maxAbs(r) < 1, `残差 ${maxAbs(r)} ms`);
  assert.equal(sync.extraMs, 0);
});

test("画面比声音还慢：声音额外等画面，而且不来回抖动", () => {
  const { residuals, sync } = run({ seconds: 20, audioMs: 80, videoMs: () => 200, enabled: true });
  // 画面 200 + 5 + 8 = 213 ms，声音需要多等约 133 ms
  assert.ok(sync.extraMs >= 133 && sync.extraMs <= 150, `extra ${sync.extraMs}`);
  const r = tail(residuals, 10_000);
  assert.ok(r.every((x) => x <= 1 && x > -25), `残差范围 ${Math.min(...r)} ~ ${Math.max(...r)}`);
});

test("画面变慢后又恢复：声音的额外延迟跟着撤掉", () => {
  const sync = new AvSync();
  const slow = run({ seconds: 15, audioMs: 80, videoMs: () => 200, enabled: true }, sync);
  assert.ok(slow.sync.extraMs > 100);
  const fast = run({ seconds: 15, audioMs: 80, videoMs: () => 20, enabled: true, startMs: 15_000 }, sync);
  assert.equal(fast.sync.extraMs, 0);
  assert.ok(maxAbs(tail(fast.residuals, 23_000)) < 1);
});

test("偶尔一帧特别慢（例如大关键帧）不会让声音跟着等", () => {
  const { sync } = run({ seconds: 20, audioMs: 90, videoMs: (t) => (t % 2000 < 16 ? 300 : 20), enabled: true });
  assert.equal(sync.extraMs, 0);
});

test("声音缓冲异常大时，画面最多推迟 MAX_VIDEO_DELAY_MS", () => {
  const { delays } = run({ seconds: 3, audioMs: 2000, videoMs: () => 20, enabled: true });
  assert.ok(Math.max(...delays) <= MAX_VIDEO_DELAY_MS + 1e-6);
});

test("关闭同步：声音的额外延迟立即归零", () => {
  const sync = new AvSync();
  run({ seconds: 10, audioMs: 80, videoMs: () => 200, enabled: true }, sync);
  assert.ok(sync.extraMs > 0);
  sync.enabled = false;
  assert.equal(sync.update(0), 0);
  assert.equal(sync.extraMs, 0);
});

test("还没有声音时不推迟画面", () => {
  const sync = new AvSync();
  sync.enabled = true;
  assert.equal(sync.onVideo(1_000_000, 5000, 5), 5000);
  assert.equal(sync.stats().skewMs, null);
});
