import { describe, expect, it } from "vitest";
import { fuzzyMatch, rankMatches } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches subsequences and rejects non-subsequences", () => {
    expect(fuzzyMatch("zi", "Zoom in")).not.toBeNull();
    expect(fuzzyMatch("xyz", "Zoom in")).toBeNull();
    expect(fuzzyMatch("", "anything")).toEqual({ score: 0, positions: [] });
  });

  it("is case and diacritic insensitive", () => {
    expect(fuzzyMatch("ZOOM", "zoom in")).not.toBeNull();
    expect(fuzzyMatch("zoom", "Zöóm")).not.toBeNull();
  });

  it("prefers word starts over scattered letters", () => {
    const a = fuzzyMatch("fw", "Fit width")!;
    const b = fuzzyMatch("fw", "Software")!;
    expect(a.score).toBeGreaterThan(b.score);
    expect(a.positions).toEqual([0, 4]);
  });

  it("prefers consecutive runs", () => {
    const a = fuzzyMatch("zoo", "Zoom in")!;
    const b = fuzzyMatch("zoo", "Zip open outline")!;
    expect(a.score).toBeGreaterThan(b.score);
  });

  it("ignores whitespace in the query", () => {
    expect(fuzzyMatch("go to", "Go to page")).not.toBeNull();
  });
});

describe("rankMatches", () => {
  const items = ["Zoom in", "Zoom out", "Fit width", "Toggle navigation pane", "Next page"];
  it("orders by score, stable on ties, and falls back to aliases", () => {
    const r = rankMatches("zo", items, (s) => s).map((x) => x.item);
    expect(r.slice(0, 2)).toEqual(["Zoom in", "Zoom out"]);
    const alias = rankMatches("magnify", items, (s) => s, (s) => (s === "Zoom in" ? "magnify" : ""));
    expect(alias.map((x) => x.item)).toEqual(["Zoom in"]);
  });
  it("returns everything for an empty query", () => {
    expect(rankMatches("  ", items, (s) => s)).toHaveLength(items.length);
  });
});
