import { test } from "@playwright/test";
import { boot, openSample } from "./helpers";

// Visual review aids; written to the gitignored artifacts dir.
for (const theme of ["light", "dark", "hc"] as const) {
  test(`screenshot ${theme}`, async ({ page }) => {
    await boot(page, { theme });
    await page.screenshot({ path: `e2e/artifacts/welcome-${theme}.png` });
    await openSample(page);
    await page.waitForTimeout(400);
    await page.screenshot({ path: `e2e/artifacts/viewer-${theme}.png` });
  });
}
