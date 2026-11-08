import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { stripTypeScriptTypes } from "node:module";

test("desktop ACK follows WebView receipt and handles frames before connect reply", async () => {
  const source = fs.readFileSync(new URL("../src/lib/backend/tauri.ts", import.meta.url), "utf8")
    .replace(/^import[\s\S]*?from "[^"]+";\r?\n/gm, "");
  const js = stripTypeScriptTypes(source).replace(/\bexport\s+/g, "") + "\nglobalThis.backend = tauriBackend();";
  const channels: any[] = [];
  const calls: any[] = [];
  let resolve: (v: any) => void = () => {};
  const sandbox: any = {
    Channel: class { constructor() { channels.push(this); } },
    getCurrentWindow: () => ({}),
    invoke: (command: string, args: any) => {
      calls.push([command, args]);
      return command === "ctrl_connect" ? new Promise(r => { resolve = r; }) : Promise.resolve();
    },
  };
  vm.runInNewContext(js, sandbox);
  let received = 0;
  const connected = sandbox.backend.connect("123456", null, {}, { packet() { received++; }, event() {} }, true);
  const pkt = new ArrayBuffer(24);
  const dv = new DataView(pkt); dv.setUint8(0, 1); dv.setUint32(4, 9, true);
  assert.equal(calls.length, 1);
  channels[0].onmessage(pkt);
  assert.equal(received, 1);
  assert.equal(calls.length, 1); // SID not known yet
  resolve({ sid: 5 });
  await connected;
  assert.equal(calls[1][0], "ctrl_ack");
  assert.equal(calls[1][1].sid, 5);
  assert.equal(calls[1][1].seq, 9);
  channels[0].onmessage(pkt);
  assert.equal(calls.length, 3);
});
