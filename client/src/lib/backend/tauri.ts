// 桌面版：控制台是 Tauri 窗口，控制端服务在同一个进程里（client/src-tauri/src/lib.rs 的命令）。
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { AppInfo, Backend, ServerState, SessionEvent, SessionInfo, WindowFrame } from "../api";
import type { Lang } from "../i18n";

/** 把异步注册的监听变成同步返回的取消函数 */
function unlistenLater(p: Promise<UnlistenFn>): () => void {
  return () => void p.then((f) => f());
}

function windowFrame(): WindowFrame {
  const w = getCurrentWindow();
  return {
    minimize: () => w.minimize(),
    toggleMaximize: () => w.toggleMaximize(),
    isMaximized: () => w.isMaximized(),
    onResized: (cb) => unlistenLater(w.onResized(() => cb())),
    close: () => w.close(),
    destroy: () => w.destroy(),
    onCloseRequested: (cb) =>
      unlistenLater(
        w.onCloseRequested((e) => {
          if (cb()) return;
          e.preventDefault();
          // 例如在任务栏上关闭最小化的窗口：先把窗口拿出来，用户才看得到询问
          void w.unminimize().then(() => w.setFocus());
        }),
      ),
  };
}

export function tauriBackend(): Backend {
  return {
    frame: windowFrame(),
    kind: "desktop",
    appInfo: () => invoke<AppInfo>("app_info"),
    serverStatus: () => invoke<ServerState>("server_status"),
    onServerStatus: (cb) => unlistenLater(listen<ServerState>("server-status", (e) => cb(e.payload))),
    connect: (code, resume, settings, sink, clipboard) => {
      const video = new Channel<ArrayBuffer>();
      video.onmessage = (buf) => sink.packet(new Uint8Array(buf));
      const events = new Channel<SessionEvent>();
      events.onmessage = (ev) => sink.event(ev);
      return invoke<SessionInfo>("ctrl_connect", { code, resume, settings, video, events, clipboard });
    },
    onHostOnline: (cb) => unlistenLater(listen<number>("host-online", (e) => cb(e.payload))),
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
