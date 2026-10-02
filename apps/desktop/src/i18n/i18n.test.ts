import { beforeAll, describe, expect, it } from "vitest";
import { i18next, initI18n, setLocale, t } from "./index";

describe("i18next + ICU", () => {
  beforeAll(async () => {
    await initI18n("en");
  });

  it("formats plurals per ICU rules", () => {
    expect(t("palette.count", { count: 0 })).toBe("No commands");
    expect(t("palette.count", { count: 1 })).toBe("1 command");
    expect(t("palette.count", { count: 1234 })).toBe("1,234 commands");
  });

  it("interpolates named arguments", () => {
    expect(t("status.page", { page: 3, total: 24 })).toBe("Page 3 of 24");
  });

  it("pseudo-locales keep plural logic", async () => {
    await setLocale("en-XA");
    expect(t("palette.count", { count: 1 })).toMatch(/^\[.*1 .*\]$/);
    expect(t("status.page", { page: 3, total: 24 })).toContain("3");
    await setLocale("ar-XB");
    expect(t("palette.count", { count: 2 })).toContain("‫");
    await setLocale("en");
    expect(i18next.language).toBe("en");
  });
});
