// AudioWorklet：在音频线程里从抖动缓冲取数据输出（由 audio.ts 通过 `?worker&url` 加载）。
import { JitterBuffer, type JitterStats } from "./jitter";

// AudioWorkletGlobalScope 的全局对象（TypeScript 没有对应的 lib）
declare const sampleRate: number;
declare const currentTime: number;
declare function registerProcessor(name: string, ctor: unknown): void;
declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}

export type PlayerMessage =
  | { type: "pcm"; planes: Float32Array[]; start: boolean; hostUs: number }
  | { type: "extra"; ms: number }
  | { type: "reset" }
  | { type: "close" };

/** 音频线程 → 主线程 */
export type WorkletMessage =
  | ({ type: "stats" } & JitterStats)
  /** `ctxTime` 这一刻（AudioContext 时间）开始输出的声音，是被控端 `hostUs` 时采集的 */
  | { type: "clock"; ctxTime: number; hostUs: number; appliedMs: number };

interface Options {
  processorOptions?: { inputRate?: number };
}

/** 时钟报告间隔（秒） */
const CLOCK_INTERVAL = 0.1;

class AirhandAudio extends AudioWorkletProcessor {
  private readonly jb: JitterBuffer;
  private alive = true;
  private sinceStats = 0;
  private sinceClock = Infinity;

  constructor(options: Options) {
    super();
    this.jb = new JitterBuffer(options.processorOptions?.inputRate ?? 48000, sampleRate, 2);
    this.port.onmessage = (e: MessageEvent<PlayerMessage>) => {
      const m = e.data;
      if (m.type === "pcm") this.jb.push(m.planes, m.start, m.hostUs);
      else if (m.type === "extra") this.jb.setExtraMs(m.ms);
      else if (m.type === "reset") this.jb.reset();
      else if (m.type === "close") this.alive = false;
    };
  }

  process(_inputs: Float32Array[][], outputs: Float32Array[][]): boolean {
    const out = outputs[0];
    if (out && out.length > 0) {
      const n = out[0].length;
      // 在 pull 之前取：currentTime 就是这一块第一个采样的输出时间
      const clock = this.jb.clock();
      this.jb.pull(out, n);
      this.sinceClock += n / sampleRate;
      if (clock && this.sinceClock >= CLOCK_INTERVAL) {
        this.sinceClock = 0;
        this.post({ type: "clock", ctxTime: currentTime, hostUs: clock.hostUs, appliedMs: clock.appliedMs });
      }
      this.sinceStats += n;
      if (this.sinceStats >= sampleRate) {
        this.sinceStats = 0;
        this.post({ type: "stats", ...this.jb.stats() });
      }
    }
    return this.alive;
  }

  private post(m: WorkletMessage) {
    this.port.postMessage(m);
  }
}

registerProcessor("airhand-audio", AirhandAudio);
