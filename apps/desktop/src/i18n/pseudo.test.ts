import { IntlMessageFormat } from "intl-messageformat";
import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import { generatePseudo, mapIcuLiterals, pseudoAccent, pseudoRtl } from "./pseudo";

const catalog = en as Record<string, string>;
const args = { name: "doc.pdf", page: 3, total: 24, count: 2, percent: 50, tool: "T", language: "L", done: 1, detail: "d" };

describe("pseudo-localisation", () => {
  it("keeps ICU arguments, plural keywords and selectors intact", () => {
    const m = "{count, plural, =0 {No items} one {# item} other {# items}} in {name}";
    const out = mapIcuLiterals(m, (t) => t.toUpperCase());
    expect(out).toBe("{count, plural, =0 {NO ITEMS} one {# ITEM} other {# ITEMS}} IN {name}");
  });

  it("leaves quoted braces alone", () => {
    expect(mapIcuLiterals("use '{x}' now", (t) => t.toUpperCase())).toBe("USE '{x}' NOW");
  });

  for (const [id, fn] of [["accent", pseudoAccent], ["rtl", pseudoRtl]] as const) {
    it(`${id}: every English message stays a valid ICU message with the same arguments`, () => {
      const pseudo = generatePseudo(catalog, fn);
      expect(Object.keys(pseudo)).toEqual(Object.keys(catalog));
      for (const [key, msg] of Object.entries(pseudo)) {
        const formatted = new IntlMessageFormat(msg, "en").format(args);
        expect(typeof formatted === "string" || Array.isArray(formatted), key).toBe(true);
        const original = new IntlMessageFormat(catalog[key] as string, "en");
        const names = (s: string) => [...s.matchAll(/\{(\w+)[,}]/g)].map((m) => m[1]).sort().join();
        expect(names(msg), key).toBe(names(catalog[key] as string));
        void original;
      }
    });
  }

  it("accent pseudo-locale is longer and visibly accented", () => {
    const out = pseudoAccent("Open a file");
    expect(out.length).toBeGreaterThan("Open a file".length * 1.3);
    expect(out).toContain("Öþéñ");
    expect(out.startsWith("[")).toBe(true);
  });

  it("rtl pseudo-locale wraps the whole message in one embedding", () => {
    expect(pseudoRtl("Save")).toBe("‫Save‬");
    expect(pseudoRtl("Page {n}")).toBe("\u202BPage {n}\u202C");
  });
});
