import type { IconName } from "../icons/Icon";

export type Tier = "core" | "standard" | "advanced";
export type ToolKind = "panel" | "menu" | "dialog";

export type ToolGroup = "document" | "edit" | "protect" | "convert" | "forms" | "compare" | "print" | "accessibility";

export interface ToolDef {
  id: string;
  /** Catalog keys. */
  titleKey: string;
  descriptionKey: string;
  icon: IconName;
  tier: Tier;
  group?: ToolGroup;
  /** First roadmap milestone the tool ships in, as [major, minor]. */
  milestone: readonly [number, number];
  kind: ToolKind;
  shortcut?: string;
  keywords?: string[];
}

/**
 * The tool registry (ARCHITECTURE 1.4). A tool is visible only once its
 * milestone has shipped: there are no placeholders. Later-milestone entries
 * live here so tiers and groups are exercised, and appear when
 * SHIPPED_MILESTONE advances.
 */
export const SHIPPED_MILESTONE: readonly [number, number] = [0, 1];

export const TOOLS: readonly ToolDef[] = [
  { id: "view", titleKey: "tool.view.title", descriptionKey: "tool.view.description", icon: "view", tier: "core", milestone: [0, 1], kind: "menu", keywords: ["zoom", "fit", "mode"] },
  { id: "search", titleKey: "tool.search.title", descriptionKey: "tool.search.description", icon: "search", tier: "core", milestone: [0, 1], kind: "panel", shortcut: "Mod+F", keywords: ["find"] },
  { id: "annotate", titleKey: "tool.annotate.title", descriptionKey: "tool.annotate.description", icon: "annotate", tier: "core", milestone: [0, 1], kind: "panel", keywords: ["highlight", "comment", "note"] },
  { id: "fill-sign", titleKey: "tool.fillSign.title", descriptionKey: "tool.fillSign.description", icon: "fillsign", tier: "core", milestone: [0, 1], kind: "panel", keywords: ["form", "signature"] },
  { id: "organize", titleKey: "tool.organize.title", descriptionKey: "tool.organize.description", icon: "organize", tier: "core", milestone: [0, 1], kind: "panel", keywords: ["pages", "rotate", "delete", "reorder"] },
  { id: "doc-properties", titleKey: "tool.docProperties.title", descriptionKey: "tool.docProperties.description", icon: "info", tier: "standard", group: "document", milestone: [0, 1], kind: "dialog", shortcut: "Mod+D", keywords: ["metadata", "title", "author"] },
  // Later milestones: hidden until shipped.
  { id: "compress", titleKey: "tool.compress.title", descriptionKey: "tool.compress.description", icon: "save", tier: "core", milestone: [0, 2], kind: "panel" },
  { id: "protect", titleKey: "tool.protect.title", descriptionKey: "tool.protect.description", icon: "pin", tier: "standard", group: "protect", milestone: [0, 3], kind: "panel" },
  { id: "edit", titleKey: "tool.edit.title", descriptionKey: "tool.edit.description", icon: "annotate", tier: "standard", group: "edit", milestone: [0, 4], kind: "panel" },
  { id: "preflight", titleKey: "tool.preflight.title", descriptionKey: "tool.preflight.description", icon: "check", tier: "advanced", group: "print", milestone: [0, 7], kind: "panel" },
];

export const GROUP_ORDER: readonly ToolGroup[] = ["document", "edit", "protect", "convert", "forms", "compare", "print", "accessibility"];

export function isShipped(tool: ToolDef, shipped: readonly [number, number] = SHIPPED_MILESTONE): boolean {
  return tool.milestone[0] < shipped[0] || (tool.milestone[0] === shipped[0] && tool.milestone[1] <= shipped[1]);
}

export function shippedTools(shipped: readonly [number, number] = SHIPPED_MILESTONE, all: readonly ToolDef[] = TOOLS): ToolDef[] {
  return all.filter((t) => isShipped(t, shipped));
}

/** Tools shown on the toolbar: every shipped Core tool plus the user's pins. */
export function toolbarTools(pinned: readonly string[], shipped = SHIPPED_MILESTONE, all: readonly ToolDef[] = TOOLS): ToolDef[] {
  return shippedTools(shipped, all).filter((t) => t.tier === "core" || pinned.includes(t.id));
}

export interface ToolSection {
  group: ToolGroup | "core";
  tier: Tier;
  tools: ToolDef[];
}

/** All Tools panel layout: Core, then Standard grouped, then Advanced grouped. */
export function allToolsSections(shipped = SHIPPED_MILESTONE, all: readonly ToolDef[] = TOOLS): ToolSection[] {
  const list = shippedTools(shipped, all);
  const out: ToolSection[] = [];
  const core = list.filter((t) => t.tier === "core");
  if (core.length) out.push({ group: "core", tier: "core", tools: core });
  for (const tier of ["standard", "advanced"] as const) {
    for (const group of GROUP_ORDER) {
      const tools = list.filter((t) => t.tier === tier && t.group === group);
      if (tools.length) out.push({ group, tier, tools });
    }
  }
  return out;
}

export function findTool(id: string): ToolDef | undefined {
  return TOOLS.find((t) => t.id === id);
}
