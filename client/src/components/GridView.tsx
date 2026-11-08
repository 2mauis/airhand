import { useT } from "../lib/i18n";
import type { RemoteSession } from "../lib/session";
import { store, useSession, useStore } from "../lib/store";
import { IconClose, IconPlus } from "./Icons";
import { CanvasHost } from "./SessionView";

/** 多屏总览：所有会话的实时画面，点击进入控制 */
export function GridView() {
  const t = useT();
  const sessions = useStore((s) => s.sessions);
  const n = sessions.length;
  const total = n + 1;
  const cols = total <= 1 ? 1 : total <= 4 ? 2 : total <= 9 ? 3 : 4;
  return (
    <div className="grid-view">
      <div className="grid-head">
        <h2>{t("sidebar.grid")}</h2>
        <span className="muted">{t("grid.summary", { n })}</span>
      </div>
      <div className="grid" style={{ gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))` }}>
        {sessions.map((s) => (
          <Tile key={s.id} s={s} />
        ))}
        <button className="tile add" onClick={() => store.setView("new")}>
          <IconPlus size={26} />
          <span>{t("grid.add")}</span>
        </button>
      </div>
    </div>
  );
}

function Tile({ s }: { s: RemoteSession }) {
  const t = useT();
  useSession(s);
  return (
    <div className="tile" onClick={() => store.setView(s.id)}>
      <div className="tile-screen">
        <CanvasHost s={s} interactive={false} />
        {s.state !== "live" && (
          <div className="tile-overlay">{s.state === "closed" ? (
            t("common.disconnected")
          ) : s.state === "waiting" ? (
            t("common.waiting")
          ) : s.state === "approving" ? (
            t("common.approving")
          ) : (
            <span className="spinner" />
          )}</div>
        )}
      </div>
      <div className="tile-bar">
        <span className={`dot ${s.state}`} />
        <span className="tile-name">{s.hostName || s.code}</span>
        <span className="muted small">
          {s.state === "live" ? `${Math.round(s.stats.fps)} fps · ${s.stats.rttMs != null ? Math.round(s.stats.rttMs) : "—"} ms` : ""}
        </span>
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
    </div>
  );
}
