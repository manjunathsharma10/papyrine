import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { MOD, boot, openSample } from "./helpers";

async function audit(page: Page, label: string) {
  const results = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"]).analyze();
  const bad = results.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  const summary = bad.map((v) => `${label}: ${v.id} (${v.impact}) ${v.help}\n  ${v.nodes.slice(0, 3).map((n) => n.target.join(" ") + " :: " + (n.failureSummary ?? "").split("\n").slice(1, 3).join(" | ")).join("\n  ")}`);
  expect(summary, summary.join("\n")).toEqual([]);
}

const THEMES = ["light", "dark", "hc"] as const;

for (const theme of THEMES) {
  test.describe(`axe: ${theme} theme`, () => {
    test("welcome, viewer, panes, panels", async ({ page }) => {
      await boot(page, { theme });
      await audit(page, "welcome");

      await openSample(page);
      await audit(page, "viewer+thumbnails");

      await page.keyboard.press(`${MOD}+Shift+2`);
      await expect(page.getByTestId("bookmarks")).toBeVisible();
      await audit(page, "bookmarks");

      await page.keyboard.press(`${MOD}+f`);
      await expect(page.getByTestId("find-input")).toBeFocused();
      await page.keyboard.type("journal");
      await expect(page.getByTestId("find-hit").first()).toBeVisible();
      await audit(page, "search panel");

      await page.keyboard.press("Escape");
      await page.getByTestId("tool-annotate").click();
      await expect(page.getByRole("heading", { name: "Annotate" })).toBeVisible();
      await audit(page, "annotate panel");

      await page.getByTestId("tool-organize").click();
      await expect(page.getByRole("button", { name: "Rotate page right" })).toBeVisible();
      await page.getByRole("button", { name: "Rotate page right" }).click();
      await audit(page, "organize panel + dirty tab");

      await page.keyboard.press(`${MOD}+Shift+t`);
      await expect(page.getByTestId("all-tools")).toBeVisible();
      await audit(page, "all tools");
    });

    test("overlays: palette, menus, dialogs, banners", async ({ page }) => {
      await boot(page, { theme });
      await openSample(page);

      await page.keyboard.press(`${MOD}+k`);
      await expect(page.getByTestId("palette-input")).toBeFocused();
      await page.keyboard.type("zoom");
      await audit(page, "palette");
      await page.keyboard.press("Escape");

      await page.getByTestId("tool-view").click();
      await expect(page.getByRole("menu")).toBeVisible();
      await audit(page, "view menu");
      await page.keyboard.press("Escape");

      await page.getByTestId("settings-menu").click();
      await expect(page.getByRole("menu")).toBeVisible();
      await audit(page, "settings menu");
      await page.keyboard.press("Escape");

      await page.keyboard.press(`${MOD}+g`);
      await expect(page.getByRole("dialog", { name: "Go to page" })).toBeVisible();
      await audit(page, "go to page");
      await page.keyboard.press("Escape");

      await page.keyboard.press(`${MOD}+d`);
      await expect(page.getByRole("dialog", { name: "Document properties" })).toBeVisible();
      await audit(page, "document properties");
      await page.keyboard.press("Escape");

      await page.evaluate(() => {
        const p = (window as any).__papyrine;
        p.host.simulateExternalChange(p.store.getState().activeId);
        p.store.getState().notify("info", "toast.saved");
      });
      await expect(page.getByTestId("banner-changed-on-disk")).toBeVisible();
      await audit(page, "banner + toast");
    });
  });
}

test("axe: forced colors (system theme, Windows contrast mode)", async ({ page }) => {
  await page.emulateMedia({ forcedColors: "active" });
  await boot(page);
  await audit(page, "forced welcome");
  await openSample(page);
  await audit(page, "forced viewer");
  await page.keyboard.press(`${MOD}+k`);
  await expect(page.getByTestId("palette-input")).toBeFocused();
  await audit(page, "forced palette");
});

test("axe: system dark via prefers-color-scheme", async ({ page }) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await boot(page);
  await openSample(page);
  await audit(page, "system dark viewer");
});

test("axe: pseudo-locale and RTL", async ({ page }) => {
  await boot(page, { locale: "ar-XB" });
  await openSample(page);
  await audit(page, "rtl viewer");
  await page.keyboard.press(`${MOD}+f`);
  await expect(page.getByTestId("find-input")).toBeFocused();
  await audit(page, "rtl search");
});
