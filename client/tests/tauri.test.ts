import assert from "node:assert/strict";
import { test } from "node:test";
import fs from "node:fs";
import vm from "node:vm";
import { stripTypeScriptTypes } from "node:module";
import { videoSeq } from "../src/lib/media.ts";

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
    videoSeq,
    invoke: (command: string, args: any) => {
      calls.push([command, args]);
      return command === "ctrl_connect" ? new Promise(r => { resolve = r; }) : Promise.resolve();
    },
  };
  vm.runInNewContext(js, sandbox);
  // 沙箱里建的对象原型不同，按 JSON 比较
  const call = (i: number) => JSON.parse(JSON.stringify(calls[i]));
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
  assert.deepEqual(call(1), ["ctrl", { command: { cmd: "ack", sid: 5, seq: 9 } }]);
  channels[0].onmessage(pkt);
  assert.equal(calls.length, 3);
  // 光标包、声音包不确认
  const cursor = new ArrayBuffer(24);
  new DataView(cursor).setUint8(0, 2);
  channels[0].onmessage(cursor);
  assert.equal(calls.length, 3);
  assert.equal(received, 3);
  // 对会话的操作都走同一个 ctrl 命令
  await sandbox.backend.command({ cmd: "keyframe", sid: 5 });
  assert.deepEqual(call(3), ["ctrl", { command: { cmd: "keyframe", sid: 5 } }]);
});
