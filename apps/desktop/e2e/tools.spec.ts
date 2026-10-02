import { expect, test } from "@playwright/test";
import { MOD, boot, openSample } from "./helpers";

test("toolbar shows core tools only; All Tools panel pins others and persists", async ({ page }) => {
  await boot(page);
  await openSample(page);

  const names = await page.locator('.toolbar [data-testid^="tool-"]').evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
  expect(names).toEqual(["tool-view", "tool-search", "tool-annotate", "tool-fill-sign", "tool-organize"]);

  await page.keyboard.press(`${MOD}+Shift+t`);
  const panel = page.getByTestId("all-tools");
  await expect(panel).toBeVisible();
  await expect(panel.getByRole("heading", { name: "Core tools" })).toBeVisible();
  await expect(panel.getByRole("heading", { name: "Document" })).toBeVisible();
  // Nothing from later milestones, and no empty Advanced disclosure.
  await expect(panel).not.toContainText("Compress");
  await expect(panel.getByRole("button", { name: "Advanced" })).toHaveCount(0);

  const pin = panel.getByRole("button", { name: "Pin Document Properties to the toolbar" });
  await expect(pin).toHaveAttribute("aria-pressed", "false");
  await pin.focus();
  await page.keyboard.press("Enter");
  await expect(pin).toHaveAttribute("aria-pressed", "true");
  await expect(page.getByTestId("tool-doc-properties")).toBeVisible();

  await page.reload();
  await expect(page.getByTestId("welcome")).toBeVisible();
  await page.getByTestId("welcome-open").focus();
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("tool-doc-properties")).toBeVisible();

  // Pinned tool opens its dialog from the toolbar.
  await page.getByTestId("tool-doc-properties").click();
  await expect(page.getByRole("dialog", { name: "Document properties" })).toBeVisible();
});

test("tool panels are lazy chunks, fetched on first use", async ({ page }) => {
  const chunks: string[] = [];
  page.on("request", (r) => {
    const u = r.url();
    if (/SearchPanel|OrganizePanel|AllToolsPanel|PendingPanel/.test(u)) chunks.push(u);
  });
  await boot(page);
  await openSample(page);
  await page.waitForTimeout(300);
  // Idle prefetch may warm Search, but never the others.
  expect(chunks.some((u) => /OrganizePanel|AllToolsPanel|PendingPanel/.test(u))).toBe(false);
  await page.getByTestId("tool-organize").click();
  await expect(page.getByRole("button", { name: "Rotate page right" })).toBeVisible();
  expect(chunks.some((u) => /OrganizePanel/.test(u))).toBe(true);
});

test("annotate and fill & sign panels describe themselves honestly", async ({ page }) => {
  await boot(page);
  await openSample(page);
  await page.getByTestId("tool-annotate").click();
  await expect(page.getByTestId("right-pane")).toContainText("document engine is connected");
  await page.getByTestId("tool-fill-sign").click();
  await expect(page.getByRole("heading", { name: "Fill & Sign" })).toBeVisible();
  await page.keyboard.press(`${MOD}+k`);
  await expect(page.getByTestId("palette-input")).toBeFocused();
  await page.keyboard.type("close tool pane");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("right-pane")).toHaveCount(0);
});
