// 远程声音：WebCodecs 解码 Opus → AudioWorklet 抖动缓冲 → 扬声器。
//
// * 所有会话共用一个 AudioContext，每个正在播放的会话一个 AudioWorkletNode；
// * 解码后的 PCM 以 Transferable 方式交给音频线程，主线程不做任何混音；
// * 音频线程定期报告"正在播放的是被控端哪一刻的声音"，换算成本地时间后交给音画同步。
import type { PlayerMessage, WorkletMessage } from "./audio-worklet";
import workletUrl from "./audio-worklet?worker&url";
import type { JitterStats } from "./jitter";

/** 与 airhand_proto::session::AUDIO_* 一致 */
const SAMPLE_RATE = 48000;
const CHANNELS = 2;
const FRAME_SAMPLES = 960;
const CONFIG: AudioDecoderConfig = { codec: "opus", sampleRate: SAMPLE_RATE, numberOfChannels: CHANNELS };

let shared: { ctx: AudioContext; ready: Promise<void> } | null = null;

function audioContext() {
  if (shared) return shared;
  let ctx: AudioContext;
  try {
    ctx = new AudioContext({ latencyHint: "interactive", sampleRate: SAMPLE_RATE });
  } catch {
    ctx = new AudioContext({ latencyHint: "interactive" });
  }
  const ready = ctx.audioWorklet.addModule(workletUrl);
  // 自动播放策略：没有用户操作时 AudioContext 可能处于 suspended，在第一次点击/按键时恢复
  const resume = () => {
    if (ctx.state === "suspended") void ctx.resume().catch(() => {});
  };
  window.addEventListener("pointerdown", resume, { capture: true });
  window.addEventListener("keydown", resume, { capture: true });
  resume();
  shared = { ctx, ready };
  return shared;
}

let support: Promise<boolean> | null = null;

/** 本机能否解码 Opus（WebView2 自带软件解码器，正常都支持） */
export function opusSupported(): Promise<boolean> {
  support ??= (async () => {
    if (typeof AudioDecoder === "undefined") return false;
    try {
      return (await AudioDecoder.isConfigSupported(CONFIG)).supported === true;
    } catch {
      return false;
    }
  })();
  return support;
}

export class AudioPlayer {
  /** 最近一次统计（约每秒更新） */
  stats: JitterStats | null = null;
  onUnsupported: (() => void) | null = null;
  /** 被控端 `hostUs`（微秒）采集的声音在本地 `localMs`（performance.now() 时基）被听到 */
  onClock: ((hostUs: number, localMs: number, appliedMs: number) => void) | null = null;

  private enabled = false;
  private gen = 0;
  private node: AudioWorkletNode | null = null;
  private ctx: AudioContext | null = null;
  private decoder: AudioDecoder | null = null;
  private extraMs = 0;
  /**
   * 已送去解码、还没出结果的包（按顺序一一对应）。不用解码输出的时间戳：
   * 浏览器可能按采样数自己推算输出时间戳，跳过静音段之后就对不上了。
   */
  private inflight: { hostUs: number; start: boolean }[] = [];

  enable() {
    if (this.enabled) return;
    this.enabled = true;
    const gen = ++this.gen;
    void (async () => {
      if (!(await opusSupported())) {
        if (gen === this.gen) this.onUnsupported?.();
        return;
      }
      const { ctx, ready } = audioContext();
      try {
        await ready;
      } catch (e) {
        console.warn("can't load the audio worklet", e);
        return;
      }
      if (gen !== this.gen) return;
      const node = new AudioWorkletNode(ctx, "airhand-audio", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [CHANNELS],
        processorOptions: { inputRate: SAMPLE_RATE },
      });
      node.port.onmessage = (e: MessageEvent<WorkletMessage>) => this.onWorklet(e.data);
      node.connect(ctx.destination);
      this.node = node;
      this.ctx = ctx;
      if (this.extraMs > 0) this.post({ type: "extra", ms: this.extraMs });
    })();
  }

  disable() {
    if (!this.enabled) return;
    this.enabled = false;
    this.gen++;
    this.closeDecoder();
    if (this.node) {
      this.post({ type: "close" });
      this.node.disconnect();
      this.node = null;
    }
    this.stats = null;
  }

  /** 新的会话流：解码器与缓冲都从头开始 */
  reset() {
    this.closeDecoder();
    this.post({ type: "reset" });
  }

  /** 音画同步：让声音额外缓冲（画面比声音慢时等画面） */
  setExtraDelay(ms: number) {
    this.extraMs = ms;
    this.post({ type: "extra", ms });
  }

  push(start: boolean, hostUs: number, data: Uint8Array) {
    if (!this.node) return;
    if (!this.decoder || this.decoder.state === "closed") this.openDecoder();
    const dec = this.decoder!;
    if (dec.decodeQueueSize > 25) {
      // 解码跟不上（几乎不会发生）：丢掉积压，重新开始
      this.closeDecoder();
      return;
    }
    this.inflight.push({ hostUs, start });
    try {
      dec.decode(new EncodedAudioChunk({ type: "key", timestamp: hostUs, data }));
    } catch {
      this.closeDecoder();
    }
  }

  private onWorklet(m: WorkletMessage) {
    if (m.type === "stats") {
      this.stats = m;
    } else if (m.type === "clock" && this.ctx && this.onClock) {
      // 音频上下文时间 → 本地时间：getOutputTimestamp 给出"此刻扬声器正在播放的位置"，已含输出延迟
      const ts = this.ctx.getOutputTimestamp();
      if (!ts.performanceTime || ts.contextTime === undefined) return;
      this.onClock(m.hostUs, ts.performanceTime + (m.ctxTime - ts.contextTime) * 1000, m.appliedMs);
    }
  }

  private post(m: PlayerMessage, transfer: Transferable[] = []) {
    this.node?.port.postMessage(m, transfer);
  }

  private openDecoder() {
    const decoder = new AudioDecoder({
      output: (d) => this.output(d),
      error: (e) => {
        console.warn("audio decoder error", e);
        if (this.decoder === decoder) {
          this.decoder = null;
          this.inflight = [];
        }
      },
    });
    decoder.configure(CONFIG);
    this.decoder = decoder;
  }

  private closeDecoder() {
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
    this.inflight = [];
  }

  private output(d: AudioData) {
    const info = this.inflight.shift();
    if (!this.node || !info) {
      d.close();
      return;
    }
    const n = d.numberOfFrames;
    const planes: Float32Array[] = [];
    for (let c = 0; c < Math.min(d.numberOfChannels, CHANNELS); c++) {
      const p = new Float32Array(n);
      d.copyTo(p, { planeIndex: c, format: "f32-planar" });
      planes.push(p);
    }
    d.close();
    // 解码器刚创建时会丢掉开头的编码器前瞻（输出比一包短）：丢的是包的前面部分
    const hostUs = info.hostUs + (Math.max(0, FRAME_SAMPLES - n) / SAMPLE_RATE) * 1e6;
    this.post({ type: "pcm", planes, start: info.start, hostUs }, planes.map((p) => p.buffer));
  }
}
