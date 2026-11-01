import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri 期望固定端口
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // WebView2 基于最新 Chromium
    target: "es2023",
    assetsInlineLimit: 0,
    chunkSizeWarningLimit: 800,
  },
  worker: { format: "es" },
});
