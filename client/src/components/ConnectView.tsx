// 新建连接标签：输入控制码连接；下面是本次关闭过的连接，点一下重新连接。
//
// 输入的内容放在 store 里（Draft），切到别的标签再回来时还在。
import { useEffect, useRef } from "react";
import { fmtCode, openSession, resetDraft } from "../lib/actions";
import { useT } from "../lib/i18n";
import { problemText } from "../lib/messages";
import { bestCodec } from "../lib/codec";
import { CODEC_LABEL, PRESETS, matchPreset, presetDesc, presetName, presetSettings } from "../lib/presets";
import { store, useStore, type Recent } from "../lib/store";
import { IconArrowRight, IconBolt, IconClose, IconHistory, IconMonitor, IconWarn } from "./Icons";

export function ConnectView() {
  const t = useT();
  const draft = useStore((s) => s.draft);
  const server = useStore((s) => s.server);
  const decoders = useStore((s) => s.decoders);
  const recent = useStore((s) => s.recent);
  const sessions = useStore((s) => s.sessions);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);

  // 新建连接标签总是有的时候才显示这一页；万一没有（例如状态不一致），补一个
  useEffect(() => {
    if (!draft) store.newTab();
  }, [draft]);

  const busy = draft?.busy ?? false;
  const error = draft?.error ?? null;

  // 打开这一页时光标放在第一个空格里；连接失败后放在最后一格，方便改
  useEffect(() => {
    const digits = store.get().draft?.digits ?? [];
    const i = digits.findIndex((d) => !d);
    inputs.current[i < 0 ? 5 : i]?.focus();
  }, []);
  useEffect(() => {
    if (!busy && error) inputs.current[5]?.focus();
  }, [busy, error]);

  if (!draft) return null;
  const { digits, preset } = draft;
  // 还不知道对方支持什么：先按本机能解码的选，连上后会按双方都支持的重新选（见 RemoteSession.fitCodec）
  const codec = bestCodec(decoders);
  const code = digits.join("");
  const ready = /^\d{6}$/.test(code) && code[0] !== "0";
  const online = server.state === "connected";
  const open = new Set(sessions.map((s) => s.code));
  const recents = recent.filter((r) => !open.has(r.code));

  const setFrom = (start: number, text: string) => {
    const only = text.replace(/\D/g, "");
    if (!only) return;
    const next = [...digits];
    let i = start;
    for (const ch of only) {
      if (i > 5) break;
      next[i++] = ch;
    }
    store.patchDraft({ digits: next, error: null });
    inputs.current[Math.min(i, 5)]?.focus();
  };

  const connect = () => {
    if (!ready || busy || !online) return;
    void openSession(code, presetSettings(preset, codec));
  };

  const reconnect = (r: Recent) => {
    if (busy || !online) return;
    void openSession(r.code, r.settings);
  };

  return (
    <div className="newtab">
      <div className="nt-wrap">
        <section className="nt-card">
          <div className="nt-head">
            <div className="hero-icon">
              <IconMonitor size={26} />
            </div>
            <h1>{t("connect.title")}</h1>
            <p className="muted">{t("connect.hint")}</p>
          </div>

          <div className="nt-field">
            <div className="nt-label">
              <span>{t("connect.code")}</span>
              {code.length > 0 && !busy && (
                <button className="text-btn" onClick={() => (resetDraft(), inputs.current[0]?.focus())}>
                  {t("connect.clear")}
                </button>
              )}
            </div>
            <div
              className={`code-inputs ${error ? "invalid" : ""}`}
              onPaste={(e) => (e.preventDefault(), setFrom(0, e.clipboardData.getData("text")))}
            >
              {digits.map((d, i) => (
                <input
                  key={i}
                  ref={(el) => {
                    inputs.current[i] = el;
                  }}
                  className={`code-box ${d ? "filled" : ""} ${i === 3 ? "gap" : ""}`}
                  value={d}
                  inputMode="numeric"
                  autoComplete="off"
                  maxLength={1}
                  disabled={busy}
                  aria-label={`${t("connect.code")} ${i + 1}`}
                  onFocus={(e) => e.target.select()}
                  onChange={(e) => {
                    const v = e.target.value;
                    if (v.length > 1) return setFrom(i, v);
                    if (/^\d$/.test(v)) setFrom(i, v);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Backspace") {
                      e.preventDefault();
                      const next = [...digits];
                      if (next[i]) next[i] = "";
                      else if (i > 0) {
                        next[i - 1] = "";
                        inputs.current[i - 1]?.focus();
                      }
                      store.patchDraft({ digits: next, error: null });
                    } else if (e.key === "Delete") {
                      e.preventDefault();
                      const next = [...digits];
                      next[i] = "";
                      store.patchDraft({ digits: next, error: null });
                    } else if (e.key === "ArrowLeft" && i > 0) inputs.current[i - 1]?.focus();
                    else if (e.key === "ArrowRight" && i < 5) inputs.current[i + 1]?.focus();
                    else if (e.key === "Home") inputs.current[0]?.focus();
                    else if (e.key === "End") inputs.current[5]?.focus();
                    else if (e.key === "Enter") connect();
                  }}
                />
              ))}
            </div>
          </div>

          <div className="nt-field">
            <div className="nt-label">
              <span>{t("connect.quality")}</span>
            </div>
            <div className="segmented presets">
              {PRESETS.map((p) => (
                <button
                  key={p.id}
                  className={preset === p.id ? "active" : ""}
                  title={presetDesc(p.id)}
                  onClick={() => store.patchDraft({ preset: p.id })}
                  disabled={busy}
                >
                  {presetName(p.id)}
                </button>
              ))}
            </div>
            <div className="preset-desc muted small">
              {presetDesc(preset)} · {t("connect.prefer", { codec: CODEC_LABEL[codec] })}
            </div>
          </div>

          {error && (
            <div className="error-box" role="alert">
              <IconWarn size={15} />
              <span>{problemText(error)}</span>
            </div>
          )}
          {server.state === "disconnected" && (
            <div className="error-box soft">
              <span className="dot closed" />
              <span>
                {t("server.disconnected")} · {problemText(server.reason)}
              </span>
            </div>
          )}

          <button className="primary-btn" disabled={!ready || busy || !online} onClick={connect}>
            {busy ? <span className="spinner" /> : <IconBolt size={18} />}
            <span>{busy ? t("connect.busy") : !online ? t("connect.waitServer") : t("connect.go")}</span>
          </button>
        </section>

        {recents.length > 0 && (
          <section className="nt-recent">
            <div className="section-title">
              <span>
                <IconHistory size={14} />
                {t("connect.recent")}
              </span>
              <span className="muted small">{t("connect.recentHint")}</span>
            </div>
            <div className="recent-list">
              {recents.map((r) => {
                const p = matchPreset(r.settings);
                return (
                  <div
                    key={r.code}
                    role="button"
                    tabIndex={0}
                    className={`recent-item ${busy || !online ? "disabled" : ""}`}
                    onClick={() => reconnect(r)}
                    onKeyDown={(e) => e.key === "Enter" && reconnect(r)}
                  >
                    <span className="avatar">{(r.name || r.code).slice(0, 1).toUpperCase()}</span>
                    <span className="recent-meta">
                      <span className="recent-name">{r.name || fmtCode(r.code)}</span>
                      <span className="muted small">
                        {fmtCode(r.code)} · {p ? presetName(p) : t("connect.customQuality")}
                      </span>
                    </span>
                    <IconArrowRight size={16} className="recent-go" />
                    <button
                      className="icon-btn subtle"
                      title={t("connect.forget")}
                      onClick={(e) => {
                        e.stopPropagation();
                        store.forgetRecent(r.code);
                      }}
                    >
                      <IconClose size={13} />
                    </button>
                  </div>
                );
              })}
            </div>
          </section>
        )}
      </div>
    </div>
  );
}
