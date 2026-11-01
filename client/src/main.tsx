import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { store } from "./lib/store";
import "./styles.css";

// 仅开发模式：方便用 DevTools / 自动化脚本查看会话状态（例如测量音画同步）
if (import.meta.env.DEV) Object.assign(window, { __airhand: { store } });

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
