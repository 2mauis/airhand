// 音画同步（纯逻辑，便于单独测试）。
//
// 画面帧和声音包都带有被控端的采集时间，而且来自同一个时钟。对每条流测量
// "本地显示/听到的时刻 − 被控端采集时刻"（偏移），两台电脑之间未知的时钟差对两条流是一样的，
// 相减就抵消了，所以不需要对时：
//
// * 声音偏移 A：正在播放的采样被听到的本地时刻 − 它的采集时刻（抖动缓冲 + 网络 + 声卡输出）
// * 画面偏移 V：画面最早能显示的本地时刻 − 它的采集时刻（网络 + 解码 + 显示）
//
// 通常 A > V（声音要缓冲），于是把每一帧推迟到"采集时刻 + A"再显示；
// 如果画面反而更慢（带宽紧张、大关键帧），就让声音额外缓冲（extraMs）去等画面。
// 所有时间单位：毫秒，本地时间与 performance.now() 同一时基。

/** 画到 Canvas 之后到真正显示出来的平均时间（约半个刷新周期） */
export const DISPLAY_MS = 8;
/** 画面最多推迟这么久（声音缓冲异常大时不跟） */
export const MAX_VIDEO_DELAY_MS = 500;
/** 声音最多为画面额外等这么久 */
export const MAX_EXTRA_MS = 300;
/** 统计画面偏移的时间窗口 */
const WINDOW_MS = 3_000;
/** 两次调整声音额外延迟的最短间隔 */
const UPDATE_MS = 2_000;
/** 画面比声音慢超过这个值才让声音等 */
const RAISE_MS = 15;
/** 声音多等了超过这个值才减少 */
const LOWER_MS = 30;
/** 声音偏移单次测量与当前值相差太大时直接采用（例如重新缓冲、静音后重新开始） */
const SNAP_MS = 40;

export interface AvSyncStats {
  /** 声音比画面晚多少（不做同步时的音画差；正数 = 声音晚）。还没测到时为 null */
  skewMs: number | null;
  /** 当前画面被推迟了多少 */
  videoDelayMs: number;
  /** 当前声音为等画面额外缓冲了多少 */
  extraMs: number;
}

function percentile(values: number[], p: number): number {
  const s = [...values].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(s.length * p))];
}

export class AvSync {
  /** 是否生效（由会话设置：用户打开了开关并且正在播放声音）。不生效时画面立即显示、声音不额外等待 */
  enabled = false;
  /** 声音额外延迟（已下发给播放器的值） */
  extraMs = 0;

  /** 声音偏移 A（平滑后） */
  private audio: number | null = null;
  /** 去掉额外延迟后的声音偏移 */
  private audioBase: number | null = null;
  /** 最近的画面偏移样本 */
  private video: { at: number; off: number }[] = [];
  private lastUpdate = -Infinity;
  private delayAvg = 0;

  /** 声音时钟报告：被控端 `hostUs` 采集的声音在本地 `localMs` 被听到。`appliedMs` = 已生效的额外延迟 */
  onAudioClock(hostUs: number, localMs: number, appliedMs: number) {
    const off = localMs - hostUs / 1000;
    if (this.audio === null || Math.abs(off - this.audio) > SNAP_MS) this.audio = off;
    else this.audio += (off - this.audio) * 0.1;
    this.audioBase = this.audio - appliedMs;
  }

  /**
   * 一帧画面到达。返回应当送去解码的本地时刻（不晚于 `arrivalMs` 表示立即解码）。
   * `decodeMs` = 当前平均解码耗时。
   */
  onVideo(hostUs: number, arrivalMs: number, decodeMs: number): number {
    const host = hostUs / 1000;
    const ready = arrivalMs + decodeMs + DISPLAY_MS;
    this.video.push({ at: arrivalMs, off: ready - host });
    while (this.video.length > 0 && this.video[0].at < arrivalMs - WINDOW_MS) this.video.shift();
    if (this.video.length > 600) this.video.shift();

    let due = arrivalMs;
    if (this.enabled && this.audio !== null) {
      due = host + this.audio - decodeMs - DISPLAY_MS;
      due = Math.min(Math.max(due, arrivalMs), arrivalMs + MAX_VIDEO_DELAY_MS);
    }
    this.delayAvg += (due - arrivalMs - this.delayAvg) * 0.1;
    return due;
  }

  /** 定期调用。声音需要的额外延迟变化时返回新值，否则返回 null。 */
  update(nowMs: number): number | null {
    let next = this.extraMs;
    if (!this.enabled || this.audioBase === null) {
      next = 0;
    } else if (nowMs - this.lastUpdate >= UPDATE_MS && this.video.length >= 10) {
      // 画面偏移取 90 分位：偶尔一帧慢不至于让声音跟着等，持续慢才等
      const v = percentile(
        this.video.map((s) => s.off),
        0.9,
      );
      const want = Math.min(MAX_EXTRA_MS, Math.max(0, v - this.audioBase));
      if (want > this.extraMs + RAISE_MS) next = Math.round(want + 5);
      else if (want < this.extraMs - LOWER_MS) next = Math.round(want > 0 ? want + 5 : 0);
    }
    if (next === this.extraMs) return null;
    this.lastUpdate = nowMs;
    this.extraMs = next;
    return next;
  }

  stats(): AvSyncStats {
    let skew: number | null = null;
    if (this.audioBase !== null && this.video.length > 0) {
      skew = this.audioBase - percentile(
        this.video.map((s) => s.off),
        0.5,
      );
    }
    return { skewMs: skew, videoDelayMs: Math.max(0, this.delayAvg), extraMs: this.extraMs };
  }

  /** 断线重连等：重新测量 */
  reset() {
    this.audio = null;
    this.audioBase = null;
    this.video = [];
    this.delayAvg = 0;
  }
}
