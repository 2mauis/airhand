import { api } from "../lib/api";
import { setLang, useLang, useT, type Lang } from "../lib/i18n";
import { problemText } from "../lib/messages";
import type { RemoteSession } from "../lib/session";
import { store, useSession, useStore } from "../lib/store";
import { IconClose, IconGrid, IconPlus, IconSwap } from "./Icons";
import { Logo } from "./Logo";

export function Sidebar() {
  const t = useT();
  const sessions = useStore((s) => s.sessions);
  const view = useStore((s) => s.view);
  const server = useStore((s) => s.server);
  const app = useStore((s) => s.app);
  const web = api.kind === "web";

  const serverLabel =
    server.state === "connected"
      ? t("sidebar.server.connected")
      : server.state === "connecting"
        ? t("sidebar.server.connecting")
        : t("sidebar.server.disconnected");

  return (
    <aside className="sidebar">
      <div className="brand">
        <Logo size={30} />
        <div className="brand-text">
          <div className="brand-name">Airhand</div>
          <div className="brand-sub">{t("sidebar.subtitle")}</div>
        </div>
      </div>

      <div
        className={`server-pill ${server.state}`}
        title={server.state === "disconnected" ? problemText(server.reason) : app?.server}
      >
        <span className="dot" />
        <span>{serverLabel}</span>
        {app && !app.pinned && (
          <span className="badge warn" title={t("sidebar.unpinnedHint")}>
            {t("sidebar.unpinned")}
          </span>
        )}
      </div>

      <nav className="nav">
        <button className={`nav-item ${view === "new" ? "active" : ""}`} onClick={() => store.setView("new")}>
          <IconPlus />
          <span>{t("sidebar.new")}</span>
        </button>
        <button
          className={`nav-item ${view === "grid" ? "active" : ""}`}
          onClick={() => store.setView("grid")}
          disabled={sessions.length === 0}
        >
          <IconGrid />
          <span>{t("sidebar.grid")}</span>
          {sessions.length > 0 && <span className="count">{sessions.length}</span>}
        </button>
      </nav>

      <div className="section-title">
        <span>{t("sidebar.sessions")}</span>
        <span className="muted">{sessions.length}</span>
      </div>
      <div className="session-list">
        {sessions.length === 0 && <div className="empty-hint">{t("sidebar.empty")}</div>}
        {sessions.map((s) => (
          <SessionItem key={s.id} s={s} active={view === s.id} />
        ))}
      </div>

      <div className="sidebar-footer">
        <div className="me-row">
          <div className="me">
            <div className="me-name" title={web ? t("sidebar.nameHintWeb") : undefined}>
              {app?.hostname || t("common.thisComputer")}
            </div>
            <div className="muted small">
              {t(web ? "sidebar.versionWeb" : "sidebar.version", { version: app?.version ?? "—" })}
            </div>
          </div>
          <LangSwitch />
        </div>
        {/* 网页版没有被控端模式 */}
        {!web && (
          <button className="ghost-btn" title={t("sidebar.toHostHint")} onClick={() => void api.switchToHost()}>
            <IconSwap size={16} />
            <span>{t("sidebar.toHost")}</span>
          </button>
        )}
      </div>
    </aside>
  );
}

/** 每个选项都用它自己的语言书写，看不懂当前语言的人也能找到 */
const LANGS: { id: Lang; label: string }[] = [
  { id: "zh", label: "中文" },
  { id: "en", label: "EN" },
];

export function LangSwitch() {
  const lang = useLang();
  const choose = (l: Lang) => {
    setLang(l);
    // 同步给 Rust 端：托盘菜单、窗口标题、之后产生的提示与错误
    void api.setLanguage(l);
  };
  return (
    <div className="segmented lang-switch" role="group" aria-label="Language / 语言" title="Language / 语言">
      {LANGS.map((l) => (
        <button key={l.id} className={lang === l.id ? "active" : ""} aria-pressed={lang === l.id} onClick={() => choose(l.id)}>
          {l.label}
        </button>
      ))}
    </div>
  );
}

function SessionItem({ s, active }: { s: RemoteSession; active: boolean }) {
  const t = useT();
  useSession(s);
  const status =
    s.state === "live"
      ? `${Math.round(s.stats.fps)} fps · ${s.stats.rttMs != null ? `${Math.round(s.stats.rttMs)} ms` : "—"}`
      : s.state === "reconnecting"
        ? t("common.reconnecting")
        : s.state === "waiting"
          ? t("common.waiting")
          : s.state === "connecting"
          ? t("common.connecting")
          : t("common.disconnected");
  return (
    <div className={`session-item ${active ? "active" : ""} ${s.state}`} onClick={() => store.setView(s.id)}>
      <div className="avatar">{(s.hostName || "?").slice(0, 1).toUpperCase()}</div>
      <div className="session-meta">
        <div className="session-name">{s.hostName || s.code}</div>
        <div className="session-sub">
          <span className={`dot ${s.state}`} />
          {status}
        </div>
      </div>
      <button
        className="icon-btn subtle"
        title={t("common.disconnect")}
        onClick={(e) => {
          e.stopPropagation();
          store.remove(s.id);
        }}
      >
        <IconClose size={14} />
      </button>
    </div>
  );
}
