// 远程会话：WebCodecs 硬件解码 + Canvas 渲染 + 输入采集 + 光标同步 + 自动重连。
//
// 性能要点：
// * 视频帧以二进制传入（桌面版是 Tauri Channel，网页版是 WebSocket，零 JSON），直接切片交给 VideoDecoder；
// * 解码器 optimizeForLatency，帧解码后立即绘制到 desynchronized Canvas，不排队等 vsync；
// * 鼠标移动按 8ms 合并，按键/点击立即发送；
// * 不可见的会话自动降到 5fps，节省带宽与对端 CPU；
// * 只有当前正在查看的会话播放声音，其余会话通知被控端停止采集声音；
// * 音画同步（可选）：按采集时间把画面推迟到与声音对齐后再解码显示，见 avsync.ts。
import {
  api,
  toProblem,
  type Codec,
  type DisplayInfo,
  type InputEvent,
  type MouseButton,
  type Problem,
  type QualitySettings,
  type SessionEvent,
} from "./api";
import { AudioPlayer } from "./audio";
import { AvSync, type AvSyncStats } from "./avsync";
import { annexBToAvc, avcC, codecString, h264NeedsAvc, markH264NeedsAvc } from "./codec";
import { t } from "./i18n";
import { RECOVERS, type SessionNotice } from "./messages";
import { CODEC_LABEL } from "./presets";
import { store } from "./store";

export type SessionState = "connecting" | "live" | "reconnecting" | "closed";
export type ScaleMode = "fit" | "native";

export interface Stats {
  fps: number;
  kbps: number;
  rttMs: number | null;
  width: number;
  height: number;
  decodeMs: number;
}

export interface EncoderState {
  codec: Codec;
  name: string;
  hardware: boolean;
  width: number;
  height: number;
}

const CODEC_BY_ID: Codec[] = ["H264", "H265", "AV1"];
const BUTTONS: MouseButton[] = ["Left", "Middle", "Right", "Back", "Forward"];
const MOVE_INTERVAL_MS = 8;
const BACKGROUND_FPS = 5;
const MAX_RETRIES = 30;
/** 二进制包头长度，见 crates/airhand-link/src/controller/session.rs */
const HEADER_LEN = 24;
/** 等待显示的画面帧太多（异常情况）时全部立即解码 */
const MAX_QUEUED_FRAMES = 120;
/** 解码器送进去这么多帧、过了这么久还一帧都没输出，就认为本机解不了这种格式 */
const UNDECODABLE_CHUNKS = 10;
const UNDECODABLE_MS = 2000;

/** 本机（控制端）是 macOS：键盘事件有两处和其它系统不一样，见 bindInput */
const IS_MAC = typeof navigator !== "undefined" && /Mac/.test(navigator.platform || navigator.userAgent);
const META_KEYS = new Set(["MetaLeft", "MetaRight"]);
const MODIFIER_KEYS = new Set([
  "ShiftLeft",
  "ShiftRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "MetaLeft",
  "MetaRight",
  "CapsLock",
  "Fn",
]);

interface PendingVideo {
  key: boolean;
  codec: Codec;
  seq: number;
  width: number;
  height: number;
  hostUs: number;
  data: Uint8Array;
  /** 送去解码的本地时刻 */
  due: number;
}

let nextId = 1;

export class RemoteSession {
  readonly id = `s${nextId++}`;
  readonly code: string;
  readonly canvas: HTMLCanvasElement;

  state: SessionState = "connecting";
  /** 上次断开的原因（显示时才变成文字，见 messages.ts） */
  closedReason: Problem | null = null;
  sid: number | null = null;
  hostName = "";
  hostOs = "";
  hostVersion = "";
  displays: DisplayInfo[] = [];
  currentDisplay = 0;
  hostCodecs: Codec[] = ["H264"];
  encoder: EncoderState | null = null;
  stats: Stats = { fps: 0, kbps: 0, rttMs: null, width: 0, height: 0, decodeMs: 0 };
  /** 画面上方的提示（显示时才变成文字，见 messages.ts） */
  notice: SessionNotice | null = null;
  /** 被控端为什么采集不到声音（英文技术信息）；正常、或者没开声音时为 null */
  audioError: string | null = null;
  settings: QualitySettings;
  scaleMode: ScaleMode = "fit";
  /** 双向同步剪贴板文本 */
  clipboardSync = true;
  /** 播放远程声音（用户开关；实际只在当前查看的会话上播放） */
  audioEnabled = true;
  /** 音画同步开关（用户设置；实际只在播放声音时生效） */
  avSync = false;
  /** 音画同步统计（约每秒更新） */
  syncStats: AvSyncStats = { skewMs: null, videoDelayMs: 0, extraMs: 0 };
  retries = 0;

  /** 变化计数，供 React useSyncExternalStore 使用 */
  version = 0;
  private listeners = new Set<() => void>();

  private ctx: CanvasRenderingContext2D;
  private gen = 0;
  private disposed = false;
  private retryTimer: number | undefined;

  // 解码
  private decoder: VideoDecoder | null = null;
  private decoderKey = "";
  private waitKey = true;
  private lastKeyReq = 0;
  private pendingDecode = new Map<number, { t0: number; hostUs: number }>();
  private decodeAvg = 0;
  /** 当前解码器用的是 AVC 格式（WebKitGTK 不支持 Annex-B，见 codec.ts 的 annexBToAvc） */
  private avcMode = false;
  private decoderCodec: Codec = "H264";
  /** 当前解码器配置后送进去的帧数 / 成功输出的帧数 / 配置时刻（用来判断本机能不能解这种格式） */
  private chunksSinceConfig = 0;
  private decodedSinceConfig = 0;
  private configuredAt = 0;

  // 声音
  readonly audio = new AudioPlayer();
  private readonly sync = new AvSync();
  /** 音画同步时等待解码的画面帧（按顺序） */
  private videoQueue: PendingVideo[] = [];
  private videoTimer: number | undefined;
  /** 最近一次画出的帧的采集时间（被控端时钟，微秒），调试用 */
  lastDrawnHostUs = 0;
  private foreground = false;
  /** 当前是否已让被控端发送声音 */
  private audioOn = false;

  // 布局
  private hostEl: HTMLElement | null = null;
  private resizeObs: ResizeObserver | null = null;

  // 输入
  private interactive = false;
  private visible = true;
  private queue: InputEvent[] = [];
  private moveTimer: number | undefined;
  private flushQueued = false;
  private wheelAcc = { x: 0, y: 0 };
  private pressed = new Set<string>();

  // 光标
  private cursorRaw: { w: number; h: number; hx: number; hy: number; rgba: Uint8ClampedArray } | null = null;
  private cursorVisible = true;
  private cursorCss = "default";
  private cursorScale = 0;

  constructor(code: string, settings: QualitySettings) {
    this.code = code;
    this.settings = settings;
    this.canvas = document.createElement("canvas");
    this.canvas.className = "remote-canvas";
    this.canvas.tabIndex = 0;
    this.canvas.width = 16;
    this.canvas.height = 9;
    const ctx = this.canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) throw new Error(t("error.canvas"));
    this.ctx = ctx;
    this.bindInput();
    this.audio.onClock = (hostUs, localMs, appliedMs) => {
      this.sync.onAudioClock(hostUs, localMs, appliedMs);
      const extra = this.sync.update(performance.now());
      if (extra !== null) this.audio.setExtraDelay(extra);
    };
    this.audio.onUnsupported = () => {
      this.notice = { kind: "noOpus" };
      this.emit();
    };
  }

  // ───────────────────────── 订阅 ─────────────────────────

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  };
  getVersion = () => this.version;

  private emit() {
    this.version++;
    for (const l of this.listeners) l();
  }

  // ───────────────────────── 连接 ─────────────────────────

  /** 建立连接。首次连接失败会抛出错误。 */
  async start(): Promise<void> {
    const gen = ++this.gen;
    // 重连后旧连接的数据可能还会到达，按 gen 丢掉
    const sink = {
      packet: (data: Uint8Array) => {
        if (gen === this.gen) this.onBinary(data);
      },
      event: (ev: SessionEvent) => {
        if (gen === this.gen) this.onEvent(ev);
      },
    };
    this.resetDecoder();
    this.audio.reset();
    this.sync.reset();
    // 在连接之前清掉旧提示：被控端紧跟在 Hello 后面发的提示可能比 connect 的返回值先到
    this.notice = null;
    this.audioError = null;
    const info = await api.connect(this.code, this.settings, sink, this.clipboardSync);
    if (gen !== this.gen || this.disposed) {
      void api.disconnect(info.sid);
      return;
    }
    this.sid = info.sid;
    this.hostName = info.hostName;
    this.hostOs = info.hostOs;
    this.state = "live";
    this.retries = 0;
    this.closedReason = null;
    if (!this.visible) this.pushSettings();
    // 新的会话流默认不发声音，需要重新请求
    this.audioOn = false;
    this.syncAudio();
    this.emit();
  }

  private scheduleRetry(reason: Problem | null) {
    if (this.disposed) return;
    this.sid = null;
    this.syncAudio();
    this.closedReason = reason;
    if (this.retries >= MAX_RETRIES) {
      this.state = "closed";
      this.emit();
      return;
    }
    this.state = "reconnecting";
    this.emit();
    const delay = Math.min(1000 * 2 ** Math.min(this.retries, 3), 8000);
    this.retries++;
    clearTimeout(this.retryTimer);
    this.retryTimer = window.setTimeout(() => {
      this.start().catch((e) => this.scheduleRetry(toProblem(e)));
    }, delay);
  }

  /** 手动重连（关闭状态下） */
  reconnect() {
    this.retries = 0;
    this.scheduleRetry(this.closedReason);
  }

  dispose() {
    this.disposed = true;
    this.gen++;
    clearTimeout(this.retryTimer);
    clearTimeout(this.moveTimer);
    clearTimeout(this.videoTimer);
    this.videoQueue = [];
    if (this.sid !== null) void api.disconnect(this.sid);
    this.sid = null;
    this.state = "closed";
    this.decoder?.close();
    this.decoder = null;
    this.audio.disable();
    this.detach();
    this.emit();
  }

  // ───────────────────────── 事件 ─────────────────────────

  private onEvent(ev: SessionEvent) {
    switch (ev.type) {
      case "hello":
        this.hostName = ev.hostname;
        this.hostOs = ev.os;
        this.hostVersion = ev.version;
        this.displays = ev.displays;
        this.currentDisplay = ev.current;
        this.hostCodecs = ev.codecs;
        break;
      case "displays":
        this.displays = ev.displays;
        this.currentDisplay = ev.current;
        this.cursorScale = 0;
        break;
      case "stats":
        this.stats = {
          fps: ev.fps,
          kbps: ev.kbps,
          rttMs: ev.rttMs,
          width: ev.width,
          height: ev.height,
          decodeMs: this.decodeAvg,
        };
        this.syncStats = this.sync.stats();
        break;
      case "encoder":
        this.encoder = { codec: ev.codec, name: ev.name, hardware: ev.hardware, width: ev.width, height: ev.height };
        break;
      case "notice": {
        const n = ev.notice;
        if (n.kind === "captureRecovered" || n.kind === "audioRecovered") {
          // 对方的问题自己恢复了：撤下对应的提示（别的提示不动）
          if (n.kind === "audioRecovered") this.audioError = null;
          if (this.notice?.kind === RECOVERS[n.kind]) this.notice = null;
        } else if (n.kind === "missingPermissions" && !n.screen && !n.input) {
          // 对方把权限都打开了：撤下之前的权限提示
          if (this.notice?.kind === "missingPermissions") this.notice = null;
        } else if (n.kind === "audioFailed") {
          this.audioError = n.detail;
          // 声音的问题不盖住正在显示的其它提示（例如缺少权限、Wayland），原因在声音菜单里一直能看到
          if (!this.notice || this.notice.kind === "audioFailed") this.notice = n;
        } else {
          this.notice = n;
        }
        break;
      }
      case "closed":
        if (this.disposed) return;
        if (ev.retry) {
          this.scheduleRetry(ev.reason);
        } else {
          // 对方主动断开：不自动重连
          this.sid = null;
          this.syncAudio();
          this.closedReason = ev.reason;
          this.state = "closed";
          this.emit();
        }
        return;
    }
    this.emit();
  }

  /** 一个数据包（可能是更大的缓冲区中的一段：网页版前面还有 4 字节的 ch） */
  private onBinary(pkt: Uint8Array) {
    if (pkt.byteLength < HEADER_LEN) return;
    const dv = new DataView(pkt.buffer, pkt.byteOffset, pkt.byteLength);
    const kind = dv.getUint8(0);
    // 采集时间（被控端时钟，微秒）；在 2^53 以内，转成 number 不丢精度
    const hostUs = Number(dv.getBigUint64(16, true));
    if (kind === 1) this.onVideo(dv, pkt, hostUs);
    else if (kind === 2) this.onCursor(dv, pkt);
    else if (kind === 3) this.audio.push(dv.getUint8(1) === 1, hostUs, pkt.subarray(HEADER_LEN));
  }

  // ───────────────────────── 解码 ─────────────────────────

  private resetDecoder() {
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    this.decoder = null;
    this.decoderKey = "";
    this.waitKey = true;
    this.pendingDecode.clear();
    clearTimeout(this.videoTimer);
    this.videoTimer = undefined;
    this.videoQueue = [];
  }

  private requestKeyframe() {
    const now = performance.now();
    if (this.sid === null || now - this.lastKeyReq < 500) return;
    this.lastKeyReq = now;
    void api.keyframe(this.sid).catch(() => {});
  }

  /** 音画同步当前是否生效（开关打开且正在播放声音） */
  get syncActive(): boolean {
    return this.sync.enabled;
  }

  private onVideo(dv: DataView, pkt: Uint8Array, hostUs: number) {
    const key = dv.getUint8(1) === 1;
    const v: PendingVideo = {
      key,
      codec: CODEC_BY_ID[dv.getUint16(2, true)] ?? "H264",
      seq: dv.getUint32(4, true),
      width: dv.getUint16(8, true),
      height: dv.getUint16(10, true),
      hostUs,
      data: pkt.subarray(HEADER_LEN),
      due: 0,
    };
    if (key) {
      this.waitKey = false;
    } else if (this.waitKey) {
      // 还没有关键帧（刚连接或解码器重置）：丢弃并请求关键帧
      this.requestKeyframe();
      return;
    }
    const now = performance.now();
    // 同步没生效时（关闭或静音）返回 now，画面照常立即显示
    v.due = this.sync.onVideo(hostUs, now, this.decodeAvg);
    // 先处理到期的（窗口在后台时定时器会被节流，每来一帧都检查一次）
    this.pumpVideo();
    if (this.videoQueue.length === 0 && v.due <= now + 1) {
      this.decodeVideo(v);
      return;
    }
    this.videoQueue.push(v);
    if (this.videoQueue.length > MAX_QUEUED_FRAMES) this.flushVideo();
    else this.scheduleVideo();
  }

  /** 按到期时间把排队的帧送去解码 */
  private pumpVideo = () => {
    clearTimeout(this.videoTimer);
    this.videoTimer = undefined;
    const now = performance.now();
    while (this.videoQueue.length > 0 && this.videoQueue[0].due <= now + 1) this.decodeVideo(this.videoQueue.shift()!);
    this.scheduleVideo();
  };

  private scheduleVideo() {
    if (this.videoTimer !== undefined || this.videoQueue.length === 0) return;
    this.videoTimer = window.setTimeout(this.pumpVideo, Math.max(0, this.videoQueue[0].due - performance.now()));
  }

  /** 关闭音画同步等：排队的帧立即解码 */
  private flushVideo() {
    clearTimeout(this.videoTimer);
    this.videoTimer = undefined;
    const q = this.videoQueue;
    this.videoQueue = [];
    for (const v of q) this.decodeVideo(v);
  }

  private decodeVideo(v: PendingVideo) {
    let data = v.data;
    if (v.key) {
      const codecStr = codecString(v.codec, v.data);
      // 本机 WebView 不支持 Annex-B H.264（WebKitGTK）：转换为 AVC 格式，参数集放进 description
      let description: Uint8Array | undefined;
      if (v.codec === "H264" && h264NeedsAvc()) {
        const conv = annexBToAvc(v.data);
        if (!conv.sps || !conv.pps) {
          this.waitKey = true;
          this.requestKeyframe();
          return;
        }
        description = avcC(conv.sps, conv.pps);
        data = conv.data;
      }
      const cfgKey = `${codecStr}/${v.width}x${v.height}/${description?.join(",") ?? "annexb"}`;
      if (!this.decoder || this.decoder.state === "closed" || cfgKey !== this.decoderKey) {
        this.configureDecoder(v.codec, codecStr, v.width, v.height, cfgKey, description);
      }
    } else if (!this.decoder || this.decoder.state === "closed") {
      // 解码器出错被重置：等下一个关键帧
      this.waitKey = true;
      this.requestKeyframe();
      return;
    }

    const dec = this.decoder!;
    // 送进去不少帧了却一帧也没解出来，也没报错（例如 WebKitGTK 的部分 HEVC/AV1 解码器）：本机实际无法解码这种格式
    if (
      this.decodedSinceConfig === 0 &&
      this.chunksSinceConfig >= UNDECODABLE_CHUNKS &&
      performance.now() - this.configuredAt > UNDECODABLE_MS
    ) {
      this.onUndecodable(this.decoderCodec);
      return;
    }
    if (dec.decodeQueueSize > 8) {
      // 解码跟不上（极少见）：清空并从下一个关键帧重新开始
      this.resetDecoder();
      this.requestKeyframe();
      return;
    }
    if (this.avcMode && !v.key) data = annexBToAvc(v.data).data;
    this.chunksSinceConfig++;
    const ts = v.seq * 1000;
    this.pendingDecode.set(ts, { t0: performance.now(), hostUs: v.hostUs });
    try {
      dec.decode(new EncodedVideoChunk({ type: v.key ? "key" : "delta", timestamp: ts, data }));
    } catch {
      this.resetDecoder();
      this.requestKeyframe();
    }
  }

  /**
   * 本机实际无法解码这种格式（探测时说支持，但一帧也解不出来）：
   * H.264 改用 AVC 格式重试；其它格式从可用列表里去掉，并让被控端改用 H.264。
   */
  private onUndecodable(codec: Codec) {
    this.resetDecoder();
    if (codec === "H264") {
      if (h264NeedsAvc()) {
        this.notice = { kind: "h264Undecodable" };
        this.emit();
      } else {
        markH264NeedsAvc();
      }
      this.requestKeyframe();
      return;
    }
    console.warn(`can't decode ${codec} here, switching to H.264`);
    store.setDecoders(store.get().decoders.filter((c) => c !== codec));
    if (this.settings.codec === codec) {
      this.notice = { kind: "codecFallback", codec: CODEC_LABEL[codec] };
      this.updateSettings({ ...this.settings, codec: "H264" });
    }
    this.requestKeyframe();
  }

  private configureDecoder(
    kind: Codec,
    codec: string,
    width: number,
    height: number,
    cfgKey: string,
    description?: Uint8Array,
  ) {
    if (this.decoder && this.decoder.state !== "closed") this.decoder.close();
    const avc = description !== undefined;
    this.avcMode = avc;
    this.decoderCodec = kind;
    this.decodedSinceConfig = 0;
    this.chunksSinceConfig = 0;
    this.configuredAt = performance.now();
    const decoder = new VideoDecoder({
      output: (frame) => {
        if (this.decoder === decoder && this.decodedSinceConfig++ === 0 && this.notice?.kind === "h264Undecodable") {
          this.notice = null;
          this.emit();
        }
        this.draw(frame);
      },
      error: (e) => {
        console.warn("video decoder error", e);
        // 一帧都没解出来就出错：本机不支持这种码流（例如 WebKitGTK 不支持 Annex-B 的 H.264）
        if (this.decoder === decoder && this.decodedSinceConfig === 0) {
          this.onUndecodable(kind);
          return;
        }
        if (this.decoder === decoder) {
          this.decoder = null;
          this.waitKey = true;
          this.requestKeyframe();
        }
      },
    });
    decoder.configure({
      codec,
      codedWidth: width,
      codedHeight: height,
      optimizeForLatency: true,
      hardwareAcceleration: "no-preference",
      ...(description ? { description } : {}),
    });
    this.decoder = decoder;
    this.decoderKey = cfgKey;
  }

  private draw(frame: VideoFrame) {
    const pending = this.pendingDecode.get(frame.timestamp);
    if (pending !== undefined) {
      this.pendingDecode.delete(frame.timestamp);
      const ms = performance.now() - pending.t0;
      this.decodeAvg = this.decodeAvg === 0 ? ms : this.decodeAvg * 0.9 + ms * 0.1;
      this.lastDrawnHostUs = pending.hostUs;
    }
    if (this.pendingDecode.size > 32) this.pendingDecode.clear();
    const w = frame.displayWidth;
    const h = frame.displayHeight;
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.layout();
    }
    this.ctx.drawImage(frame, 0, 0);
    frame.close();
  }

  // ───────────────────────── 光标 ─────────────────────────

  private onCursor(dv: DataView, pkt: Uint8Array) {
    this.cursorVisible = dv.getUint8(1) === 1;
    if (dv.getUint16(2, true) === 1) {
      const w = dv.getUint16(8, true);
      const h = dv.getUint16(10, true);
      if (w > 0 && h > 0 && pkt.byteLength >= HEADER_LEN + w * h * 4) {
        this.cursorRaw = {
          w,
          h,
          hx: dv.getUint16(12, true),
          hy: dv.getUint16(14, true),
          rgba: new Uint8ClampedArray(pkt.buffer, pkt.byteOffset + HEADER_LEN, w * h * 4),
        };
        this.cursorScale = 0;
      }
    }
    this.applyCursor();
  }

  /** 按当前显示比例缩放远端光标，生成 CSS cursor */
  private applyCursor() {
    if (!this.cursorVisible) {
      this.setCursor("none");
      return;
    }
    const c = this.cursorRaw;
    if (!c) {
      this.setCursor("default");
      return;
    }
    const display = this.displays[this.currentDisplay];
    const srcWidth = display?.width || this.canvas.width;
    const cssWidth = this.canvas.getBoundingClientRect().width || this.canvas.width;
    // 远端物理像素 → 本地 CSS 像素；限制在 Chromium 支持的 128px 以内
    let scale = cssWidth / srcWidth;
    scale = Math.min(Math.max(scale, 0.35), 2, 128 / Math.max(c.w, c.h));
    if (Math.abs(scale - this.cursorScale) < 0.02 && this.cursorCss !== "none") return;
    this.cursorScale = scale;
    const src = new OffscreenCanvas(c.w, c.h);
    src.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(c.rgba), c.w, c.h), 0, 0);
    const w = Math.max(1, Math.round(c.w * scale));
    const h = Math.max(1, Math.round(c.h * scale));
    const out = document.createElement("canvas");
    out.width = w;
    out.height = h;
    const octx = out.getContext("2d")!;
    octx.imageSmoothingQuality = "high";
    octx.drawImage(src, 0, 0, w, h);
    const hx = Math.min(w - 1, Math.round(c.hx * scale));
    const hy = Math.min(h - 1, Math.round(c.hy * scale));
    this.setCursor(`url(${out.toDataURL("image/png")}) ${hx} ${hy}, default`);
  }

  private setCursor(css: string) {
    this.cursorCss = css;
    this.canvas.style.cursor = css;
  }

  // ───────────────────────── 布局 ─────────────────────────

  attach(host: HTMLElement) {
    this.detach();
    this.hostEl = host;
    host.appendChild(this.canvas);
    this.resizeObs = new ResizeObserver(() => this.layout());
    this.resizeObs.observe(host);
    this.layout();
  }

  detach() {
    this.resizeObs?.disconnect();
    this.resizeObs = null;
    if (this.hostEl && this.canvas.parentElement === this.hostEl) this.hostEl.removeChild(this.canvas);
    this.hostEl = null;
  }

  setScaleMode(m: ScaleMode) {
    this.scaleMode = m;
    this.layout();
    this.emit();
  }

  private layout() {
    const host = this.hostEl;
    if (!host) return;
    const vw = this.canvas.width;
    const vh = this.canvas.height;
    let w: number;
    let h: number;
    if (this.scaleMode === "native" && this.interactive) {
      w = vw / devicePixelRatio;
      h = vh / devicePixelRatio;
    } else {
      const s = Math.min(host.clientWidth / vw, host.clientHeight / vh);
      w = Math.max(1, Math.floor(vw * s));
      h = Math.max(1, Math.floor(vh * s));
    }
    this.canvas.style.width = `${w}px`;
    this.canvas.style.height = `${h}px`;
    this.applyCursor();
  }

  // ───────────────────────── 可见性 / 设置 ─────────────────────────

  /** 不可见（例如切到其它会话）时降帧率，节省带宽 */
  setVisible(v: boolean) {
    if (this.visible === v) return;
    this.visible = v;
    this.pushSettings();
    if (v) this.requestKeyframe();
  }

  /** 是否为当前正在查看的会话（只有它播放声音） */
  setForeground(v: boolean) {
    if (this.foreground === v) return;
    this.foreground = v;
    this.syncAudio();
  }

  setAudioEnabled(v: boolean) {
    this.audioEnabled = v;
    this.syncAudio();
    this.emit();
  }

  /** 音画同步开关：打开后画面会推迟到与声音对齐（操作反馈也会跟着变慢） */
  setAvSync(v: boolean) {
    this.avSync = v;
    this.applySyncState();
    this.emit();
  }

  /** 同步只在开关打开且正在播放声音时生效；不生效时排队的画面立即显示，声音也不再为画面额外等待 */
  private applySyncState() {
    this.sync.enabled = this.avSync && this.audioOn;
    if (!this.sync.enabled) this.flushVideo();
    const extra = this.sync.update(performance.now());
    if (extra !== null) this.audio.setExtraDelay(extra);
    this.syncStats = this.sync.stats();
  }

  private syncAudio() {
    const want = this.audioEnabled && this.foreground && this.sid !== null && !this.disposed;
    if (want === this.audioOn) return;
    this.audioOn = want;
    if (want) this.audio.enable();
    else this.audio.disable();
    this.applySyncState();
    // 关闭时也通知被控端停止采集，不占带宽
    if (this.sid !== null) void api.audio(this.sid, want).catch(() => {});
    // 被控端不再采集，之前"无法采集声音"的状态也就作废了（再打开时它会重新报告）
    if (!want) this.clearAudioError();
  }

  private clearAudioError() {
    const shown = this.notice?.kind === "audioFailed";
    if (this.audioError === null && !shown) return;
    this.audioError = null;
    if (shown) this.notice = null;
    this.emit();
  }

  setInteractive(v: boolean) {
    this.interactive = v;
    if (!v) this.releaseAll();
    this.layout();
  }

  updateSettings(s: QualitySettings) {
    this.settings = s;
    this.pushSettings();
    this.emit();
  }

  private pushSettings() {
    if (this.sid === null) return;
    const s = this.visible ? this.settings : { ...this.settings, fps: Math.min(this.settings.fps, BACKGROUND_FPS) };
    void api.settings(this.sid, s).catch(() => {});
  }

  selectDisplay(index: number) {
    if (this.sid === null || index === this.currentDisplay) return;
    this.releaseAll();
    void api.selectDisplay(this.sid, index).catch(() => {});
  }

  setClipboardSync(v: boolean) {
    this.clipboardSync = v;
    if (this.sid !== null) void api.clipboardSync(this.sid, v).catch(() => {});
    this.emit();
  }

  dismissNotice() {
    this.notice = null;
    this.emit();
  }

  // ───────────────────────── 输入 ─────────────────────────

  /** 发送一组按键（例如 Win、Alt+Tab 等无法在本地直接按下的组合键） */
  sendCombo(codes: string[]) {
    const evs: InputEvent[] = [];
    for (const code of codes) evs.push({ Key: { code, down: true } });
    for (const code of [...codes].reverse()) evs.push({ Key: { code, down: false } });
    this.enqueue(evs, true);
  }

  sendText(text: string) {
    if (!text) return;
    // 统一换行，分块发送避免单条消息过大
    const t = text.replace(/\r\n/g, "\n").replace(/\n/g, "\r");
    for (let i = 0; i < t.length; i += 512) this.enqueue([{ Text: { text: t.slice(i, i + 512) } }], true);
  }

  focus() {
    this.canvas.focus({ preventScroll: true });
  }

  private releaseAll() {
    this.pressed.clear();
    this.enqueue(["ReleaseAll"], true);
  }

  /**
   * macOS 的 Caps Lock：打开时只有 keydown，关闭时只有 keyup（中间不会抬起）。
   * 每次都当作"按一下"发给对方，两边的大小写锁定状态才能保持一致。
   */
  private tapCapsLock() {
    this.enqueue(
      [{ Key: { code: "CapsLock", down: true } }, { Key: { code: "CapsLock", down: false } }],
      true,
    );
  }

  private enqueue(evs: InputEvent[], urgent: boolean) {
    if (this.sid === null) return;
    for (const ev of evs) {
      const last = this.queue[this.queue.length - 1];
      if (typeof ev === "object" && "MouseMove" in ev && last && typeof last === "object" && "MouseMove" in last) {
        this.queue[this.queue.length - 1] = ev;
      } else {
        this.queue.push(ev);
      }
    }
    if (urgent) {
      if (!this.flushQueued) {
        this.flushQueued = true;
        queueMicrotask(() => this.flush());
      }
    } else if (this.moveTimer === undefined) {
      this.moveTimer = window.setTimeout(() => this.flush(), MOVE_INTERVAL_MS);
    }
  }

  private flush() {
    this.flushQueued = false;
    clearTimeout(this.moveTimer);
    this.moveTimer = undefined;
    if (this.queue.length === 0 || this.sid === null) {
      this.queue = [];
      return;
    }
    const batch = this.queue;
    this.queue = [];
    void api.input(this.sid, batch).catch(() => {});
  }

  private normPos(e: MouseEvent): { x: number; y: number } {
    const r = this.canvas.getBoundingClientRect();
    return {
      x: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)),
      y: Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)),
    };
  }

  private bindInput() {
    const c = this.canvas;
    const on = <K extends keyof HTMLElementEventMap>(
      type: K,
      fn: (e: HTMLElementEventMap[K]) => void,
      opts?: AddEventListenerOptions,
    ) => c.addEventListener(type, fn as EventListener, opts);

    on("pointermove", (e) => {
      if (!this.interactive) return;
      this.enqueue([{ MouseMove: this.normPos(e) }], false);
    });
    on("pointerdown", (e) => {
      if (!this.interactive) return;
      e.preventDefault();
      this.focus();
      c.setPointerCapture(e.pointerId);
      const button = BUTTONS[e.button];
      if (!button) return;
      this.enqueue([{ MouseMove: this.normPos(e) }, { MouseButton: { button, down: true } }], true);
    });
    on("pointerup", (e) => {
      if (!this.interactive) return;
      e.preventDefault();
      const button = BUTTONS[e.button];
      if (!button) return;
      this.enqueue([{ MouseMove: this.normPos(e) }, { MouseButton: { button, down: false } }], true);
    });
    on("contextmenu", (e) => e.preventDefault());
    on("auxclick", (e) => e.preventDefault());
    on(
      "wheel",
      (e) => {
        if (!this.interactive) return;
        e.preventDefault();
        // 浏览器：像素/行/页 → Windows：120 = 一格（向上为正）
        const unit = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? 120 * 3 : 1.2;
        this.wheelAcc.x += e.deltaX * unit;
        this.wheelAcc.y -= e.deltaY * unit;
        const dx = Math.trunc(this.wheelAcc.x);
        const dy = Math.trunc(this.wheelAcc.y);
        if (dx === 0 && dy === 0) return;
        this.wheelAcc.x -= dx;
        this.wheelAcc.y -= dy;
        const clamp = (v: number) => Math.max(-32000, Math.min(32000, v));
        this.enqueue([{ Wheel: { dx: clamp(dx), dy: clamp(dy) } }], true);
      },
      { passive: false },
    );
    on("keydown", (e) => {
      if (!this.interactive) return;
      e.preventDefault();
      e.stopPropagation();
      if (!e.code || e.code === "Unidentified") return;
      if (IS_MAC && e.code === "CapsLock") return this.tapCapsLock();
      if (e.repeat && !this.pressed.has(e.code)) return;
      this.pressed.add(e.code);
      this.enqueue([{ Key: { code: e.code, down: true } }], true);
    });
    on("keyup", (e) => {
      if (!this.interactive) return;
      e.preventDefault();
      e.stopPropagation();
      if (!e.code || e.code === "Unidentified") return;
      if (IS_MAC && e.code === "CapsLock") return this.tapCapsLock();
      this.pressed.delete(e.code);
      const evs: InputEvent[] = [];
      // macOS：按住 Command 时其它键抬起不产生 keyup（例如 Command+C 只有 C 的 keydown），
      // Command 抬起时把这期间按下的普通键一起抬起，否则对方那边会一直按着
      if (IS_MAC && META_KEYS.has(e.code)) {
        for (const code of this.pressed) {
          if (MODIFIER_KEYS.has(code)) continue;
          this.pressed.delete(code);
          evs.push({ Key: { code, down: false } });
        }
      }
      evs.push({ Key: { code: e.code, down: false } });
      this.enqueue(evs, true);
    });
    // 失焦（Alt+Tab 切走等）时释放所有按键，避免对端粘键
    on("blur", () => this.releaseAll());
  }
}
