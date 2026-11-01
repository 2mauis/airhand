// 声音抖动缓冲的仿真测试：`npm test`（Node 自带测试运行器，直接运行 TypeScript）
import assert from "node:assert/strict";
import { test } from "node:test";
import { JitterBuffer } from "../src/lib/jitter.ts";

const RATE = 48000;
const PACKET = 960; // 20 ms
const QUANTUM = 128; // AudioWorklet 每次 process 的帧数

interface Arrival {
  /** 到达时间（输出帧） */
  at: number;
  start: boolean;
  planes: Float32Array[];
}

/** 按发送时刻生成包：每包是连续的正弦波片段，`gaps` 内不发包（模拟被控端静音） */
function packets(seconds: number, opts: { drift?: number; gaps?: [number, number][] } = {}) {
  const out: { sentAt: number; start: boolean; planes: Float32Array[] }[] = [];
  const period = (PACKET / (1 + (opts.drift ?? 0)));
  let phase = 0;
  let silent = true;
  for (let i = 0; i * period < seconds * RATE; i++) {
    const t = i * period;
    const inGap = (opts.gaps ?? []).some(([a, b]) => t >= a * RATE && t < b * RATE);
    if (inGap) {
      silent = true;
      continue;
    }
    const l = new Float32Array(PACKET);
    for (let k = 0; k < PACKET; k++) l[k] = Math.sin((phase += (2 * Math.PI * 440) / RATE)) * 0.5;
    out.push({ sentAt: t, start: silent, planes: [l, l.slice()] });
    silent = false;
  }
  return out;
}

function simulate(arrivals: Arrival[], seconds: number) {
  const jb = new JitterBuffer(RATE, RATE, 2);
  arrivals.sort((a, b) => a.at - b.at);
  const out = [new Float32Array(QUANTUM), new Float32Array(QUANTUM)];
  const levels: number[] = [];
  let next = 0;
  let maxJump = 0;
  let prev = 0;
  let audible = 0;
  for (let t = 0; t < seconds * RATE; t += QUANTUM) {
    while (next < arrivals.length && arrivals[next].at <= t) {
      jb.push(arrivals[next].planes, arrivals[next].start);
      next++;
    }
    jb.pull(out, QUANTUM);
    for (const v of out[0]) {
      if (v !== 0) audible++;
      maxJump = Math.max(maxJump, Math.abs(v - prev));
      prev = v;
    }
    levels.push(jb.stats().bufferedMs);
  }
  return { jb, levels, maxJump, audible };
}

/** 简单的确定性随机数 */
function rng(seed: number) {
  return () => ((seed = (seed * 1103515245 + 12345) % 2 ** 31) / 2 ** 31);
}

test("稳定网络：不断粮、不跳跃、波形连续，延迟回落到下限", () => {
  const arr = packets(60).map((p) => ({ at: p.sentAt + 5 * 48, start: p.start, planes: p.planes }));
  const { jb, maxJump } = simulate(arr, 60);
  const s = jb.stats();
  assert.equal(s.underruns, 0);
  assert.equal(s.skips, 0);
  // 440 Hz、幅度 0.5 的正弦波相邻采样最大差约 0.029；渐入不会更大
  assert.ok(maxJump < 0.035, `波形不连续: ${maxJump}`);
  assert.equal(Math.round(s.targetMs), 40, `目标延迟应回落到 40ms，实际 ${s.targetMs}`);
});

test("随机抖动：初始缓冲足以吸收 0~40 ms 的抖动", () => {
  const rand = rng(7);
  let last = 0;
  const arr = packets(20).map((p) => {
    const at = Math.max(last, p.sentAt + rand() * 40 * 48);
    last = at;
    return { at, start: p.start, planes: p.planes };
  });
  assert.equal(simulate(arr, 20).jb.stats().underruns, 0);
});

test("延迟尖峰（例如大关键帧堵住流）：目标延迟自动加大，之后不再断粮", () => {
  // 基础延迟 5 ms，每 2 秒有一包晚到 120 ms，后面的包排在它后面（QUIC 流是有序的）
  let last = 0;
  const arr = packets(40).map((p, i) => {
    const at = Math.max(last, p.sentAt + (i % 100 === 50 ? 120 : 5) * 48);
    last = at;
    return { at, start: p.start, planes: p.planes };
  });
  const { jb } = simulate(arr, 40);
  const s = jb.stats();
  assert.ok(s.underruns >= 1 && s.underruns <= 6, `断粮次数 ${s.underruns}`);
  assert.ok(s.targetMs >= 100, `目标延迟应加大，实际 ${s.targetMs}`);

  // 后半段不应再断粮
  const half = simulate(
    arr.filter((a) => a.at < 20 * RATE),
    20,
  ).jb.stats().underruns;
  assert.equal(s.underruns, half, "稳定后仍在断粮");
});

test("对方静音：不算网络抖动，目标延迟不变", () => {
  const arr = packets(10, { gaps: [[3, 4], [6, 6.5]] }).map((p) => ({ at: p.sentAt + 2 * 48, start: p.start, planes: p.planes }));
  const { jb } = simulate(arr, 10);
  const s = jb.stats();
  assert.equal(s.underruns, 0);
  assert.equal(Math.round(s.targetMs), 60);
});

test("时钟偏差：两端声卡快慢不同时缓冲保持稳定", () => {
  for (const drift of [0.005, -0.005]) {
    const arr = packets(60, { drift }).map((p) => ({ at: p.sentAt + 5 * 48, start: p.start, planes: p.planes }));
    const { jb, levels } = simulate(arr, 60);
    const s = jb.stats();
    const tail = levels.slice(-levels.length / 4);
    const max = Math.max(...tail);
    assert.equal(s.skips, 0, `drift=${drift} 不应跳跃`);
    assert.ok(s.underruns <= 1, `drift=${drift} 断粮 ${s.underruns} 次`);
    assert.ok(max < s.targetMs + 40, `drift=${drift} 缓冲失控: ${max}ms`);
  }
});

test("卡顿后数据涌来：跳过积压，不让声音越来越滞后", () => {
  // 第 5 秒起网络卡住 500 ms，之后积压的包一次性到达
  const arr = packets(15).map((p) => {
    let at = p.sentAt + 5 * 48;
    if (at >= 5 * RATE && at < 5.5 * RATE) at = 5.5 * RATE;
    return { at, start: p.start, planes: p.planes };
  });
  const { jb, levels } = simulate(arr, 15);
  const s = jb.stats();
  assert.ok(s.skips >= 1, "应当跳过积压");
  const end = levels.slice(-100);
  assert.ok(Math.max(...end) < s.targetMs + 30, `结尾缓冲 ${Math.max(...end)}ms 仍然过多`);
});

test("输出采样率不同（44.1 kHz）时按比例重采样", () => {
  const jb = new JitterBuffer(RATE, 44100, 2);
  const arr = packets(5);
  const out = [new Float32Array(QUANTUM), new Float32Array(QUANTUM)];
  let next = 0;
  let produced = 0;
  for (let t = 0; t < 5 * 44100; t += QUANTUM) {
    // 输出时间 t（44.1k 帧）对应的输入时间
    while (next < arr.length && arr[next].sentAt + 5 * 48 <= (t * RATE) / 44100) {
      jb.push(arr[next].planes, arr[next].start);
      next++;
    }
    jb.pull(out, QUANTUM);
    produced += QUANTUM;
  }
  const s = jb.stats();
  assert.equal(s.underruns, 0);
  assert.equal(s.skips, 0);
  assert.ok(produced > 0 && s.bufferedMs < 100);
});

/** 带采集时间戳的包：被控端时钟从 `hostStart` 微秒开始 */
function stamped(seconds: number, hostStart: number, delayMs: number) {
  return packets(seconds).map((p) => ({
    at: p.sentAt + delayMs * 48,
    start: p.start,
    planes: p.planes,
    hostUs: hostStart + (p.sentAt / RATE) * 1e6,
  }));
}

test("播放时钟：报告的采集时间 = 输出时刻 − 端到端延迟", () => {
  const jb = new JitterBuffer(RATE, RATE, 2);
  const arr = stamped(10, 5e9, 5);
  const out = [new Float32Array(QUANTUM), new Float32Array(QUANTUM)];
  let next = 0;
  const lags: number[] = [];
  for (let t = 0; t < 10 * RATE; t += QUANTUM) {
    while (next < arr.length && arr[next].at <= t) {
      jb.push(arr[next].planes, arr[next].start, arr[next].hostUs);
      next++;
    }
    const c = jb.clock();
    if (c && t > 2 * RATE) lags.push((t / RATE) * 1000 - (c.hostUs - 5e9) / 1000);
    jb.pull(out, QUANTUM);
  }
  // 播放的声音比采集晚 = 网络 5 ms + 缓冲；缓冲稳定在目标附近
  const s = jb.stats();
  const avg = lags.reduce((a, b) => a + b, 0) / lags.length;
  assert.ok(lags.length > 1000);
  assert.ok(Math.abs(avg - (5 + s.bufferedMs)) < 8, `平均延迟 ${avg} ms，缓冲 ${s.bufferedMs} ms`);
  assert.ok(Math.max(...lags) - Math.min(...lags) < 25, "播放时钟抖动过大");
});

test("额外延迟：小幅增加靠减速追上，大幅增加停顿一次补齐，减少时加速/跳过", () => {
  const jb = new JitterBuffer(RATE, RATE, 2);
  const arr = stamped(60, 0, 5);
  const out = [new Float32Array(QUANTUM), new Float32Array(QUANTUM)];
  let next = 0;
  let t = 0;
  /** 播放到第 sec 秒，返回此时"输出时刻 − 采集时刻"（ms） */
  const lagAt = (sec: number) => {
    let lag = NaN;
    for (; t < sec * RATE; t += QUANTUM) {
      while (next < arr.length && arr[next].at <= t) {
        jb.push(arr[next].planes, arr[next].start, arr[next].hostUs);
        next++;
      }
      const c = jb.clock();
      if (c) lag = (t / RATE) * 1000 - c.hostUs / 1000;
      jb.pull(out, QUANTUM);
    }
    return lag;
  };
  const base = lagAt(5);
  jb.setExtraMs(40);
  const small = lagAt(15);
  assert.ok(Math.abs(small - base - 40) < 8, `+40ms 后延迟 ${small - base}`);
  assert.equal(jb.underruns, 0);

  const skipsBefore = jb.skips;
  jb.setExtraMs(240);
  const big = lagAt(17);
  assert.ok(Math.abs(big - base - 240) < 10, `+240ms 应很快到位，实际 ${big - base}`);
  assert.equal(jb.underruns, 0, "停顿补齐不应计为断粮");

  jb.setExtraMs(0);
  const back = lagAt(30);
  assert.ok(Math.abs(back - base) < 10, `恢复后延迟 ${back - base}`);
  assert.ok(jb.skips > skipsBefore, "大幅减少时应直接跳过");
});
