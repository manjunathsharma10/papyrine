import type { IconName } from "../icons/Icon";
import { LOCALES } from "../i18n";
import { type AppState, activeDoc, useApp } from "../store/app";
import { shippedTools } from "../tools/registry";
import { stepZoom } from "../viewer/zoom";

export interface Command {
  id: string;
  titleKey: string;
  titleParams?: Record<string, string | number>;
  categoryKey: string;
  icon?: IconName;
  shortcut?: string;
  /** Extra search aliases (matched below title hits). */
  keywords?: string;
  enabled?: (s: AppState) => boolean;
  /** Hide from the palette (shortcut-only commands). */
  hidden?: boolean;
  run: (s: AppState) => void | Promise<void>;
}

const hasDoc = (s: AppState) => !!activeDoc(s);
const doc = (s: AppState) => activeDoc(s);

const ZOOM_PRESETS = [50, 75, 100, 125, 150, 200, 400];

export function buildCommands(): Command[] {
  const list: Command[] = [
    { id: "file.open", titleKey: "cmd.file.open", categoryKey: "cat.file", icon: "open", shortcut: "Mod+O", run: (s) => s.openDialog() },
    { id: "file.save", titleKey: "cmd.file.save", categoryKey: "cat.file", icon: "save", shortcut: "Mod+S", enabled: (s) => !!doc(s)?.info.dirty, run: (s) => s.save() },
    { id: "file.close", titleKey: "cmd.file.close", categoryKey: "cat.file", icon: "close", shortcut: "Mod+W", enabled: hasDoc, run: (s) => (s.activeId ? s.closeTab(s.activeId) : undefined) },
    { id: "edit.undo", titleKey: "cmd.edit.undo", categoryKey: "cat.edit", icon: "undo", shortcut: "Mod+Z", enabled: (s) => !!doc(s)?.info.canUndo, run: (s) => s.undo() },
    { id: "edit.redo", titleKey: "cmd.edit.redo", categoryKey: "cat.edit", icon: "redo", shortcut: "Mod+Shift+Z", enabled: (s) => !!doc(s)?.info.canRedo, run: (s) => s.redo() },
    { id: "tab.next", titleKey: "cmd.tab.next", categoryKey: "cat.window", shortcut: "Ctrl+PageDown", enabled: (s) => s.tabs.length > 1, run: (s) => s.cycleTab(1) },
    { id: "tab.prev", titleKey: "cmd.tab.prev", categoryKey: "cat.window", shortcut: "Ctrl+PageUp", enabled: (s) => s.tabs.length > 1, run: (s) => s.cycleTab(-1) },
    { id: "palette.open", titleKey: "cmd.palette.open", categoryKey: "cat.window", icon: "command", shortcut: "Mod+K", hidden: true, run: (s) => s.setPalette(!s.paletteOpen) },

    { id: "nav.next", titleKey: "cmd.nav.next", categoryKey: "cat.navigate", icon: "chevrondown", shortcut: "Mod+ArrowDown", enabled: hasDoc, run: (s) => s.stepPage(1) },
    { id: "nav.prev", titleKey: "cmd.nav.prev", categoryKey: "cat.navigate", icon: "chevronup", shortcut: "Mod+ArrowUp", enabled: hasDoc, run: (s) => s.stepPage(-1) },
    { id: "nav.first", titleKey: "cmd.nav.first", categoryKey: "cat.navigate", shortcut: "Mod+Home", enabled: hasDoc, run: (s) => s.goToPage(0) },
    { id: "nav.last", titleKey: "cmd.nav.last", categoryKey: "cat.navigate", shortcut: "Mod+End", enabled: hasDoc, run: (s) => s.goToPage(Number.MAX_SAFE_INTEGER) },
    { id: "nav.goto", titleKey: "cmd.nav.goto", categoryKey: "cat.navigate", icon: "goto", shortcut: "Mod+G", enabled: hasDoc, run: (s) => s.setGoTo(true) },
    { id: "nav.back", titleKey: "cmd.nav.back", categoryKey: "cat.navigate", icon: "arrowleft", shortcut: "Alt+ArrowLeft", enabled: (s) => (doc(s)?.back.length ?? 0) > 0, run: (s) => s.historyBack() },
    { id: "nav.forward", titleKey: "cmd.nav.forward", categoryKey: "cat.navigate", icon: "arrowright", shortcut: "Alt+ArrowRight", enabled: (s) => (doc(s)?.forward.length ?? 0) > 0, run: (s) => s.historyForward() },

    { id: "view.zoomIn", titleKey: "cmd.view.zoomIn", categoryKey: "cat.view", icon: "zoom-in", shortcut: "Mod+=", enabled: hasDoc, run: (s) => s.setZoom(stepZoom(doc(s)?.zoom ?? 1, 1)) },
    { id: "view.zoomOut", titleKey: "cmd.view.zoomOut", categoryKey: "cat.view", icon: "zoom-out", shortcut: "Mod+-", enabled: hasDoc, run: (s) => s.setZoom(stepZoom(doc(s)?.zoom ?? 1, -1)) },
    { id: "view.fitPage", titleKey: "cmd.view.fitPage", categoryKey: "cat.view", icon: "fit-page", shortcut: "Mod+0", enabled: hasDoc, run: (s) => s.setZoomMode("fit-page") },
    { id: "view.fitWidth", titleKey: "cmd.view.fitWidth", categoryKey: "cat.view", icon: "fit-width", shortcut: "Mod+2", enabled: hasDoc, run: (s) => s.setZoomMode("fit-width") },
    { id: "view.actual", titleKey: "cmd.view.actual", categoryKey: "cat.view", icon: "actual", shortcut: "Mod+1", enabled: hasDoc, run: (s) => s.setZoomMode("actual") },
    ...ZOOM_PRESETS.map<Command>((p) => ({
      id: `view.zoom.${p}`,
      titleKey: "cmd.view.zoomTo",
      titleParams: { percent: p },
      categoryKey: "cat.view",
      icon: "view",
      enabled: hasDoc,
      run: (s) => s.setZoom(p / 100),
    })),
    { id: "view.continuous", titleKey: "cmd.view.continuous", categoryKey: "cat.view", icon: "page-continuous", enabled: hasDoc, run: (s) => s.setViewMode("continuous") },
    { id: "view.single", titleKey: "cmd.view.single", categoryKey: "cat.view", icon: "page-single", enabled: hasDoc, run: (s) => s.setViewMode("single") },

    { id: "pane.thumbnails", titleKey: "cmd.pane.thumbnails", categoryKey: "cat.panes", icon: "thumbnails", shortcut: "Mod+Shift+1", run: (s) => s.setLeftPane("thumbnails") },
    { id: "pane.bookmarks", titleKey: "cmd.pane.bookmarks", categoryKey: "cat.panes", icon: "bookmarks", shortcut: "Mod+Shift+2", run: (s) => s.setLeftPane("bookmarks") },
    { id: "pane.toggleLeft", titleKey: "cmd.pane.toggleLeft", categoryKey: "cat.panes", shortcut: "F4", run: (s) => s.toggleLeftPane() },
    { id: "pane.closeRight", titleKey: "cmd.pane.closeRight", categoryKey: "cat.panes", enabled: (s) => !!s.rightTool, run: (s) => s.setRightTool(null) },
    { id: "pane.allTools", titleKey: "cmd.pane.allTools", categoryKey: "cat.panes", icon: "tools", shortcut: "Mod+Shift+T", run: (s) => s.toggleTool("all-tools") },
    { id: "pane.focusNext", titleKey: "cmd.pane.focusNext", categoryKey: "cat.panes", shortcut: "F6", hidden: true, run: () => cycleRegion(1) },
    { id: "pane.focusPrev", titleKey: "cmd.pane.focusPrev", categoryKey: "cat.panes", shortcut: "Shift+F6", hidden: true, run: () => cycleRegion(-1) },

    { id: "theme.system", titleKey: "cmd.theme.system", categoryKey: "cat.appearance", icon: "system", run: (s) => s.setTheme("system") },
    { id: "theme.light", titleKey: "cmd.theme.light", categoryKey: "cat.appearance", icon: "sun", run: (s) => s.setTheme("light") },
    { id: "theme.dark", titleKey: "cmd.theme.dark", categoryKey: "cat.appearance", icon: "moon", run: (s) => s.setTheme("dark") },
    { id: "theme.hc", titleKey: "cmd.theme.hc", categoryKey: "cat.appearance", icon: "contrast", run: (s) => s.setTheme("hc") },
    ...LOCALES.map<Command>((l) => ({
      id: `locale.${l.id}`,
      titleKey: "cmd.locale.set",
      titleParams: { language: `@${l.labelKey}` },
      categoryKey: "cat.appearance",
      icon: "globe",
      run: (s) => s.setLocalePref(l.id),
    })),
  ];

  // Every shipped tool is reachable by name, whatever its tier.
  for (const tool of shippedTools()) {
    if (tool.kind === "menu") continue;
    list.push({
      id: `tool.${tool.id}`,
      titleKey: "cmd.tool.open",
      titleParams: { tool: `@${tool.titleKey}` },
      categoryKey: "cat.tools",
      icon: tool.icon,
      shortcut: tool.shortcut,
      keywords: tool.keywords?.join(" "),
      enabled: hasDoc,
      run: (s) => (tool.kind === "dialog" ? s.setProps(true) : s.toggleTool(tool.id)),
    });
  }
  return list;
}

export const COMMANDS: Command[] = buildCommands();

export function runCommand(id: string): void {
  const cmd = COMMANDS.find((c) => c.id === id);
  const s = useApp.getState();
  if (cmd && (cmd.enabled?.(s) ?? true)) void cmd.run(s);
}

/** F6 moves focus between the main regions, like a desktop app. */
export function cycleRegion(dir: 1 | -1): void {
  const regions = Array.from(document.querySelectorAll<HTMLElement>("[data-region]")).filter((r) => r.offsetParent !== null);
  if (regions.length === 0) return;
  const active = document.activeElement;
  const cur = regions.findIndex((r) => r.contains(active));
  const next = regions[(cur + dir + regions.length) % regions.length] as HTMLElement;
  const target = next.matches("[tabindex],button,input") ? next : (next.querySelector<HTMLElement>("[tabindex],button,input,[role=tab]") ?? next);
  target.focus();
}
