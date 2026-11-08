// 桌面版：控制台是 Tauri 窗口，控制端服务在同一个进程里（client/src-tauri/src/lib.rs 的命令）。
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { AppInfo, Backend, SavedDevice, ServerState, SessionEvent, SessionInfo, WindowFrame } from "../api";
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
    connect: async (code, resume, settings, sink, clipboard) => {
      let sid: number | null = null;
      let unacked: number | null = null;
      let closed = false;
      const ack = (seq: number) => {
        if (sid !== null) void invoke("ctrl_ack", { sid, seq }).catch(() => {});
        else unacked = seq;
      };
      const video = new Channel<ArrayBuffer>();
      video.onmessage = (buf) => {
        if (closed) return;
        // ACK only after the WebView receives this packet, including the IPC hop.
        if (buf.byteLength >= 24 && new DataView(buf).getUint8(0) === 1) ack(new DataView(buf).getUint32(4, true));
        sink.packet(new Uint8Array(buf));
      };
      const events = new Channel<SessionEvent>();
      events.onmessage = (ev) => {
        if (ev.type === "closed") closed = true;
        sink.event(ev);
      };
      const info = await invoke<SessionInfo>("ctrl_connect", { code, resume, settings, video, events, clipboard });
      sid = info.sid;
      if (!closed && unacked !== null) ack(unacked);
      return info;
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
    loadDevices: () => invoke<SavedDevice[]>("devices_load"),
    saveDevices: (devices) => invoke("devices_save", { devices }),
    setLanguage: (lang) => invoke("set_language", { lang }),
    onLangChanged: (cb) => unlistenLater(listen<Lang>("lang-changed", (e) => cb(e.payload))),
    // 不直接用 setFullscreen：无边框窗口全屏时还要停用拖动边缘改大小（见 src-tauri/src/fullscreen.rs）
    setWindowFullscreen: (on) => invoke("set_fullscreen", { on }),
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
