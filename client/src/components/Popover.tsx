import { useEffect, useRef, type ReactNode } from "react";

/** 弹出层（工具栏菜单、画质面板）：点击外面时关闭 */
export function Popover({ className, onClose, children }: { className: string; onClose: () => void; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const close = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    // 推迟到下一轮再开始监听：打开弹出层的那次点击不算"点击外面"
    const timer = window.setTimeout(() => window.addEventListener("pointerdown", close), 0);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("pointerdown", close);
    };
  }, [onClose]);
  return (
    <div className={`popover ${className}`} ref={ref}>
      {children}
    </div>
  );
}
