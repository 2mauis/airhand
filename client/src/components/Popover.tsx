import { useEffect, useRef, type ReactNode } from "react";

/**
 * 弹出层（工具栏菜单、画质面板、主菜单）：点击外面或按 Esc 时关闭。
 *
 * 放在 `.menu-anchor` 里，和打开它的按钮在一起：点那个按钮不算"点击外面"，
 * 由按钮自己切换开 / 关（否则这里先关掉，按钮的 click 又把它打开）。
 */
export function Popover({ className, onClose, children }: { className: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: PointerEvent) => {
      const anchor = ref.current?.parentElement;
      if (anchor && !anchor.contains(e.target as Node)) onClose();
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // 推迟到下一轮再开始监听：打开弹出层的那次点击不算"点击外面"
    const timer = window.setTimeout(() => window.addEventListener("pointerdown", close), 0);
    window.addEventListener("keydown", esc);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", esc);
    };
  }, [onClose]);
  return (
    // 标签栏整个是窗口的拖动区域：弹出层里的空白处不能拖动窗口
    <div className={`popover ${className}`} ref={ref} data-tauri-drag-region="false">
      {children}
    </div>
  );
}
