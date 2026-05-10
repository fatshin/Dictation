import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "path";

const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;
const host = env?.TAURI_DEV_HOST;
const isE2E = env?.VITE_E2E === "1";
const mainInput = new URL("./index.html", import.meta.url).pathname;
const overlayInput = new URL("./overlay.html", import.meta.url).pathname;

export default defineConfig(async () => ({
  plugins: [react()],
  resolve: isE2E
    ? {
        alias: {
          // E2E mode: replace Tauri APIs with in-page fakes so Playwright
          // can drive the React UI without a real Rust backend.
          "@tauri-apps/api/core": path.resolve(__dirname, "src/__mocks__/tauri-core.ts"),
          "@tauri-apps/api/event": path.resolve(__dirname, "src/__mocks__/tauri-event.ts"),
        },
      }
    : undefined,
  build: {
    rollupOptions: {
      input: {
        main: mainInput,
        overlay: overlayInput,
      },
    },
  },
  clearScreen: false,
  server: {
    port: isE2E ? 1422 : 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
