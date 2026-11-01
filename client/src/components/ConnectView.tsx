import { useEffect, useRef, useState } from "react";
import { toProblem, type Problem } from "../lib/api";
import { useT } from "../lib/i18n";
import { problemText } from "../lib/messages";
import { CODEC_LABEL, DEFAULT_PRESET, PRESETS, preferredCodec, presetDesc, presetName, presetSettings } from "../lib/presets";
import { RemoteSession } from "../lib/session";
import { store, useStore } from "../lib/store";
import { IconBolt, IconMonitor } from "./Icons";

export function ConnectView() {
  const t = useT();
  const [digits, setDigits] = useState<string[]>(Array(6).fill(""));
  const [preset, setPreset] = useState(DEFAULT_PRESET);
  const [busy, setBusy] = useState(false);
  // 保存类型而不是文字：切换语言后错误提示也跟着变
  const [error, setError] = useState<Problem | null>(null);
  const inputs = useRef<(HTMLInputElement | null)[]>([]);
  const server = useStore((s) => s.server);
  const decoders = useStore((s) => s.decoders);
  const sessions = useStore((s) => s.sessions);
  const codec = preferredCodec(decoders);
  const code = digits.join("");
  const ready = /^\d{6}$/.test(code) && code[0] !== "0";

  useEffect(() => {
    inputs.current[0]?.focus();
  }, []);

  const setFrom = (start: number, text: string) => {
    const only = text.replace(/\D/g, "");
    if (!only) return;
    const next = [...digits];
    let i = start;
    for (const ch of only) {
      if (i > 5) break;
      next[i++] = ch;
    }
    setDigits(next);
    setError(null);
    inputs.current[Math.min(i, 5)]?.focus();
  };

  const connect = async () => {
    if (!ready || busy) return;
    const dup = sessions.find((s) => s.code === code && s.state !== "closed");
    if (dup) {
      store.setView(dup.id);
      return;
    }
    setBusy(true);
    setError(null);
    const s = new RemoteSession(code, presetSettings(preset, codec));
    try {
      await s.start();
      store.add(s);
      setDigits(Array(6).fill(""));
    } catch (e) {
      s.dispose();
      setError(toProblem(e));
      inputs.current[5]?.focus();
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="connect-view">
      <div className="connect-card">
        <div className="hero-icon">
          <IconMonitor size={30} />
        </div>
        <h1>{t("connect.title")}</h1>
        <p className="muted">{t("connect.hint")}</p>

        <div className="code-inputs" onPaste={(e) => (e.preventDefault(), setFrom(0, e.clipboardData.getData("text")))}>
          {digits.map((d, i) => (
            <input
              key={i}
              ref={(el) => {
                inputs.current[i] = el;
              }}
              className={`code-box ${d ? "filled" : ""} ${i === 3 ? "gap" : ""}`}
              value={d}
              inputMode="numeric"
              maxLength={1}
              disabled={busy}
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
                  setDigits(next);
                } else if (e.key === "ArrowLeft" && i > 0) inputs.current[i - 1]?.focus();
                else if (e.key === "ArrowRight" && i < 5) inputs.current[i + 1]?.focus();
                else if (e.key === "Enter") void connect();
              }}
            />
          ))}
        </div>

        <div className="preset-row">
          {PRESETS.map((p) => (
            <button
              key={p.id}
              className={`chip ${preset === p.id ? "active" : ""}`}
              title={presetDesc(p.id)}
              onClick={() => setPreset(p.id)}
              disabled={busy}
            >
              {presetName(p.id)}
            </button>
          ))}
        </div>
        <div className="preset-desc muted small">
          {presetDesc(preset)} · {t("connect.prefer", { codec: CODEC_LABEL[codec] })}
        </div>

        {error && <div className="error-box">{problemText(error)}</div>}

        <button className="primary-btn" disabled={!ready || busy || server.state !== "connected"} onClick={connect}>
          {busy ? <span className="spinner" /> : <IconBolt size={18} />}
          <span>
            {busy ? t("connect.busy") : server.state !== "connected" ? t("connect.waitServer") : t("connect.go")}
          </span>
        </button>

        <div className="features">
          <div>
            <b>QUIC</b>
            <span>{t("connect.feature.quic")}</span>
          </div>
          <div>
            <b>{t("connect.feature.hw")}</b>
            <span>H.264 / HEVC / AV1</span>
          </div>
          <div>
            <b>{t("connect.feature.multi")}</b>
            <span>{t("connect.feature.multiDesc")}</span>
          </div>
        </div>
      </div>
    </div>
  );
}
