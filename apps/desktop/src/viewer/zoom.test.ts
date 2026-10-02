import { describe, expect, it } from "vitest";
import {
  CSS_PX_PER_PT,
  MAX_ZOOM,
  MIN_ZOOM,
  PAGE_GAP,
  VIEW_PAD,
  clampZoom,
  fitPageZoom,
  fitWidthZoom,
  formatZoom,
  layoutPages,
  parseZoom,
  stepZoom,
  tileScale,
  wheelZoom,
} from "./zoom";

const letter = { width: 612, height: 792, rotation: 0 as const };

describe("zoom", () => {
  it("clamps to 10%..6400%", () => {
    expect(clampZoom(0.001)).toBe(MIN_ZOOM);
    expect(clampZoom(1000)).toBe(MAX_ZOOM);
    expect(MIN_ZOOM).toBe(0.1);
    expect(MAX_ZOOM).toBe(64);
    expect(clampZoom(NaN)).toBe(1);
  });

  it("steps through presets and never gets stuck at the ends", () => {
    expect(stepZoom(1, 1)).toBe(1.25);
    expect(stepZoom(1, -1)).toBe(0.75);
    expect(stepZoom(0.9, 1)).toBe(1);
    expect(stepZoom(64, 1)).toBe(64);
    expect(stepZoom(0.1, -1)).toBe(0.1);
    expect(stepZoom(0.25, -1)).toBe(0.1);
  });

  it("wheel zoom is multiplicative and reversible", () => {
    const z = wheelZoom(wheelZoom(1, -40), 40);
    expect(z).toBeCloseTo(1, 6);
    expect(wheelZoom(1, -40)).toBeGreaterThan(1);
    expect(wheelZoom(63, -10000)).toBe(MAX_ZOOM);
  });

  it("fit modes", () => {
    const w = 1000 + 2 * VIEW_PAD;
    expect(fitWidthZoom(w, 612)).toBeCloseTo(1000 / (612 * CSS_PX_PER_PT), 8);
    const fp = fitPageZoom(2000, 600 + 2 * VIEW_PAD, letter);
    expect(fp).toBeCloseTo(600 / (792 * CSS_PX_PER_PT), 8);
  });

  it("parses and formats user input", () => {
    expect(parseZoom("150")).toBe(1.5);
    expect(parseZoom(" 12,5 % ")).toBe(0.125);
    expect(parseZoom("abc")).toBeNull();
    expect(parseZoom("0")).toBeNull();
    expect(parseZoom("999999")).toBe(MAX_ZOOM);
    expect(formatZoom(1)).toBe("100%");
    expect(formatZoom(0.125)).toBe("12.5%");
  });

  it("lays pages out in a padded vertical stack", () => {
    const l = layoutPages([letter, letter], 1, 500);
    const h = 792 * CSS_PX_PER_PT;
    expect(l.tops).toEqual([VIEW_PAD, VIEW_PAD + h + PAGE_GAP]);
    expect(l.totalHeight).toBeCloseTo(VIEW_PAD * 2 + h * 2 + PAGE_GAP, 6);
    expect(l.totalWidth).toBeCloseTo(612 * CSS_PX_PER_PT + 2 * VIEW_PAD, 6);
  });

  it("quantises tile scale within 5% of the exact scale", () => {
    for (const z of [0.1, 0.333, 1, 1.7, 8, 64]) {
      for (const dpr of [1, 2]) {
        const exact = z * CSS_PX_PER_PT * dpr;
        const q = tileScale(z, dpr);
        expect(Math.abs(q / exact - 1)).toBeLessThan(0.05);
      }
    }
  });
});
