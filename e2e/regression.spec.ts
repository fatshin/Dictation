import { test, expect } from "@playwright/test";

// Each test starts on a fresh page with the Tauri stub already loaded
// (vite alias replaces @tauri-apps/api/* in dev mode when VITE_E2E=1).
//
// All the regressions we hit during Phase A / B1 are encoded here so any
// future refactor is forced to keep them green.

async function gotoMainUI(page: import("@playwright/test").Page) {
  await page.goto("/");
  // Setup screen passes through (mock returns ready=true) → consent screen.
  await page.getByRole("button", { name: /consent/i }).click();
  // Auto-paste only appears on the main UI, and only once → unambiguous.
  await expect(page.getByText(/Auto-paste/)).toBeVisible();
}

test.describe("regression suite", () => {
  test("R1: setup → consent → main UI renders", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByText(/Recording Consent/)).toBeVisible();
    await page.getByRole("button", { name: /consent/i }).click();
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
    await page.getByRole("button", { name: /consent/i }).click();
    await expect(page.locator("[data-task-select]")).toContainText("English");
  });
});
