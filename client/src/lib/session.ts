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
  type Problem,
  type QualitySettings,
  type ResumeToken,
  type SessionEvent,
  type Platform,
  type SystemAction,
} from "./api";
import { AudioPlayer } from "./audio";
import { ConnectionAttempt } from "./connection-attempt";
import { RemoteInput } from "./input";
import { AvSync, type AvSyncStats } from "./avsync";
import { bestCodec, codecUsable } from "./codec";
import { VideoPlayer } from "./video";
import { t } from "./i18n";
import { RECOVERS, type SessionNotice } from "./messages";
import { CODEC_LABEL } from "./presets";
import { store } from "./store";

/** waiting = 对方掉线了，等它重新上线（服务器会通知，见 hostOnline） */
/** `approving`：连上了，在等对方的用户同意（之后收到 hello 才是 `live`） */
export type SessionState = "connecting" | "approving" | "live" | "reconnecting" | "waiting" | "closed";
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

const BACKGROUND_FPS = 5;
const MAX_RETRIES = 30;
/** 等对方上线时，保险起见每隔这么久也试一次（正常情况下服务器会在对方上线时马上通知） */
const WAIT_CHECK_MS = 60_000;
/** 没连上服务器（网页版还有控制端服务）时多久试一次；连上时马上会试（serverConnected） */
const NO_SERVER_RETRY_MS = 5_000;
/** 服务器的明确回答：自动重试只会被当成在猜控制码 */
const GIVE_UP = new Set<Problem["kind"]>(["codeNotFound", "rateLimited", "invalidCode", "notController"]);
/** 与服务器（网页版还有控制端服务）的连接断了：等重新连上再试，不计入重试次数 */
const NO_SERVER = new Set<Problem["kind"]>(["notConnected", "serverLost", "serverUnreachable", "consoleLost"]);
/** 二进制包头长度，见 crates/airhand-link/src/controller/session.rs */
const HEADER_LEN = 24;

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
  /** 对方报告的平台（hello 之前为 null，这时按 hostOs 猜） */
  hostPlatform: Platform | null = null;
  /** 对方支持的系统操作（手机的返回、主页……）；电脑为空 */
  hostActions: SystemAction[] = [];
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

  /** 连上了（第一次和每次重连）：记进我的设备 */
  onConnected: ((s: RemoteSession) => void) | null = null;
  /** 画质设置变了：记进我的设备，下次用同样的画质连 */
  onSettingsChange: ((s: RemoteSession) => void) | null = null;

  /** 变化计数，供 React useSyncExternalStore 使用 */
  version = 0;
  private listeners = new Set<() => void>();

  private readonly video: VideoPlayer;
  private readonly attempt = new ConnectionAttempt();
  private disposed = false;
  private retryTimer: number | undefined;
  /** 控制码（数字），用来认出 hostOnline 通知 */
  private readonly codeNum: number;
  /** 重连凭证：连上过就有，重连时带上（对方换了控制码就作废） */
  private resume: ResumeToken | null = null;
  /** 最近一次开始连接、最近一次收到对方上线通知的时刻 */
  private attemptAt = 0;
  private onlineAt = -1;

  private lastKeyReq = 0;

  // 声音
  readonly audio = new AudioPlayer();
  private readonly sync = new AvSync();
  /** 最近一次画出的帧的采集时间（被控端时钟，微秒），调试用 */
  get lastDrawnHostUs() { return this.video.lastDrawnHostUs; }
  private foreground = false;
  /** 当前是否已让被控端发送声音 */
  private audioOn = false;

  // 布局
  private hostEl: HTMLElement | null = null;
  private resizeObs: ResizeObserver | null = null;

  // 输入
  private interactive = false;
  private visible = true;
  private readonly input: RemoteInput;

  // 光标
  private cursorRaw: { w: number; h: number; hx: number; hy: number; rgba: Uint8ClampedArray } | null = null;
  private cursorVisible = true;
  private cursorCss = "default";
  private cursorScale = 0;

  /**
   * `saved`：我的设备里记着的这台电脑。带着上次的重连凭证连，对方不在线时等它上线（见 [`open`]），
   * 连上之前标签上就显示它的名字
   */
  constructor(code: string, settings: QualitySettings, saved?: { resume: ResumeToken | null; name: string; os: string }) {
    this.code = code;
    this.codeNum = Number(code.replace(/\D/g, ""));
    this.settings = settings;
    if (saved) {
      this.resume = saved.resume;
      this.hostName = saved.name;
      this.hostOs = saved.os;
    }
    this.canvas = document.createElement("canvas");
    this.canvas.className = "remote-canvas";
    this.canvas.tabIndex = 0;
    this.canvas.width = 16;
    this.canvas.height = 9;
    const ctx = this.canvas.getContext("2d", { alpha: false, desynchronized: true });
    if (!ctx) throw new Error(t("error.canvas"));
    this.video = new VideoPlayer({
      canvas: this.canvas, ctx, sync: this.sync,
      requestKeyframe: () => this.requestKeyframe(),
      unsupported: (codec) => this.onUndecodable(codec),
      resized: () => this.layout(),
      recovered: () => {
        if (this.notice?.kind === "h264Undecodable") { this.notice = null; this.emit(); }
      },
    });
    this.input = new RemoteInput(this.canvas, () => this.sid !== null && !this.disposed, (events) => {
      if (this.sid !== null) void api.input(this.sid, events).catch(() => {});
    });
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
    const gen = this.attempt.begin();
    // 重连后旧连接的数据可能还会到达，按 gen 丢掉
    const sink = {
      packet: (data: Uint8Array) => {
        if (this.attempt.accepts(gen)) this.onBinary(data);
      },
      event: (ev: SessionEvent) => {
        if (this.attempt.accepts(gen)) this.onEvent(ev);
      },
    };
    this.video.reset();
    this.audio.reset();
    this.sync.reset();
    // 在连接之前清掉旧提示：被控端紧跟在 Hello 后面发的提示可能比 connect 的返回值先到
    this.notice = null;
    this.audioError = null;
    this.attemptAt = performance.now();
    let info;
    const sent = this.settings;
    try {
      info = await api.connect(this.code, this.resume, this.settings, sink, this.clipboardSync);
    } catch (e) {
      // A Closed event already decided whether to retry. An old failure must
      // not overwrite that decision, or restart a deliberately declined call.
      if (!this.attempt.accepts(gen) || this.disposed) return;
      throw e;
    } finally {
      this.attempt.finish(gen);
    }
    if (!this.attempt.accepts(gen) || this.disposed) {
      void api.disconnect(info.sid).catch(() => {});
      return;
    }
    this.resume = info.resume;
    this.sid = info.sid;
    this.hostName = info.hostName;
    this.hostOs = info.hostOs;
    // 收到对方的 hello 才算开始（对方可能要先问它的用户，见 onEvent 的 awaitingApproval）；
    // hello 也可能比这里先到，那时已经是 live 了
    this.retries = 0;
    this.closedReason = null;
    // 在后台要降帧率；连接过程中设置变了（例如 Hello 先到，换了编码格式，那时还没有 sid 发不出去）也要重发
    if (!this.visible || this.settings !== sent) this.pushSettings();
    // 新的会话流默认不发声音，需要重新请求
    this.audioOn = false;
    this.syncAudio();
    this.emit();
    this.onConnected?.(this);
  }

  /**
   * 第一次连接。带着重连凭证（我的设备里保存的）时，对方不在线不算失败：
   * 进入"等待对方上线"，对方一上线就连上（和断线后等对方一样）。其它失败照样抛出
   */
  async open(): Promise<void> {
    try {
      await this.start();
    } catch (e) {
      const p = toProblem(e);
      if (p.kind !== "hostOffline" || this.disposed) throw e;
      this.scheduleRetry(p);
    }
  }

  /** 重连凭证（连上过才有）：记进我的设备，下次对方不在线时可以等它 */
  get resumeToken(): ResumeToken | null {
    return this.resume;
  }

  /**
   * 会话断了 / 重连失败之后：按原因决定什么时候再试。
   *
   * 重连都带着凭证，服务器不会把它们当成猜控制码，对方不在线时回答 hostOffline 并在对方上线时通知，
   * 所以等对方的时候不用反复尝试，也不限次数。
   */
  private scheduleRetry(reason: Problem | null) {
    if (this.disposed) return;
    this.sid = null;
    this.syncAudio();
    this.closedReason = reason;
    clearTimeout(this.retryTimer);
    const kind = reason?.kind;
    let delay: number;
    if (kind === "hostOffline") {
      this.state = "waiting";
      this.retries = 0;
      // 上线通知和 connect 的回答不走同一条通道，可能先到：这次连接开始之后收到过通知就马上再试
      delay = this.onlineAt >= this.attemptAt ? 0 : WAIT_CHECK_MS;
    } else if (kind && GIVE_UP.has(kind)) {
      this.state = "closed";
      this.emit();
      return;
    } else if (kind && NO_SERVER.has(kind)) {
      this.state = "reconnecting";
      delay = NO_SERVER_RETRY_MS;
    } else {
      if (this.retries >= MAX_RETRIES) {
        this.state = "closed";
        this.emit();
        return;
      }
      this.state = "reconnecting";
      delay = Math.min(1000 * 2 ** Math.min(this.retries, 3), 8000);
      this.retries++;
    }
    this.emit();
    this.retryTimer = window.setTimeout(() => this.retryNow(), delay);
  }

  private retryNow() {
    clearTimeout(this.retryTimer);
    this.retryTimer = undefined;
    if (this.disposed || this.attempt.attempting) return;
    this.start().catch((e) => this.scheduleRetry(toProblem(e)));
  }

  /** 服务器通知：控制码为 `code` 的被控端重新上线了 */
  hostOnline(code: number) {
    if (code !== this.codeNum || this.disposed) return;
    this.onlineAt = performance.now();
    if (this.state === "waiting") this.retryNow();
  }

  /** 与服务器（重新）连上了：服务器不记得断线前谁在等谁，在等的会话马上再试一次 */
  serverConnected() {
    const kind = this.closedReason?.kind;
    if (this.state === "waiting" || (this.state === "reconnecting" && kind && NO_SERVER.has(kind))) this.retryNow();
  }

  /** 手动重连（关闭状态下） */
  reconnect() {
    if (this.disposed) return;
    this.retries = 0;
    this.state = "reconnecting";
    this.emit();
    this.retryNow();
  }

  dispose() {
    this.disposed = true;
    this.attempt.invalidate();
    clearTimeout(this.retryTimer);
    this.input.dispose();
    this.video.reset();
    if (this.sid !== null) void api.disconnect(this.sid).catch(() => {});
    this.sid = null;
    this.state = "closed";
    this.audio.disable();
    this.detach();
    this.emit();
  }

  // ───────────────────────── 事件 ─────────────────────────

  private onEvent(ev: SessionEvent) {
    switch (ev.type) {
      case "awaitingApproval":
        this.state = "approving";
        break;
      case "hello":
        this.state = "live";
        this.hostName = ev.hostname;
        this.hostOs = ev.os;
        this.hostPlatform = ev.platform;
        this.hostActions = ev.actions;
        this.hostVersion = ev.version;
        this.displays = ev.displays;
        this.currentDisplay = ev.current;
        this.hostCodecs = ev.codecs;
        this.fitCodec();
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
          decodeMs: this.video.decodeAvg,
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
          if (this.notice?.kind === "missingPermissions" || this.notice?.kind === "androidNoInput") this.notice = null;
        } else if (n.kind === "missingPermissions" && this.hostPlatform === "Android") {
          // 对方是手机：缺的是 Airhand 的"远程操作"（无障碍服务），不是 Mac 的系统权限
          this.notice = { kind: "androidNoInput" };
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
        this.attempt.invalidate();
        clearTimeout(this.retryTimer);
        this.input.reset();
        this.video.reset();
        if (ev.retry) {
          this.scheduleRetry(ev.reason);
        } else {
          // 对方主动断开：不自动重连
          this.sid = null;
          this.syncAudio();
          this.closedReason = ev.reason;
          // 对方换了控制码：凭证跟着旧控制码作废，之后手动重连就是普通的连接请求
          if (ev.reason.kind === "codeChanged") this.resume = null;
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
    if (kind === 1) this.video.push(dv, pkt, hostUs);
    else if (kind === 2) this.onCursor(dv, pkt);
    else if (kind === 3) this.audio.push(dv.getUint8(1) === 1, hostUs, pkt.subarray(HEADER_LEN));
  }

  // ───────────────────────── 解码 ─────────────────────────

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

  /**
   * 本机实际无法解码这种格式（探测时说支持，但一帧也解不出来）：
   * H.264 改用 AVC 格式重试；其它格式从可用列表里去掉，并让被控端按优先级改用下一个能用的格式。
   */
  private onUndecodable(codec: Codec) {
    if (codec === "H264") {
      this.notice = { kind: "h264Undecodable" };
      this.emit();
      return;
    }
    const decoders = store.get().decoders.filter((c) => c !== codec);
    store.setDecoders(decoders);
    if (this.settings.codec === codec) {
      const to = bestCodec(decoders, this.hostCodecs);
      console.warn(`can't decode ${codec} here, switching to ${to}`);
      this.notice = { kind: "codecFallback", codec: CODEC_LABEL[codec], to: CODEC_LABEL[to] };
      this.updateSettings({ ...this.settings, codec: to });
    } else {
      console.warn(`can't decode ${codec} here`);
    }
    this.requestKeyframe();
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
    if (!this.sync.enabled) this.video.flush();
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
    this.input.interactive = v;
    if (!v) this.releaseAll();
    this.layout();
  }

  updateSettings(s: QualitySettings) {
    this.settings = s;
    this.pushSettings();
    this.emit();
    this.onSettingsChange?.(this);
  }

  /**
   * 知道对方支持哪些格式之后：当前选的格式双方不是都支持时，按优先级（AV1 > HEVC > H.264）换成能用的里最好的。
   * 例如 Mac 被控端没有 AV1 编码器：请求 AV1 时对方自己只会回退到 H.264，这里改为请求 HEVC。
   */
  private fitCodec() {
    const decoders = store.get().decoders;
    if (codecUsable(this.settings.codec, decoders, this.hostCodecs)) return;
    this.updateSettings({ ...this.settings, codec: bestCodec(decoders, this.hostCodecs) });
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

  /** 系统操作（对方是手机时的返回、主页……；只发对方在 hello 里列出的） */
  sendSystem(action: SystemAction) {
    if (this.hostActions.includes(action)) this.enqueue([{ System: { action } }], true);
  }

  sendText(text: string) {
    if (!text) return;
    // 统一换行，分块发送避免单条消息过大
    const t = text.replace(/\r\n/g, "\n").replace(/\n/g, "\r");
    for (let i = 0; i < t.length; i += 512) this.enqueue([{ Text: { text: t.slice(i, i + 512) } }], true);
  }

  focus() { this.input.focus(); }
  private releaseAll() { this.input.releaseAll(); }
  private enqueue(events: InputEvent[], urgent: boolean) { this.input.enqueue(events, urgent); }
}
