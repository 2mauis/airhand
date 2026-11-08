// 关闭控制台 / 切换为被控端之前的确认：这两个操作会断开所有会话。
import { useEffect } from "react";
import { quit } from "../lib/actions";
import { useT } from "../lib/i18n";
import { store, useStore } from "../lib/store";
import { IconPower } from "./Icons";

export function ConfirmDialog() {
  const t = useT();
  const c = useStore((s) => s.confirm);

  useEffect(() => {
    if (!c) return;
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") store.setConfirm(null);
    };
    window.addEventListener("keydown", esc);
    return () => window.removeEventListener("keydown", esc);
  }, [c]);

  if (!c) return null;
  const cancel = () => store.setConfirm(null);
  return (
    <div
      className="modal-backdrop"
      data-tauri-drag-region="false"
      onPointerDown={(e) => e.target === e.currentTarget && cancel()}
    >
      <div className="modal" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title">
        <div className="modal-icon">
          <IconPower size={20} />
        </div>
        <h3 id="confirm-title">{t(`confirm.${c.kind}.title`)}</h3>
        <p>{t(`confirm.${c.kind}.body`, { n: c.sessions })}</p>
        <div className="modal-actions">
          <button className="ghost-btn" onClick={cancel}>
            {t("confirm.cancel")}
          </button>
          {/* 默认焦点在这里：Enter 确认，Esc 取消 */}
          <button className="danger-btn" autoFocus onClick={() => void quit(c.kind)}>
            {t(`confirm.${c.kind}.ok`)}
          </button>
        </div>
      </div>
    </div>
  );
}
