import * as Dropdown from "@radix-ui/react-dropdown-menu";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { COMMANDS } from "../commands/registry";
import { ariaShortcut, formatShortcut } from "../commands/shortcuts";
import { LOCALES, type LocaleId } from "../i18n";
import { Icon } from "../icons/Icon";
import { type ThemePref, useActiveDoc, useApp } from "../store/app";
import { toolbarTools, type ToolDef } from "../tools/registry";
import { formatZoom, parseZoom } from "../viewer/zoom";
import { IconButton, MenuItem, MenuRadio, Tip } from "./ui";

const shortcutOf = (id: string) => COMMANDS.find((c) => c.id === id)?.shortcut;

export function Toolbar() {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const s = useApp();
  const ref = useRef<HTMLDivElement>(null);
  useRovingToolbar(ref);

  const tools = toolbarTools(s.pinned);
  const dirty = !!doc?.info.dirty;

  return (
    <div ref={ref} className="toolbar" role="toolbar" aria-label={t("toolbar.label")} aria-orientation="horizontal" data-region="toolbar">
      <IconButton toolbarItem icon="open" label={t("toolbar.open")} shortcut="Mod+O" onClick={() => void s.openDialog()} />
      <IconButton
        toolbarItem
        icon="thumbnails"
        label={t("toolbar.togglePane")}
        shortcut="F4"
        pressed={!!s.leftPane}
        onClick={() => s.toggleLeftPane()}
      />
      <span className="sep" role="separator" aria-orientation="vertical" />
      {tools.map((tool) => (tool.kind === "menu" ? <ViewMenu key={tool.id} tool={tool} /> : <ToolButton key={tool.id} tool={tool} />))}
      <span className="spacer" />
      <ZoomControls />
      <span className="sep" role="separator" aria-orientation="vertical" />
      <IconButton
        toolbarItem
        icon="save"
        label={t("toolbar.save")}
        shortcut={shortcutOf("file.save")}
        disabled={!dirty}
        onClick={() => void s.save()}
      />
      <IconButton
        toolbarItem
        icon="tools"
        label={t("toolbar.allTools")}
        shortcut={shortcutOf("pane.allTools")}
        pressed={s.rightTool === "all-tools"}
        onClick={() => s.toggleTool("all-tools")}
      />
      <Tip label={t("toolbar.palette")} shortcut="Mod+K">
        <button
          type="button"
          className="btn btn-outline palette-btn"
          data-tb=""
          aria-label={t("toolbar.palette")}
          aria-keyshortcuts={ariaShortcut("Mod+K")}
          onClick={() => s.setPalette(true)}
        >
          <Icon name="command" size={16} />
          <span className="kbd">{formatShortcut("Mod+K")}</span>
        </button>
      </Tip>
      <SettingsMenu />
    </div>
  );
}

function ToolButton({ tool }: { tool: ToolDef }) {
  const { t } = useTranslation();
  const open = useApp((s) => s.rightTool === tool.id);
  const toggle = useApp((s) => s.toggleTool);
  const setProps = useApp((s) => s.setProps);
  const hasDoc = !!useActiveDoc();
  const title = t(tool.titleKey);
  return (
    <Tip label={title} shortcut={tool.shortcut}>
      <button
        type="button"
        className="btn"
        data-tb=""
        data-testid={`tool-${tool.id}`}
        aria-pressed={tool.kind === "panel" ? open : undefined}
        aria-keyshortcuts={tool.shortcut ? ariaShortcut(tool.shortcut) : undefined}
        disabled={!hasDoc}
        onClick={() => (tool.kind === "dialog" ? setProps(true) : toggle(tool.id))}
      >
        <Icon name={tool.icon} />
        <span className="tool-label">{title}</span>
      </button>
    </Tip>
  );
}

function ViewMenu({ tool }: { tool: ToolDef }) {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const s = useApp();
  const run = (id: string) => () => {
    const c = COMMANDS.find((x) => x.id === id);
    if (c) void c.run(useApp.getState());
  };
  const has = !!doc;
  const title = t(tool.titleKey);
  return (
    <Dropdown.Root modal={false}>
      <Tip label={title}>
        <Dropdown.Trigger asChild>
          <button type="button" className="btn" data-tb="" data-testid="tool-view" disabled={!has}>
            <Icon name={tool.icon} />
            <span className="tool-label">{title}</span>
            <Icon name="chevrondown" size={14} />
          </button>
        </Dropdown.Trigger>
      </Tip>
      <Dropdown.Portal>
        <Dropdown.Content className="menu" sideOffset={6} align="start" aria-label={title}>
          <MenuItem icon="zoom-in" shortcut={shortcutOf("view.zoomIn")} onSelect={run("view.zoomIn")}>{t("cmd.view.zoomIn")}</MenuItem>
          <MenuItem icon="zoom-out" shortcut={shortcutOf("view.zoomOut")} onSelect={run("view.zoomOut")}>{t("cmd.view.zoomOut")}</MenuItem>
          <Dropdown.Separator className="menu-sep" />
          <MenuItem icon="fit-page" shortcut={shortcutOf("view.fitPage")} onSelect={run("view.fitPage")}>{t("cmd.view.fitPage")}</MenuItem>
          <MenuItem icon="fit-width" shortcut={shortcutOf("view.fitWidth")} onSelect={run("view.fitWidth")}>{t("cmd.view.fitWidth")}</MenuItem>
          <MenuItem icon="actual" shortcut={shortcutOf("view.actual")} onSelect={run("view.actual")}>{t("cmd.view.actual")}</MenuItem>
          <Dropdown.Separator className="menu-sep" />
          <Dropdown.Label className="menu-label">{t("menu.viewMode")}</Dropdown.Label>
          <Dropdown.RadioGroup value={doc?.viewMode ?? "continuous"} onValueChange={(v) => s.setViewMode(v as "continuous" | "single")}>
            <MenuRadio value="continuous">{t("cmd.view.continuous")}</MenuRadio>
            <MenuRadio value="single">{t("cmd.view.single")}</MenuRadio>
          </Dropdown.RadioGroup>
          <Dropdown.Separator className="menu-sep" />
          <MenuItem icon="goto" shortcut={shortcutOf("nav.goto")} onSelect={run("nav.goto")}>{t("cmd.nav.goto")}</MenuItem>
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}

function ZoomControls() {
  const { t } = useTranslation();
  const doc = useActiveDoc();
  const setZoom = useApp((s) => s.setZoom);
  const [draft, setDraft] = useState<string | null>(null);
  const zoomLabel = doc ? formatZoom(doc.zoom) : "";
  const run = (id: string) => () => {
    const c = COMMANDS.find((x) => x.id === id);
    if (c) void c.run(useApp.getState());
  };
  const commit = () => {
    if (draft !== null) {
      const z = parseZoom(draft);
      if (z) setZoom(z);
    }
    setDraft(null);
  };
  return (
    <>
      <IconButton toolbarItem icon="zoom-out" label={t("cmd.view.zoomOut")} shortcut="Mod+-" disabled={!doc} onClick={run("view.zoomOut")} />
      <input
        className="zoom-input"
        data-testid="zoom-input"
        aria-label={t("toolbar.zoomLevel")}
        disabled={!doc}
        value={draft ?? zoomLabel}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={(e) => e.currentTarget.select()}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            commit();
            e.currentTarget.select();
          } else if (e.key === "Escape") setDraft(null);
        }}
      />
      <IconButton toolbarItem icon="zoom-in" label={t("cmd.view.zoomIn")} shortcut="Mod+=" disabled={!doc} onClick={run("view.zoomIn")} />
    </>
  );
}

function SettingsMenu() {
  const { t } = useTranslation();
  const theme = useApp((s) => s.theme);
  const locale = useApp((s) => s.locale);
  const setTheme = useApp((s) => s.setTheme);
  const setLocale = useApp((s) => s.setLocalePref);
  return (
    <Dropdown.Root modal={false}>
      <Tip label={t("toolbar.settings")}>
        <Dropdown.Trigger asChild>
          <button type="button" className="btn btn-icon" data-tb="" aria-label={t("toolbar.settings")} data-testid="settings-menu">
            <Icon name="more" />
          </button>
        </Dropdown.Trigger>
      </Tip>
      <Dropdown.Portal>
        <Dropdown.Content className="menu" sideOffset={6} align="end" aria-label={t("toolbar.settings")}>
          <Dropdown.Label className="menu-label">{t("menu.theme")}</Dropdown.Label>
          <Dropdown.RadioGroup value={theme} onValueChange={(v) => setTheme(v as ThemePref)}>
            <MenuRadio value="system">{t("cmd.theme.system")}</MenuRadio>
            <MenuRadio value="light">{t("cmd.theme.light")}</MenuRadio>
            <MenuRadio value="dark">{t("cmd.theme.dark")}</MenuRadio>
            <MenuRadio value="hc">{t("cmd.theme.hc")}</MenuRadio>
          </Dropdown.RadioGroup>
          <Dropdown.Separator className="menu-sep" />
          <Dropdown.Label className="menu-label">{t("menu.language")}</Dropdown.Label>
          <Dropdown.RadioGroup value={locale} onValueChange={(v) => void setLocale(v as LocaleId)}>
            {LOCALES.map((l) => (
              <MenuRadio key={l.id} value={l.id}>
                {t(l.labelKey)}
              </MenuRadio>
            ))}
          </Dropdown.RadioGroup>
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}

/** WAI-ARIA toolbar pattern: one tab stop, arrows/Home/End move between items. */
function useRovingToolbar(ref: React.RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    const items = () => Array.from(root.querySelectorAll<HTMLElement>("[data-tb]")).filter((el) => !(el as HTMLButtonElement).disabled);
    const normalise = (active?: HTMLElement) => {
      const list = items();
      const keep = active && list.includes(active) ? active : (list.find((el) => el.tabIndex === 0) ?? list[0]);
      root.querySelectorAll<HTMLElement>("[data-tb]").forEach((el) => (el.tabIndex = el === keep ? 0 : -1));
    };
    const onFocusIn = (e: FocusEvent) => {
      if ((e.target as HTMLElement).hasAttribute("data-tb")) normalise(e.target as HTMLElement);
    };
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (!target.hasAttribute("data-tb")) return;
      const rtl = getComputedStyle(root).direction === "rtl";
      const list = items();
      const i = list.indexOf(target);
      let next = -1;
      if (e.key === "ArrowRight") next = i + (rtl ? -1 : 1);
      else if (e.key === "ArrowLeft") next = i + (rtl ? 1 : -1);
      else if (e.key === "Home") next = 0;
      else if (e.key === "End") next = list.length - 1;
      else return;
      if (target instanceof HTMLInputElement && (e.key === "Home" || e.key === "End")) return;
      e.preventDefault();
      list[(next + list.length) % list.length]?.focus();
    };
    root.addEventListener("focusin", onFocusIn);
    root.addEventListener("keydown", onKey);
    const mo = new MutationObserver(() => normalise(document.activeElement as HTMLElement));
    mo.observe(root, { childList: true, subtree: true, attributes: true, attributeFilter: ["disabled"] });
    normalise();
    return () => {
      root.removeEventListener("focusin", onFocusIn);
      root.removeEventListener("keydown", onKey);
      mo.disconnect();
    };
  }, [ref]);
}
