import assert from "node:assert/strict";
import { test } from "node:test";
import { MIN_VIEW_SHORT, fitSize, stricter, targetSize, viewMaxShort } from "../src/lib/fit.ts";

test("targetSize 与被控端 encoder::target_size 一致", () => {
  // 与 client/src-tauri/src/host/encoder/mod.rs 的 sizes 测试相同
  assert.deepEqual(targetSize(1920, 1080, 0), [1920, 1080]);
  assert.deepEqual(targetSize(1921, 1081, 0), [1920, 1080]);
  assert.deepEqual(targetSize(5120, 2880, 0), [3840, 2160]);
  assert.deepEqual(targetSize(1440, 2560, 0), [1440, 2560]);
  assert.deepEqual(targetSize(3840, 2160, 1080), [1920, 1080]);
  assert.deepEqual(targetSize(2560, 1600, 1080), [1728, 1080]);
  assert.deepEqual(targetSize(1280, 720, 1080), [1280, 720]);
  assert.deepEqual(targetSize(1080, 1920, 720), [720, 1280]);
});

test("按显示区域请求的分辨率：编出来的画面不超出显示区域，并且有一边贴满", () => {
  const screens = [[2880, 1800], [3024, 1964], [3456, 2234], [1920, 1080], [2560, 1440], [3840, 2160], [5120, 2880], [1080, 2400], [1366, 768]];
  for (const [w, h] of screens) {
    for (let vw = 400; vw <= 3000; vw += 37) {
      for (let vh = 300; vh <= 2000; vh += 41) {
        const ms = viewMaxShort(w, h, vw, vh);
        const [fw, fh] = targetSize(w, h, ms);
        if (ms === 0) {
          // 屏幕本来就放得下：不限
          assert.ok(w <= vw && h <= vh, `${w}x${h} in ${vw}x${vh}`);
          continue;
        }
        if (ms === MIN_VIEW_SHORT) continue; // 缩略图：允许比显示区域大，由浏览器缩
        const where = `${w}x${h} in ${vw}x${vh}: max_short ${ms} -> ${fw}x${fh}`;
        assert.ok(fw <= vw && fh <= vh, where);
        // 4K 上限之外，至少有一边贴满：短边向下取整最多少 1 个像素（长边少宽高比那么多），
        // 再加上四舍五入和偶数取整
        const slack = Math.ceil(Math.max(w, h) / Math.min(w, h)) + 2;
        if (fw < 3840 && fh < 3840) assert.ok(vw - fw <= slack || vh - fh <= slack, where);
        assert.equal(fitSize(fw, fh, vw, vh).exact, true, where);
      }
    }
  }
});

test("显示区域比屏幕大时不限；缩略图有下限", () => {
  assert.equal(viewMaxShort(1920, 1080, 1920, 1080), 0);
  assert.equal(viewMaxShort(1920, 1080, 2560, 1400), 0);
  assert.equal(viewMaxShort(1920, 1080, 1919, 1080), 1079);
  assert.equal(viewMaxShort(2880, 1800, 320, 200), MIN_VIEW_SHORT);
  assert.equal(viewMaxShort(0, 0, 100, 100), 0);
  assert.equal(viewMaxShort(1920, 1080, 0, 0), 0);
});

test("短边上限取较严的", () => {
  assert.equal(stricter(0, 0), 0);
  assert.equal(stricter(1080, 0), 1080);
  assert.equal(stricter(0, 1001), 1001);
  assert.equal(stricter(1080, 1001), 1001);
  assert.equal(stricter(720, 1001), 720);
});

test("显示：差不多大时 1:1，否则等比缩放", () => {
  assert.deepEqual(fitSize(1600, 1000, 1600, 1000), { w: 1600, h: 1000, exact: true });
  assert.deepEqual(fitSize(1602, 1000, 1920, 1001), { w: 1602, h: 1000, exact: true });
  // 用户选的分辨率比窗口小很多：放大填满
  assert.deepEqual(fitSize(1280, 720, 1920, 1080), { w: 1920, h: 1080, exact: false });
  // 比窗口大（例如窗口刚缩小，新尺寸的画面还没到）：缩小
  assert.deepEqual(fitSize(1728, 1080, 1600, 1000), { w: 1600, h: 1000, exact: false });
});
