// 与 Rust 端的命令 / 数据结构一一对应
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
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
 * 显示给用户的问题，与 Rust 端 problem::Problem 一一对应（JSON 以 kind 区分）。
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
  | { kind: "failed"; detail: string };

/** 被控端发来的提示，与 Rust 端 controller::session::NoticeView 对应 */
export type RemoteNotice = { kind: "noDisplays" } | { kind: "wayland" } | { kind: "captureFailed"; detail: string };

export interface AppInfo {
  version: string;
  server: string;
  pinned: boolean;
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

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  serverStatus: () => invoke<ServerState>("server_status"),
  connect: (
    code: string,
    settings: QualitySettings,
    video: Channel<ArrayBuffer>,
    events: Channel<SessionEvent>,
    clipboard: boolean,
  ) => invoke<SessionInfo>("ctrl_connect", { code, settings, video, events, clipboard }),
  clipboardSync: (sid: number, enabled: boolean) => invoke<void>("ctrl_clipboard", { sid, enabled }),
  audio: (sid: number, enabled: boolean) => invoke<void>("ctrl_audio", { sid, enabled }),
  localClipboardText: () => invoke<string>("clipboard_text"),
  input: (sid: number, events: InputEvent[]) => invoke<void>("ctrl_input", { sid, events }),
  settings: (sid: number, settings: QualitySettings) => invoke<void>("ctrl_settings", { sid, settings }),
  keyframe: (sid: number) => invoke<void>("ctrl_keyframe", { sid }),
  selectDisplay: (sid: number, index: number) => invoke<void>("ctrl_display", { sid, index }),
  disconnect: (sid: number) => invoke<void>("ctrl_disconnect", { sid }),
  switchToHost: () => invoke<void>("switch_to_host"),
  /** 切换界面语言（同时作用于托盘菜单） */
  setLanguage: (lang: Lang) => invoke<void>("set_language", { lang }),
  onServerStatus: (cb: (s: ServerState) => void): Promise<UnlistenFn> =>
    listen<ServerState>("server-status", (e) => cb(e.payload)),
  /** 语言被切换（包括在托盘菜单里切换） */
  onLangChanged: (cb: (lang: Lang) => void): Promise<UnlistenFn> => listen<Lang>("lang-changed", (e) => cb(e.payload)),
};

/** Rust 命令失败时返回的是 Problem；其它异常（例如前端自己抛出的）包装成 failed */
export function toProblem(e: unknown): Problem {
  if (typeof e === "object" && e !== null && typeof (e as { kind?: unknown }).kind === "string") return e as Problem;
  return { kind: "failed", detail: e instanceof Error ? e.message : String(e) };
}
