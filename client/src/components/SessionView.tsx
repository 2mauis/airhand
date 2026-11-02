import { type ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { type MsgKey, t, useT } from "../lib/i18n";
import { errorText, noticeText, problemText } from "../lib/messages";
import { CODEC_LABEL } from "../lib/presets";
import type { RemoteSession } from "../lib/session";
import { store, useSession, useStore } from "../lib/store";
import {
  IconChevron,
  IconClipboard,
  IconCpu,
  IconExpand,
  IconFit,
  IconKeyboard,
  IconMonitor,
  IconPower,
  IconRefresh,
  IconShrink,
  IconSliders,
  IconVolume,
  IconVolumeOff,
  IconWarn,
} from "./Icons";
import { Popover } from "./Popover";
import { QualityPanel, fmtKbps } from "./QualityPanel";

/** 把会话的 canvas 挂到当前位置（同一时刻只会挂在一个地方） */
export function CanvasHost({ s, interactive }: { s: RemoteSession; interactive: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current!;
    s.attach(el);
    s.setInteractive(interactive);
    if (interactive) s.focus();
    return () => {
      s.setInteractive(false);
      s.detach();
    };
  }, [s, interactive]);
  return <div ref={ref} className={`canvas-host ${interactive && s.scaleMode === "native" ? "native" : ""}`} />;
}

type Combo = { label: MsgKey; keys: string[] };

const WINDOWS_COMBOS: Combo[] = [
  { label: "combo.winStart", keys: ["MetaLeft"] },
  { label: "combo.altTab", keys: ["AltLeft", "Tab"] },
  { label: "combo.winD", keys: ["MetaLeft", "KeyD"] },
  { label: "combo.winE", keys: ["MetaLeft", "KeyE"] },
  { label: "combo.winR", keys: ["MetaLeft", "KeyR"] },
  { label: "combo.taskManager", keys: ["ControlLeft", "ShiftLeft", "Escape"] },
  { label: "combo.altF4", keys: ["AltLeft", "F4"] },
  { label: "combo.printScreen", keys: ["PrintScreen"] },
];

// Linux 桌面（GNOME / KDE 等）的常用快捷键
const LINUX_COMBOS: Combo[] = [
  { label: "combo.super", keys: ["MetaLeft"] },
  { label: "combo.altTab", keys: ["AltLeft", "Tab"] },
  { label: "combo.superD", keys: ["MetaLeft", "KeyD"] },
  { label: "combo.terminal", keys: ["ControlLeft", "AltLeft", "KeyT"] },
  { label: "combo.altF2", keys: ["AltLeft", "F2"] },
  { label: "combo.superL", keys: ["MetaLeft", "KeyL"] },
  { label: "combo.altF4", keys: ["AltLeft", "F4"] },
  { label: "combo.printScreen", keys: ["PrintScreen"] },
];

// macOS 的常用快捷键（Command 键在对方那边就是 MetaLeft）
const MAC_COMBOS: Combo[] = [
  { label: "combo.cmdTab", keys: ["MetaLeft", "Tab"] },
  { label: "combo.spotlight", keys: ["MetaLeft", "Space"] },
  { label: "combo.missionControl", keys: ["ControlLeft", "ArrowUp"] },
  { label: "combo.forceQuit", keys: ["MetaLeft", "AltLeft", "Escape"] },
  { label: "combo.cmdQ", keys: ["MetaLeft", "KeyQ"] },
  { label: "combo.lockMac", keys: ["ControlLeft", "MetaLeft", "KeyQ"] },
  { label: "combo.screenshotMac", keys: ["MetaLeft", "ShiftLeft", "Digit5"] },
];

/** 被控端操作系统对应的快捷键（Windows 报告 "Windows 11 ..."，macOS 报告 "macOS 15.3"，Linux 报告发行版名称） */
const combosFor = (os: string) =>
  os === "" || /windows/i.test(os) ? WINDOWS_COMBOS : /^macos/i.test(os) ? MAC_COMBOS : LINUX_COMBOS;

type Menu = null | "quality" | "keys" | "display" | "clipboard" | "audio";

// Keyboard Lock API：全屏时捕获 Win、Alt+Tab 等系统快捷键并发送给远端
type KeyboardLock = { lock?: (codes?: string[]) => Promise<void>; unlock?: () => void };
const keyboardApi = () => (navigator as Navigator & { keyboard?: KeyboardLock }).keyboard;

export function SessionView({ s }: { s: RemoteSession }) {
  // 订阅语言切换（文案直接用导入的 t）
  useT();
  useSession(s);
  const immersive = useStore((st) => st.immersive);
  const [menu, setMenu] = useState<Menu>(null);
  const [toast, setToast] = useState<string | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const toggleMenu = (m: Menu) => setMenu((cur) => (cur === m ? null : m));

  // 窗口退出全屏（例如按了系统快捷键）时同步状态
  useEffect(
    () =>
      api.onWindowFullscreenExit(() => {
        if (store.get().immersive) store.setImmersive(false);
      }),
    [],
  );

  useEffect(() => {
    if (!toast) return;
    const t = window.setTimeout(() => setToast(null), 2400);
    return () => clearTimeout(t);
  }, [toast]);

  const toggleFullscreen = async () => {
    const next = !immersive;
    store.setImmersive(next);
    if (next) {
      // 先进入网页全屏（Keyboard Lock 的前提），再让窗口全屏
      await document.documentElement.requestFullscreen?.().catch(() => {});
      await keyboardApi()?.lock?.().catch(() => {});
    } else {
      keyboardApi()?.unlock?.();
      if (document.fullscreenElement) await document.exitFullscreen().catch(() => {});
    }
    await api.setWindowFullscreen(next).catch(() => {});
    s.focus();
  };

  // 网页全屏被系统退出（例如长按 Esc）时同步窗口状态
  useEffect(() => {
    const onChange = () => {
      if (!document.fullscreenElement && store.get().immersive) {
        keyboardApi()?.unlock?.();
        store.setImmersive(false);
        void api.setWindowFullscreen(false).catch(() => {});
      }
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => document.removeEventListener("fullscreenchange", onChange);
  }, []);

  const typeClipboard = async () => {
    try {
      const text = await api.localClipboardText();
      if (!text) return setToast(t("session.noClipboard"));
      s.sendText(text);
      setToast(t("session.typed", { n: text.length }));
    } catch (e) {
      setToast(errorText(e));
    }
    s.focus();
  };

  const st = s.stats;
  const live = s.state === "live";
  const rtt = st.rttMs != null ? Math.round(st.rttMs) : null;
  const rttClass = rtt == null ? "" : rtt < 60 ? "good" : rtt < 150 ? "ok" : "bad";

  return (
    <div className={`session-view ${immersive ? "immersive" : ""}`}>
      {immersive && <div className="hot-edge" />}
      <header className="toolbar">
        <div className="tb-left">
          <div className="tb-title">
            <span className={`dot ${s.state}`} />
            <span className="host">{s.hostName || s.code}</span>
            <span className="code">{s.code}</span>
          </div>
          {s.encoder && (
            <span className={`tag ${s.encoder.hardware ? "hw" : ""}`} title={s.encoder.name}>
              <IconCpu size={13} />
              <span className="tag-text">
                {CODEC_LABEL[s.encoder.codec]} · {s.encoder.hardware ? t("session.hwEncoding") : t("session.swEncoding")}
              </span>
            </span>
          )}
        </div>

        <div className="tb-stats">
          {live && (
            <>
              <span>{Math.round(st.fps)} fps</span>
              <span>{fmtKbps(st.kbps)}</span>
              <span className={`rtt ${rttClass}`}>{rtt != null ? `${rtt} ms` : "— ms"}</span>
              {s.syncActive && s.syncStats.videoDelayMs >= 5 && (
                <span className="muted" title={t("session.syncTitle")}>
                  {t("session.sync", { ms: Math.round(s.syncStats.videoDelayMs) })}
                </span>
              )}
              {st.width > 0 && (
                <span className="muted">
                  {st.width}×{st.height}
                </span>
              )}
            </>
          )}
        </div>

        <div className="tb-right">
          {s.displays.length > 1 && (
            <div className="menu-anchor">
              <button className="tb-btn" onClick={() => toggleMenu("display")} title={t("session.switchDisplay")}>
                <IconMonitor size={17} />
                <span>{t("session.display", { n: s.currentDisplay + 1 })}</span>
                <IconChevron size={14} />
              </button>
              {menu === "display" && (
                <Dropdown onClose={closeMenu}>
                  {s.displays.map((d) => (
                    <button
                      key={d.index}
                      className={`dd-item ${d.index === s.currentDisplay ? "active" : ""}`}
                      onClick={() => {
                        s.selectDisplay(d.index);
                        closeMenu();
                        s.focus();
                      }}
                    >
                      <span>
                        {t("session.display", { n: d.index + 1 })}
                        {d.primary ? t("session.primary") : ""}
                      </span>
                      <span className="muted small">
                        {d.width}×{d.height}
                      </span>
                    </button>
                  ))}
                </Dropdown>
              )}
            </div>
          )}

          <div className="menu-anchor">
            <button className={`tb-btn ${menu === "quality" ? "on" : ""}`} onClick={() => toggleMenu("quality")}>
              <IconSliders size={17} />
              <span>{t("session.quality")}</span>
            </button>
            {menu === "quality" && <QualityPanel s={s} onClose={closeMenu} />}
          </div>

          <div className="menu-anchor">
            <button className={`tb-btn icon ${menu === "keys" ? "on" : ""}`} title={t("session.shortcuts")} onClick={() => toggleMenu("keys")}>
              <IconKeyboard size={17} />
            </button>
            {menu === "keys" && (
              <Dropdown onClose={closeMenu}>
                {combosFor(s.hostOs).map((c) => (
                  <button
                    key={c.label}
                    className="dd-item"
                    onClick={() => {
                      s.sendCombo(c.keys);
                      closeMenu();
                      s.focus();
                    }}
                  >
                    {t(c.label)}
                  </button>
                ))}
              </Dropdown>
            )}
          </div>

          <div className="menu-anchor">
            <button
              className={`tb-btn icon ${menu === "clipboard" ? "on" : ""}`}
              title={t("session.clipboard")}
              onClick={() => toggleMenu("clipboard")}
            >
              <IconClipboard size={17} />
              {s.clipboardSync && <span className="tb-dot" />}
            </button>
            {menu === "clipboard" && (
              <Dropdown onClose={closeMenu}>
                <label className="dd-item toggle">
                  <span>
                    {t("session.clipSync")}
                    <span className="muted small block">
                      {t(api.kind === "web" ? "session.clipSyncHintWeb" : "session.clipSyncHint")}
                    </span>
                  </span>
                  <input
                    type="checkbox"
                    className="switch"
                    checked={s.clipboardSync}
                    onChange={(e) => s.setClipboardSync(e.target.checked)}
                  />
                </label>
                <button
                  className="dd-item"
                  onClick={() => {
                    closeMenu();
                    void typeClipboard();
                  }}
                >
                  <span>
                    {t("session.clipType")}
                    <span className="muted small block">{t("session.clipTypeHint")}</span>
                  </span>
                </button>
              </Dropdown>
            )}
          </div>
          <div className="menu-anchor">
            <button
              className={`tb-btn icon ${menu === "audio" ? "on" : ""}`}
              title={s.audioEnabled ? t("session.sound") : t("session.soundMuted")}
              onClick={() => toggleMenu("audio")}
            >
              {s.audioEnabled ? <IconVolume size={17} /> : <IconVolumeOff size={17} />}
              {s.audioEnabled && s.audioError !== null ? (
                <span className="tb-dot warn" />
              ) : (
                s.audioEnabled && s.avSync && <span className="tb-dot" />
              )}
            </button>
            {menu === "audio" && (
              <Dropdown onClose={closeMenu} className="audio-menu">
                <label className="dd-item toggle">
                  <span>
                    {t("session.playSound")}
                    <span className="muted small block">{audioHint(s)}</span>
                  </span>
                  <input
                    type="checkbox"
                    className="switch"
                    checked={s.audioEnabled}
                    onChange={(e) => s.setAudioEnabled(e.target.checked)}
                  />
                </label>
                <label className={`dd-item toggle ${s.audioEnabled ? "" : "disabled"}`}>
                  <span>
                    {t("session.avSync")}
                    <span className="muted small block">{t("session.avSyncHint")}</span>
                    <span className="muted small block">{syncHint(s)}</span>
                  </span>
                  <input
                    type="checkbox"
                    className="switch"
                    checked={s.avSync}
                    disabled={!s.audioEnabled}
                    onChange={(e) => s.setAvSync(e.target.checked)}
                  />
                </label>
              </Dropdown>
            )}
          </div>
          <button
            className={`tb-btn icon ${s.scaleMode === "native" ? "on" : ""}`}
            title={s.scaleMode === "fit" ? t("session.toNative") : t("session.toFit")}
            onClick={() => {
              s.setScaleMode(s.scaleMode === "fit" ? "native" : "fit");
              s.focus();
            }}
          >
            <IconFit size={17} />
          </button>
          <button
            className="tb-btn icon"
            title={immersive ? t("session.exitFullscreen") : t("session.fullscreen")}
            onClick={toggleFullscreen}
          >
            {immersive ? <IconShrink size={17} /> : <IconExpand size={17} />}
          </button>
          <button className="tb-btn icon danger" title={t("session.disconnect")} onClick={() => store.remove(s.id)}>
            <IconPower size={17} />
          </button>
        </div>
      </header>

      <div className="stage">
        <CanvasHost s={s} interactive={s.state === "live"} />

        {s.state !== "live" && (
          <div className="stage-overlay">
            {s.state === "closed" ? (
              <div className="overlay-card">
                <IconWarn size={28} />
                <div className="overlay-title">{s.retries > 0 ? t("session.reconnectFailed") : t("session.lost")}</div>
                <div className="muted">{s.closedReason ? problemText(s.closedReason) : t("session.ended")}</div>
                <div className="overlay-actions">
                  <button className="primary-btn small" onClick={() => s.reconnect()}>
                    <IconRefresh size={16} />
                    {t("session.reconnect")}
                  </button>
                  <button className="ghost-btn" onClick={() => store.remove(s.id)}>
                    {t("session.close")}
                  </button>
                </div>
              </div>
            ) : s.state === "waiting" ? (
              <div className="overlay-card">
                <span className="spinner large" />
                <div className="overlay-title">{t("session.waiting")}</div>
                <div className="muted small">{t("session.waitingHint")}</div>
                <div className="overlay-actions">
                  <button className="ghost-btn" onClick={() => store.remove(s.id)}>
                    {t("session.close")}
                  </button>
                </div>
              </div>
            ) : (
              <div className="overlay-card">
                <span className="spinner large" />
                <div className="overlay-title">
                  {s.state === "reconnecting" ? t("session.reconnecting") : t("session.connecting")}
                </div>
                {s.closedReason && <div className="muted small">{problemText(s.closedReason)}</div>}
                {s.retries > 0 && <div className="muted small">{t("session.attempt", { n: s.retries })}</div>}
              </div>
            )}
          </div>
        )}

        {s.notice && (
          <div className="notice" onClick={() => s.dismissNotice()}>
            <IconWarn size={16} />
            <span>{noticeText(s.notice)}</span>
          </div>
        )}
        {toast && <div className="toast">{toast}</div>}
      </div>
    </div>
  );
}

function audioHint(s: RemoteSession): string {
  if (!s.audioEnabled) return t("audio.muted");
  if (s.audioError !== null) return t("notice.audioFailed", { detail: s.audioError });
  const st = s.audio.stats;
  return st ? t("audio.jitter", { ms: Math.round(st.targetMs) }) : t("audio.remote");
}

function syncHint(s: RemoteSession): string {
  if (!s.audioEnabled) return t("sync.needSound");
  const { skewMs, videoDelayMs, extraMs } = s.syncStats;
  if (skewMs === null) return t("sync.waiting");
  const skew = Math.round(skewMs);
  if (!s.avSync) return skew >= 0 ? t("sync.audioLate", { ms: skew }) : t("sync.videoLate", { ms: -skew });
  const before = skew >= 0 ? t("sync.wasAudioLate", { ms: skew }) : t("sync.wasVideoLate", { ms: -skew });
  if (extraMs > 0) return t("sync.audioDelayed", { ms: Math.round(extraMs), before });
  return t("sync.videoDelayed", { ms: Math.round(videoDelayMs), before });
}

function Dropdown({ children, onClose, className = "" }: { children: ReactNode; onClose: () => void; className?: string }) {
  return (
    <Popover className={`dropdown ${className}`} onClose={onClose}>
      {children}
    </Popover>
  );
}
