// 与 Rust 端的命令 / 数据结构一一对应。
//
// 控制台有两种运行方式，界面完全相同，只是和控制端服务（crates/airhand-link）之间的通道不同（见 Backend）：
// * 桌面版：Tauri 窗口，命令走 invoke，画面走 Channel（backend/tauri.ts）；
// * 网页版：浏览器打开 airhand-web（web/）的端口，都走一条 WebSocket（backend/web.ts）。
import { isTauri } from "@tauri-apps/api/core";
import { tauriBackend } from "./backend/tauri";
import { webBackend } from "./backend/web";
import type { Lang } from "./i18n";

export type Codec = "H264" | "H265" | "AV1";

/** 与 airhand_proto::session::QualitySettings 对应（字段名保持 snake_case） */
export interface QualitySettings {
  fps: number;
  bitrate_kbps: number;
  /** 输出画面短边上限，0 = 原始分辨率 */
  max_short: number;
  adaptive: boolean;
  codec: Codec;
}

export interface DisplayInfo {
  index: number;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
}

export type ServerState =
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "disconnected"; reason: Problem };

/**
 * 显示给用户的问题，与 Rust 端 airhand_link::Problem 一一对应（JSON 以 kind 区分）。
 *
 * Rust 端只传类型，界面显示时才按当前语言变成文字（见 messages.ts 的 problemText），
 * 所以不会出现别的电脑或服务器的语言，切换语言时已显示的问题也会跟着变。
 * `detail` 是技术信息（英文），原样显示。
 */
export type Problem =
  // 与中转服务器的连接
  | { kind: "serverUnreachable"; detail: string }
  | { kind: "certMismatch" }
  | { kind: "incompatible"; server: number; client: number }
  | { kind: "serverLost"; detail: string }
  // 连接被控端
  | { kind: "notConnected" }
  | { kind: "invalidCode" }
  | { kind: "codeNotFound" }
  | { kind: "rateLimited" }
  | { kind: "hostBusy" }
  | { kind: "serverTimeout" }
  // 会话
  | { kind: "hostEnded" }
  | { kind: "hostUnresponsive" }
  | { kind: "kicked" }
  | { kind: "codeChanged" }
  | { kind: "sessionEnded" }
  // 其它
  | { kind: "notController" }
  | { kind: "noSession" }
  | { kind: "noClipboardText" }
  | { kind: "failed"; detail: string }
  // 只在网页版的浏览器里产生（Rust 端没有）
  /** 与 airhand-web 服务的连接断了 */
  | { kind: "consoleLost" }
  /** 浏览器不允许读取剪贴板 */
  | { kind: "clipboardBlocked" };

/** 被控端发来的提示，与 Rust 端 airhand_link::controller::NoticeView 对应 */
export type RemoteNotice =
  | { kind: "noDisplays" }
  | { kind: "wayland" }
  | { kind: "captureFailed"; detail: string }
  /** 屏幕采集恢复：撤下 captureFailed（本身不显示） */
  | { kind: "captureRecovered" }
  /** 被控端无法采集声音（只发给开着声音的控制端） */
  | { kind: "audioFailed"; detail: string }
  /** 声音采集恢复：撤下 audioFailed（本身不显示） */
  | { kind: "audioRecovered" }
  /** 被控端（macOS）缺少的系统权限；两项都为 false 表示已经全部授予 */
  | { kind: "missingPermissions"; screen: boolean; input: boolean };

export interface AppInfo {
  version: string;
  /** 中转服务器地址 */
  server: string;
  pinned: boolean;
  /** 被控端看到的本控制端名称（桌面版是计算机名） */
  hostname: string;
  lang: Lang;
}

export interface SessionInfo {
  sid: number;
  code: string;
  hostName: string;
  hostOs: string;
}

export type SessionEvent =
  | {
      type: "hello";
      hostname: string;
      os: string;
      version: string;
      displays: DisplayInfo[];
      current: number;
      codecs: Codec[];
    }
  | { type: "displays"; displays: DisplayInfo[]; current: number }
  | {
      type: "stats";
      fps: number;
      kbps: number;
      rttMs: number | null;
      width: number;
      height: number;
      frames: number;
    }
  | { type: "encoder"; codec: Codec; name: string; hardware: boolean; width: number; height: number }
  | { type: "notice"; notice: RemoteNotice }
  /** 被控端复制的文本（只有网页版会收到，由 backend/web.ts 处理） */
  | { type: "clipboard"; text: string }
  | { type: "closed"; reason: Problem; retry: boolean };

export type MouseButton = "Left" | "Right" | "Middle" | "Back" | "Forward";

/** 与 airhand_proto::session::InputEvent 对应（serde 外部标记） */
export type InputEvent =
  | { MouseMove: { x: number; y: number } }
  | { MouseButton: { button: MouseButton; down: boolean } }
  | { Wheel: { dx: number; dy: number } }
  | { Key: { code: string; down: boolean } }
  | { Text: { text: string } }
  | "ReleaseAll";

/** 会话的输出 */
export interface SessionSink {
  /** 画面 / 光标 / 声音数据包：24 字节头 + 数据（格式见 crates/airhand-link/src/controller/session.rs） */
  packet(data: Uint8Array): void;
  event(ev: SessionEvent): void;
}

/** 控制台与控制端服务之间的通道 */
export interface Backend {
  /** desktop = 桌面版（Tauri 窗口）；web = 网页版（浏览器） */
  readonly kind: "desktop" | "web";
  appInfo(): Promise<AppInfo>;
  serverStatus(): Promise<ServerState>;
  /** 返回取消订阅的函数 */
  onServerStatus(cb: (s: ServerState) => void): () => void;
  connect(code: string, settings: QualitySettings, sink: SessionSink, clipboard: boolean): Promise<SessionInfo>;
  clipboardSync(sid: number, enabled: boolean): Promise<void>;
  audio(sid: number, enabled: boolean): Promise<void>;
  /** 读取本机剪贴板文本（用于"作为按键输入"） */
  localClipboardText(): Promise<string>;
  input(sid: number, events: InputEvent[]): Promise<void>;
  settings(sid: number, settings: QualitySettings): Promise<void>;
  keyframe(sid: number): Promise<void>;
  selectDisplay(sid: number, index: number): Promise<void>;
  disconnect(sid: number): Promise<void>;
  /** 切换到被控端（只有桌面版） */
  switchToHost(): Promise<void>;
  /** 切换界面语言（桌面版同时作用于托盘菜单） */
  setLanguage(lang: Lang): Promise<void>;
  /** 语言被切换（包括在托盘菜单里切换） */
  onLangChanged(cb: (lang: Lang) => void): () => void;
  /** 让窗口全屏（桌面版；网页版只有网页全屏，什么也不做） */
  setWindowFullscreen(on: boolean): Promise<void>;
  /** 窗口退出了全屏（例如按了系统快捷键） */
  onWindowFullscreenExit(cb: () => void): () => void;
}

export const api: Backend = isTauri() ? tauriBackend() : webBackend();

/** Rust 命令失败时返回的是 Problem；其它异常（例如前端自己抛出的）包装成 failed */
export function toProblem(e: unknown): Problem {
  if (typeof e === "object" && e !== null && typeof (e as { kind?: unknown }).kind === "string") return e as Problem;
  return { kind: "failed", detail: e instanceof Error ? e.message : String(e) };
}
