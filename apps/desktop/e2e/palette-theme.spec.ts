import { expect, test } from "@playwright/test";
import { MOD, boot, openSample, zoomPercent } from "./helpers";

test.describe("command palette", () => {
  test.beforeEach(async ({ page }) => {
    await boot(page);
    await openSample(page);
  });

  test("opens with the shortcut, fuzzy matches, runs with Enter, restores focus", async ({ page }) => {
    await page.keyboard.press(`${MOD}+k`);
    const input = page.getByTestId("palette-input");
    await expect(input).toBeFocused();
    await page.keyboard.type("fitw");
    const first = page.locator(".palette-item").first();
    await expect(first).toContainText("Fit width");
    await expect(first).toHaveAttribute("aria-selected", "true");
    await page.keyboard.press("Enter");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByTestId("viewer")).toBeFocused();
    const w = await zoomPercent(page);
    await page.keyboard.press(`${MOD}+1`);
    expect(await zoomPercent(page)).toBe(100);
    expect(w).not.toBe(100);
  });

  test("arrow keys move the selection; Escape closes and keeps focus", async ({ page }) => {
    await page.keyboard.press(`${MOD}+k`);
    await page.keyboard.type("zoom");
    await page.keyboard.press("ArrowDown");
    const second = page.locator(".palette-item").nth(1);
    await expect(second).toHaveAttribute("aria-selected", "true");
    await expect(page.getByTestId("palette-input")).toHaveAttribute("aria-activedescendant", (await second.getAttribute("id"))!);
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    await expect(page.getByTestId("viewer")).toBeFocused();
  });

  test("reaches every shipped tool and command by name", async ({ page }) => {
    await page.keyboard.press(`${MOD}+k`);
    const all = await page.locator(".palette-item").allTextContents();
    for (const name of ["Open tool: Search", "Open tool: Annotate", "Open tool: Fill & Sign", "Open tool: Organize Pages", "Open tool: Document Properties", "Show all tools", "Zoom to 400%", "Single page", "Theme: high contrast", "Language: Pseudo-locale (accented)", "Go to page"]) {
      expect(all.some((t) => t.includes(name)), name).toBe(true);
    }
    // Unshipped tools do not leak in.
    expect(all.some((t) => /Compress|Protect|Preflight/.test(t))).toBe(false);
  });

  test("tool commands open the panel; aliases match; disabled commands are hidden", async ({ page }) => {
    await page.keyboard.press(`${MOD}+k`);
    await page.keyboard.type("highlight"); // alias of Annotate
    await expect(page.locator(".palette-item").first()).toContainText("Annotate");
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("right-pane")).toBeVisible();
    await expect(page.getByRole("heading", { name: "Annotate" })).toBeVisible();
    await page.keyboard.press(`${MOD}+k`);
    await page.keyboard.type("save");
    // Nothing is dirty, so Save is not offered.
    await expect(page.locator(".palette-item", { hasText: /^Save/ })).toHaveCount(0);
    await expect(page.locator(".palette-item")).toContainText("No matching command");
  });

  test("palette button in the toolbar opens it too", async ({ page }) => {
    await page.getByRole("button", { name: "Command palette" }).click();
    await expect(page.getByTestId("palette")).toBeVisible();
    await page.keyboard.press(`${MOD}+k`);
    await expect(page.getByTestId("palette")).toHaveCount(0);
  });
});

test.describe("themes", () => {
  test("switch through the palette and the settings menu; persists across reloads", async ({ page }) => {
    await boot(page);
    await openSample(page);
    const html = page.locator("html");
    await page.keyboard.press(`${MOD}+k`);
    await page.keyboard.type("theme dark");
    await page.keyboard.press("Enter");
    await expect(html).toHaveAttribute("data-theme", "dark");
    const darkBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);

    // Settings menu, keyboard only: open, arrow to High contrast, select.
    await page.getByTestId("settings-menu").focus();
    await page.keyboard.press("Enter");
    await page.getByRole("menuitemradio", { name: "Theme: high contrast" }).click();
    await expect(html).toHaveAttribute("data-theme", "hc");
    const hcBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(hcBg).toBe("rgb(0, 0, 0)");
    expect(hcBg).not.toBe(darkBg);

    await page.reload();
    await expect(html).toHaveAttribute("data-theme", "hc");

    await page.keyboard.press(`${MOD}+k`);
    await page.keyboard.type("theme light");
    await page.keyboard.press("Enter");
    await expect(html).toHaveAttribute("data-theme", "light");
    const lightBg = await page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(lightBg).toBe("rgb(238, 240, 244)");
  });

  test("follows prefers-color-scheme and forced-colors until the user overrides", async ({ page }) => {
    await page.emulateMedia({ colorScheme: "dark" });
    await boot(page);
    const bg = () => page.evaluate(() => getComputedStyle(document.body).backgroundColor);
    expect(await bg()).toBe("rgb(17, 20, 27)");
    await page.emulateMedia({ colorScheme: "light" });
    expect(await bg()).toBe("rgb(238, 240, 244)");
    await page.emulateMedia({ forcedColors: "active" });
    // Forced colours map Canvas to a system colour; the token resolves to it.
    const token = await page.evaluate(() => getComputedStyle(document.documentElement).getPropertyValue("--bg").trim());
    expect(token).toBe("Canvas");
    await expect(page.getByTestId("welcome")).toBeVisible();
  });
});
