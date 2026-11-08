import assert from "node:assert/strict";
import { test } from "node:test";
import { RemoteInput } from "../src/lib/input.ts";

test("pending input cannot leak into a replacement session", async () => {
  const batches: unknown[] = [];
  const input = new RemoteInput({ addEventListener() {} } as any, () => true, events => batches.push(events));
  input.enqueue([{ Key: { code: "KeyA", down: true } }], true);
  input.reset();
  input.enqueue([{ Key: { code: "KeyB", down: false } }], true);
  await Promise.resolve();
  assert.deepEqual(batches, [[{ Key: { code: "KeyB", down: false } }]]);
  input.dispose();
});

test("mouse moves coalesce, ordered key edges stay in bounded batches", async () => {
  const batches: any[][] = [];
  const input = new RemoteInput({ addEventListener() {} } as any, () => true, events => batches.push(events));
  for (let i = 0; i < 1000; i++) input.enqueue([{ MouseMove: { x: i / 1000, y: 0 } }], false);
  input.enqueue(["ReleaseAll"], true);
  await Promise.resolve();
  assert.equal(batches[0].length, 2);
  for (let i = 0; i < 200; i++) input.enqueue([{ Key: { code: "KeyA", down: i % 2 === 0 } }], true);
  await Promise.resolve();
  assert.ok(batches.every(b => b.length <= 64));
  assert.equal(batches.slice(1).flat().length, 200);
  input.dispose();
});
