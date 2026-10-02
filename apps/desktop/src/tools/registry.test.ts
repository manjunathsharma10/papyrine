import { describe, expect, it } from "vitest";
import { COMMANDS } from "../commands/registry";
import { TOOLS, allToolsSections, isShipped, shippedTools, toolbarTools } from "./registry";

describe("tool registry", () => {
  it("ids are unique", () => {
    expect(new Set(TOOLS.map((t) => t.id)).size).toBe(TOOLS.length);
  });

  it("v0.1 ships exactly the core tools plus document properties", () => {
    const ids = shippedTools([0, 1]).map((t) => t.id);
    expect(ids).toEqual(["view", "search", "annotate", "fill-sign", "organize", "doc-properties"]);
  });

  it("toolbar = shipped core tools + pins; unshipped tools can never be pinned onto it", () => {
    expect(toolbarTools([]).map((t) => t.id)).toEqual(["view", "search", "annotate", "fill-sign", "organize"]);
    expect(toolbarTools(["doc-properties"]).map((t) => t.id)).toContain("doc-properties");
    expect(toolbarTools(["protect"]).map((t) => t.id)).not.toContain("protect");
  });

  it("tools appear when their milestone ships, grouped by tier", () => {
    expect(isShipped(TOOLS.find((t) => t.id === "compress")!, [0, 1])).toBe(false);
    const later = allToolsSections([0, 7]);
    expect(later.map((s) => `${s.tier}:${s.group}`)).toEqual([
      "core:core",
      "standard:document",
      "standard:edit",
      "standard:protect",
      "advanced:print",
    ]);
    expect(later[0]!.tools.map((t) => t.id)).toContain("compress");
  });

  it("every shipped tool is reachable from the command palette (except the View menu)", () => {
    for (const tool of shippedTools()) {
      if (tool.kind === "menu") continue;
      expect(COMMANDS.some((c) => c.id === `tool.${tool.id}`), tool.id).toBe(true);
    }
  });

  it("command ids and shortcuts are unique", () => {
    const ids = COMMANDS.map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
    const sc = COMMANDS.filter((c) => c.shortcut).map((c) => c.shortcut);
    expect(new Set(sc).size).toBe(sc.length);
  });
});
