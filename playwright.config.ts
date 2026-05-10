import { defineConfig, devices } from "@playwright/test";

// Playwright runs the Vite dev server (the same one tauri-dev uses) and
// drives the React UI in headless Chromium. Tauri's native `invoke` channel
// is replaced with an in-page stub injected via `addInitScript` so tests can
// simulate ASR / LLM / DB calls without a real Rust backend.
//
// What this catches:
//   - React state-machine regressions (closure capture, ref staleness)
//   - DOM rendering of paste-status, error banners, dictionary list, etc.
//   - Event-listener wiring (focus:external → synth_paste, hotkey:* events)
//
// What this CANNOT catch (run manual checklist in docs/E2E_REGRESSION.md):
//   - Real Whisper / Ollama / SQLCipher / NSWorkspace / CGEventTap behaviour
//   - macOS permission prompts, fn-key long-press, AX context extraction
//   - enigo main-thread crash (covered by Rust unit tests + cargo build)

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: "list",
  use: {
    baseURL: "http://localhost:1422",
    trace: "on-first-retry",
  },
  projects: [
    { name: "chromium", use: { ...devices["Desktop Chrome"] } },
  ],
  // Run on a separate port from `pnpm tauri dev` (1420) so a developer can
  // keep tauri-dev open while running the E2E suite. reuseExistingServer is
  // off because the dev port may not have VITE_E2E=1 (which gates the
  // Tauri-API alias substitution).
  webServer: {
    command: "VITE_E2E=1 pnpm dev",
    url: "http://localhost:1422",
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
