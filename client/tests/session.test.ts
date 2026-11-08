import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { stripTypeScriptTypes } from "node:module";
import { ConnectionAttempt } from "../src/lib/connection-attempt.ts";

// Load the actual session class with browser/backend dependencies replaced by
// small fakes. This tests delivery ordering without a WebView or network.
function sessionHarness() {
  const source = fs.readFileSync(new URL("../src/lib/session.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?from "[^"]+";\r?\n/gm, "");
  const js = stripTypeScriptTypes(source).replace(/\bexport\s+/g, "") + "\nglobalThis.RemoteSession = RemoteSession;";
  let sink: any;
  let resolve: (value: any) => void = () => {};
  const calls: any[] = [];
  const sandbox: any = { console, performance, clearTimeout, setTimeout, queueMicrotask, ConnectionAttempt,
    api: {
      connect: (_c: any, _r: any, _s: any, output: any) => { sink = output; return new Promise(r => { resolve = r; }); },
      disconnect: async (sid: number) => { calls.push(["disconnect", sid]); },
    },
  };
  vm.runInNewContext(js, sandbox);
  const session = Object.create(sandbox.RemoteSession.prototype);
  Object.assign(session, {
    attempt: new ConnectionAttempt(), disposed: false, sid: null, state: "connecting", settings: {}, visible: true,
    video: { reset() {} }, audio: { reset() {} }, sync: { reset() {} }, input: { reset() {} },
    emit() {}, syncAudio() {}, onConnected() { calls.push("connected"); },
  });
  return { session, calls, event: (ev: any) => sink.event(ev), resolve: () => resolve({ sid: 7, resume: [], hostName: "host", hostOs: "Android" }) };
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
