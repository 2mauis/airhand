import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { stripTypeScriptTypes } from "node:module";
import { ConnectionAttempt } from "../src/lib/connection-attempt.ts";
import { fitSize, stricter, viewMaxShort } from "../src/lib/fit.ts";

// Load the actual session class with browser/backend dependencies replaced by
// small fakes. This tests delivery ordering without a WebView or network.
function sessionHarness() {
  const source = fs.readFileSync(new URL("../src/lib/session.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?from "[^"]+";\r?\n/gm, "");
  const js = stripTypeScriptTypes(source).replace(/\bexport\s+/g, "") + "\nglobalThis.RemoteSession = RemoteSession;";
  let sink: any;
  let resolve: (value: any) => void = () => {};
  const calls: any[] = [];
  const timers: (() => void)[] = [];
  const sandbox: any = { console, performance, clearTimeout, setTimeout, queueMicrotask, ConnectionAttempt,
    fitSize, stricter, viewMaxShort,
    api: {
      connect: (_c: any, _r: any, _s: any, output: any) => { sink = output; return new Promise(r => { resolve = r; }); },
      disconnect: async (sid: number) => { calls.push(["disconnect", sid]); },
      settings: async (sid: number, s: any) => { calls.push(["settings", sid, s]); },
    },
    // 延时发送用的定时器：由测试手动触发
    window: { setTimeout: (fn: () => void) => { timers.push(fn); return timers.length; } },
  };
  vm.runInNewContext(js, sandbox);
  const session = Object.create(sandbox.RemoteSession.prototype);
  Object.assign(session, {
    attempt: new ConnectionAttempt(), disposed: false, sid: null, state: "connecting", settings: {}, visible: true,
    viewLimit: 0, viewLimitAt: -Infinity, viewport: null, displays: [], currentDisplay: 0,
    video: { reset() {} }, audio: { reset() {} }, sync: { reset() {} }, input: { reset() {} },
    emit() {}, syncAudio() {}, onConnected() { calls.push("connected"); },
  });
  return { session, calls, timers, event: (ev: any) => sink.event(ev), resolve: () => resolve({ sid: 7, resume: [], hostName: "host", hostOs: "Android" }) };
}

test("Closed before connect reply cannot resurrect SID, erase reason or remember a device", async () => {
  const h = sessionHarness();
  const connecting = h.session.start();
  h.event({ type: "closed", reason: { kind: "declined" }, retry: false });
  h.resolve();
  await connecting;
  assert.equal(h.session.state, "closed");
  assert.equal(h.session.sid, null);
  assert.equal(h.session.closedReason.kind, "declined");
  assert.deepEqual(h.calls, [["disconnect", 7]]);
});

test("按显示区域请求分辨率：空闲时马上发，连续变化时停下来再发，原始大小时不限", () => {
  const h = sessionHarness();
  const s = h.session;
  Object.assign(s, {
    sid: 7, scaleMode: "fit", interactive: true,
    settings: { fps: 30, bitrate_kbps: 4000, max_short: 1080, adaptive: true, codec: "H264" },
    displays: [{ index: 0, width: 2880, height: 1800 }], viewport: { w: 1600, h: 1000 },
  });
  const sent = () => h.calls.filter((c) => c[0] === "settings").map((c) => c[2].max_short);

  s.updateViewLimit();
  assert.deepEqual(sent(), [1000], "空闲时马上发");
  s.updateViewLimit();
  assert.deepEqual(sent(), [1000], "没变不重发");

  // 拖动窗口：连续变化只在停下来之后发最后一次
  s.viewport = { w: 1500, h: 900 };
  s.updateViewLimit();
  s.viewport = { w: 1400, h: 850 };
  s.updateViewLimit();
  assert.deepEqual(sent(), [1000]);
  h.timers.at(-1)!();
  assert.deepEqual(sent(), [1000, 850]);

  // 用户选的上限更严时按用户的
  s.settings = { ...s.settings, max_short: 720 };
  s.pushSettings();
  assert.equal(sent().at(-1), 720);

  // 原始大小：不按窗口限制
  s.settings = { ...s.settings, max_short: 0 };
  s.scaleMode = "native";
  s.viewLimitAt = -Infinity;
  s.updateViewLimit();
  assert.equal(sent().at(-1), 0);
});

test("stale completion cannot finish a newer attempt", () => {
  const attempt = new ConnectionAttempt();
  const old = attempt.begin();
  attempt.invalidate();
  const current = attempt.begin();
  attempt.finish(old);
  assert.equal(attempt.accepts(old), false);
  assert.equal(attempt.accepts(current), true);
  assert.equal(attempt.attempting, true);
  attempt.finish(current);
  assert.equal(attempt.attempting, false);
});
