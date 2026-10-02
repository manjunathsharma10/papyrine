import { expect, type Page } from "@playwright/test";

export const MOD = process.platform === "darwin" ? "Meta" : "Control";

/** Boot the app with a clean profile and optional prefs. */
export async function boot(page: Page, prefs: Record<string, unknown> = {}) {
  await page.addInitScript((p) => {
    try {
      if (Object.keys(p).length) localStorage.setItem("papyrine.prefs.v1", JSON.stringify(p));
    } catch {
      /* ignore */
    }
  }, prefs);
  await page.goto("/");
  await expect(page.getByTestId("welcome")).toBeVisible();
}

/** Open the mock sample document with the keyboard-only path (focus Open, press Enter). */
export async function openSample(page: Page) {
  await page.getByTestId("welcome-open").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("viewer")).toBeVisible();
  await expect(page.locator(".page canvas[data-ready='1']").first()).toBeVisible();
}

export async function currentPage(page: Page): Promise<number> {
  const text = (await page.getByTestId("status-page").textContent()) ?? "";
  return Number(/(\d+)/.exec(text)?.[1] ?? 0);
}

export async function zoomPercent(page: Page): Promise<number> {
  const text = (await page.getByTestId("status-zoom").textContent()) ?? "";
  return parseFloat(text);
}
