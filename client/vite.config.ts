import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 同一份构建既给桌面版（Tauri）用，也给网页版（airhand-web 提供静态文件）用。
export default defineConfig({
  plugins: [react()],
  // 相对路径：网页版放在反向代理的子路径下（例如 https://example.com/airhand/）也能用
  base: "./",
  clearScreen: false,
  server: {
    // Tauri 期望固定端口
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
    // 在浏览器里打开开发服务器 = 网页版：WebSocket 转给本机运行的 airhand-web（见 README"开发"一节）
    proxy: { "/ws": { target: "http://127.0.0.1:8080", ws: true } },
  },
  build: {
    // WebView2 基于最新 Chromium；网页版也只支持有 WebCodecs 的新浏览器
    target: "es2023",
    assetsInlineLimit: 0,
    chunkSizeWarningLimit: 800,
  },
  worker: { format: "es" },
});
