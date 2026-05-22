import { test, expect } from "@playwright/test";

// Each test starts on a fresh page with the Tauri stub already loaded
// (vite alias replaces @tauri-apps/api/* in dev mode when VITE_E2E=1).
//
// All the regressions we hit during Phase A / B1 are encoded here so any
// future refactor is forced to keep them green.

async function gotoMainUI(page: import("@playwright/test").Page) {
  await page.goto("/");
  await expect(page.getByText(/Auto-paste/)).toBeVisible();
}

test.describe("regression suite", () => {
  test("R1: setup → main UI renders directly", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText(/Auto-paste/)).toBeVisible();
  });

  test("R2: prompt template loads and ja_keigo is the default", async ({ page }) => {
    await gotoMainUI(page);
    // Task select shows the loaded label, not "(loading...)".
    await expect(page.locator("[data-task-select]")).toContainText("日本語(敬体)");
  });

  test("R3: Record → Stop → automatic Rewrite → Output populated (B-2 regression)", async ({
    page,
  }) => {
    // This is the regression that surfaced as "no prompt template selected"
    // — runRewrite captured a stale empty `prompts` via React closure.
    await gotoMainUI(page);
    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();

    const output = page.getByPlaceholder("(empty)");
    await expect(output).toHaveValue(/本橋伸一/);
    // No error banner.
    await expect(page.locator(".error")).toHaveCount(0);
  });

  test("R4: arm-and-paste shows hint when Dictation has focus", async ({ page }) => {
    await gotoMainUI(page);
    // externalFocused defaults to false → arm waits for focus:external.
    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();
    await expect(page.getByText(/クリップボードに保存/)).toBeVisible();
  });

  test("R5: focus:external event fires synth_paste and shows success notice", async ({
    page,
  }) => {
    await gotoMainUI(page);
    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();
    await expect(page.getByText(/クリップボードに保存/)).toBeVisible();

    // Simulate the user switching to ChatGPT.
    await page.evaluate(() => {
      window.dispatchEvent(
        new CustomEvent("__e2e_tauri_event__", {
          detail: { event: "focus:external", payload: "ChatGPT" },
        }),
      );
    });

    await expect(page.getByText(/ChatGPT に貼付しました/)).toBeVisible();
    const synthCount = await page.evaluate(() => window.__E2E_STATE.synthPasteCount);
    expect(synthCount).toBe(1);
  });

  test("R6: immediate paste fires when external app is already focused", async ({
    page,
  }) => {
    // Pre-arm externalFocused = true (fn-long-press from inside ChatGPT).
    await page.addInitScript(() => {
      window.__E2E_STATE = { ...(window.__E2E_STATE ?? {}), externalFocused: true };
    });
    await gotoMainUI(page);
    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();
    // Should not show the "armed" hint long; success notice shows up instead.
    await expect(page.getByText(/貼付しました/)).toBeVisible();
    const synthCount = await page.evaluate(() => window.__E2E_STATE.synthPasteCount);
    expect(synthCount).toBe(1);
  });

  test("R7: clipboard contains the rewrite result", async ({ page }) => {
    await gotoMainUI(page);
    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();
    await expect(page.getByPlaceholder("(empty)")).toHaveValue(/本橋伸一/);
    const clip = await page.evaluate(() => window.__E2E_STATE.lastClipboard);
    expect(clip).toContain("本橋伸一");
  });

  test("R8: dictionary CRUD via Settings tab", async ({ page }) => {
    await gotoMainUI(page);
    await page.getByRole("button", { name: "Settings" }).click();
    await page.getByRole("button", { name: "+ 追加" }).click();
    await page.getByPlaceholder("本橋").fill("ARI");
    await page.getByRole("button", { name: "保存" }).click();
    await expect(page.getByText("ARI")).toBeVisible();
    const dict = await page.evaluate(() => window.__E2E_STATE.dictionary);
    expect(dict).toHaveLength(1);
    expect(dict[0].term).toBe("ARI");
  });

  test("R9: last-used template persists across reloads (localStorage)", async ({
    page,
    context,
  }) => {
    // First mount: switch to en_business.
    await page.addInitScript(() => {
      window.__E2E_STATE = {
        ...(window.__E2E_STATE ?? {}),
        prompts: [
          {
            id: "builtin_ja_keigo",
            name: "ja_keigo",
            label: "日本語(敬体)",
            body: "{input}",
            language: "ja",
            is_builtin: true,
            order_idx: 0,
            created_at: "",
            updated_at: "",
          },
          {
            id: "builtin_en_business",
            name: "en_business",
            label: "English (business)",
            body: "{input}",
            language: "en",
            is_builtin: true,
            order_idx: 1,
            created_at: "",
            updated_at: "",
          },
        ],
      };
    });

    await gotoMainUI(page);
    // Select by option value (id) — Playwright's label match is exact, and
    // the option label includes a trailing space from JSX whitespace.
    await page.locator("[data-task-select]").selectOption("builtin_en_business");

    // Reload the page; the alias-stub fakes a fresh app boot.
    await page.reload();
    await expect(page.getByText(/Auto-paste/)).toBeVisible();
    await expect(page.locator("[data-task-select]")).toContainText("English");
  });

  test("R10: Whisper-only mode bypasses LLM and pastes the raw ASR", async ({
    page,
  }) => {
    // Seed bypass_llm=true so the post-ASR helper takes the no-LLM path.
    // The asrResult/rewriteResult are deliberately different so we can
    // assert which one ends up in Output / clipboard.
    await page.addInitScript(() => {
      window.__E2E_STATE = {
        ...(window.__E2E_STATE ?? {}),
        appSettings: {
          bypass_llm: true,
          whisper_initial_prompt: "WhisperKit, Tauri, ARI",
        },
        asrResult: "raw whisper transcript",
        rewriteResult: "should not appear",
      };
    });
    await gotoMainUI(page);

    await page.getByRole("button", { name: /^Record$/ }).click();
    await page.getByRole("button", { name: /Stop Recording/ }).click();

    // Output must contain the raw ASR, NOT the LLM rewrite.
    const output = page.getByPlaceholder("(empty)");
    await expect(output).toHaveValue("raw whisper transcript");

    // rewrite_streaming must NOT have been called.
    const log = await page.evaluate(() => window.__E2E_LOG ?? []);
    const calledRewrite = log.some((e) => e.cmd === "rewrite_streaming");
    expect(calledRewrite).toBe(false);

    // Clipboard armed with the raw transcript.
    const clipboard = await page.evaluate(() => window.__E2E_STATE.lastClipboard);
    expect(clipboard).toBe("raw whisper transcript");
  });

  test("R11: Settings → General tab saves bypass_llm + whisper prompt", async ({
    page,
  }) => {
    await gotoMainUI(page);
    await page.getByRole("button", { name: "Settings" }).click();
    // The "一般" sub-tab is the default → form is already visible.
    await page.getByLabel(/LLMをスキップ/).check();
    await page
      .locator('textarea[placeholder*="WhisperKit"]')
      .fill("Tauri, Ollama, WhisperKit");
    await page.getByRole("button", { name: /^保存$/ }).click();

    // Backend mock echoes the saved value back into state.appSettings.
    await expect(page.getByText(/✅ 保存しました/)).toBeVisible();
    const saved = await page.evaluate(() => window.__E2E_STATE.appSettings);
    expect(saved.bypass_llm).toBe(true);
    expect(saved.whisper_initial_prompt).toBe("Tauri, Ollama, WhisperKit");
  });
});
