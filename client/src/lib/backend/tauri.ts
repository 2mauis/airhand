// 桌面版：控制台是 Tauri 窗口，控制端服务在同一个进程里（client/src-tauri/src/lib.rs 的命令）。
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { AppInfo, Backend, ServerState, SessionEvent, SessionInfo } from "../api";
import type { Lang } from "../i18n";

/** 把异步注册的监听变成同步返回的取消函数 */
function unlistenLater(p: Promise<UnlistenFn>): () => void {
  return () => void p.then((f) => f());
}

export function tauriBackend(): Backend {
  return {
    kind: "desktop",
    appInfo: () => invoke<AppInfo>("app_info"),
    serverStatus: () => invoke<ServerState>("server_status"),
    onServerStatus: (cb) => unlistenLater(listen<ServerState>("server-status", (e) => cb(e.payload))),
    connect: (code, settings, sink, clipboard) => {
      const video = new Channel<ArrayBuffer>();
      video.onmessage = (buf) => sink.packet(new Uint8Array(buf));
      const events = new Channel<SessionEvent>();
      events.onmessage = (ev) => sink.event(ev);
      return invoke<SessionInfo>("ctrl_connect", { code, settings, video, events, clipboard });
    },
    clipboardSync: (sid, enabled) => invoke("ctrl_clipboard", { sid, enabled }),
    audio: (sid, enabled) => invoke("ctrl_audio", { sid, enabled }),
    localClipboardText: () => invoke<string>("clipboard_text"),
    input: (sid, events) => invoke("ctrl_input", { sid, events }),
    settings: (sid, settings) => invoke("ctrl_settings", { sid, settings }),
    keyframe: (sid) => invoke("ctrl_keyframe", { sid }),
    selectDisplay: (sid, index) => invoke("ctrl_display", { sid, index }),
    disconnect: (sid) => invoke("ctrl_disconnect", { sid }),
    switchToHost: () => invoke("switch_to_host"),
    setLanguage: (lang) => invoke("set_language", { lang }),
    onLangChanged: (cb) => unlistenLater(listen<Lang>("lang-changed", (e) => cb(e.payload))),
    setWindowFullscreen: (on) => getCurrentWindow().setFullscreen(on),
    onWindowFullscreenExit: (cb) => {
      const w = getCurrentWindow();
      return unlistenLater(
        w.onResized(async () => {
          if (!(await w.isFullscreen())) cb();
        }),
      );
    },
  };
}
