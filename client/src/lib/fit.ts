// 画面只缩放一次：按画面在窗口里实际显示的大小向被控端请求分辨率，控制端再按 1:1 显示。
//
// 被控端把屏幕缩到"短边上限"再编码，控制端如果再把解出来的画面缩放到窗口里，就缩放了两次。
// 第二次是浏览器的 GPU 双线性采样，比例接近 1 时（例如 1728×1080 显示成 1600×1000）每个像素都和邻居混在一起，
// 文字明显发虚。所以显示区域比用户选的分辨率上限还小时，请求的短边上限就按显示区域算（被控端一次缩到位，
// 用的是 Catmull-Rom），收到的画面和显示区域差不多大时按 1:1 显示，并对齐到设备像素。
// 顺带也省带宽：不再发比窗口能显示的更多的像素。

/**
 * 请求的短边不小于这个值：多会话的缩略图没必要那么清楚，太小的尺寸硬件编码器也不一定支持，让浏览器缩
 * （与 Android 控制端的 VideoFit.MIN_SHORT 相同）
 */
export const MIN_VIEW_SHORT = 360;
/** 画面比显示区域小、但放大比例不到这个值时按 1:1 显示（留一点黑边），不做接近 1 的放大 */
export const SNAP = 1.04;

// 与被控端 encoder::target_size 相同：硬件编码器普遍支持的最大分辨率
const MAX_LONG = 3840;
const MAX_SHORT = 2160;

/** 被控端（桌面）按短边上限 `maxShort`（0 = 不限）编出来的尺寸，与 encoder::target_size 一致 */
export function targetSize(w: number, h: number, maxShort: number): [number, number] {
  const short = Math.min(w, h);
  if (maxShort > 0 && short > maxShort) {
    const f = maxShort / short;
    w *= f;
    h *= f;
  }
  const [mw, mh] = w >= h ? [MAX_LONG, MAX_SHORT] : [MAX_SHORT, MAX_LONG];
  const f = Math.min(mw / w, mh / h, 1);
  const even = (v: number) => Math.max(64, Math.round(v * f) & ~1);
  return [even(w), even(h)];
}

/**
 * 对方的屏幕 `w`×`h`（物理像素）完整显示在 `vw`×`vh`（设备像素）的区域里，需要请求的短边上限。
 * 不用缩小时返回 0（不限）。被控端按它算出的宽高四舍五入之后也不会超出显示区域。
 */
export function viewMaxShort(w: number, h: number, vw: number, vh: number): number {
  if (!(w > 0 && h > 0 && vw > 0 && vh > 0)) return 0;
  const s = Math.min(vw / w, vh / h);
  if (s >= 1) return 0;
  return Math.max(MIN_VIEW_SHORT, Math.floor(Math.min(w, h) * s + 1e-6));
}

/** 两个短边上限取较严的（0 = 不限） */
export function stricter(a: number, b: number): number {
  if (!a) return b;
  if (!b) return a;
  return Math.min(a, b);
}

/**
 * 画面 `fw`×`fh` 在 `vw`×`vh` 的区域里显示多大（都是设备像素）：
 * 和区域差不多大（放大不到 SNAP）时 1:1（`exact`），否则等比缩放到正好放下。
 */
export function fitSize(fw: number, fh: number, vw: number, vh: number): { w: number; h: number; exact: boolean } {
  const s = Math.min(vw / fw, vh / fh);
  if (s >= 1 && s < SNAP) return { w: fw, h: fh, exact: true };
  return { w: Math.max(1, Math.floor(fw * s)), h: Math.max(1, Math.floor(fh * s)), exact: false };
}
