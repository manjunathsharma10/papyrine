import * as Dialog from "@radix-ui/react-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { rankMatches } from "../commands/fuzzy";
import { COMMANDS, type Command } from "../commands/registry";
import { formatShortcut } from "../commands/shortcuts";
import { Icon } from "../icons/Icon";
import { useApp } from "../store/app";
import { restoreFocus } from "./focus";
import type { TFunction } from "i18next";

/** Resolve a command title; params starting with "@" are catalog keys. */
export function commandTitle(cmd: Command, t: TFunction): string {
  const params: Record<string, string | number> = {};
  for (const [k, v] of Object.entries(cmd.titleParams ?? {})) params[k] = typeof v === "string" && v.startsWith("@") ? t(v.slice(1)) : v;
  return t(cmd.titleKey, params);
}

function Highlighted({ text, positions }: { text: string; positions: number[] }) {
  if (positions.length === 0) return <>{text}</>;
  const set = new Set(positions);
  const chars = Array.from(text);
  const out: React.ReactNode[] = [];
  let run = "";
  let marked = false;
  const flush = (key: number) => {
    if (!run) return;
    out.push(marked ? <mark key={key}>{run}</mark> : run);
    run = "";
  };
  chars.forEach((ch, i) => {
    const m = set.has(i);
    if (m !== marked) {
      flush(i);
      marked = m;
    }
    run += ch;
  });
  flush(chars.length);
  return <>{out}</>;
}

export default function CommandPalette() {
  const { t } = useTranslation();
  const open = useApp((s) => s.paletteOpen);
  const setOpen = useApp((s) => s.setPalette);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  // Enabled-ness is read at open and after every store change while open.
  const state = useApp();

  useEffect(() => {
    if (open) {
      setQuery("");
      setActive(0);
    }
  }, [open]);

  const results = useMemo(() => {
    if (!open) return [];
    const enabled = COMMANDS.filter((c) => !c.hidden && (c.enabled?.(state) ?? true));
    return rankMatches(
      query,
      enabled.map((cmd) => ({ cmd, title: commandTitle(cmd, t), category: t(cmd.categoryKey) })),
      (x) => x.title,
      (x) => `${x.category} ${x.cmd.keywords ?? ""}`,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, state.activeId, state.docs, state.tabs, state.rightTool, t, open]);

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`#cmd-opt-${active}`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const run = (i: number) => {
    const r = results[i];
    if (!r) return;
    setOpen(false);
    // Let the dialog release focus first so commands that focus a pane (Search) win.
    setTimeout(() => void r.item.cmd.run(useApp.getState()), 0);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") setActive((a) => Math.min(results.length - 1, a + 1));
    else if (e.key === "ArrowUp") setActive((a) => Math.max(0, a - 1));
    else if (e.key === "Home" && e.ctrlKey) setActive(0);
    else if (e.key === "End" && e.ctrlKey) setActive(results.length - 1);
    else if (e.key === "PageDown") setActive((a) => Math.min(results.length - 1, a + 8));
    else if (e.key === "PageUp") setActive((a) => Math.max(0, a - 8));
    else if (e.key === "Enter") run(active);
    else return;
    e.preventDefault();
  };

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay" />
        <Dialog.Content className="dialog" aria-describedby={undefined} data-testid="palette" onCloseAutoFocus={restoreFocus}>
          <Dialog.Title className="sr-only">{t("palette.title")}</Dialog.Title>
          <input
            className="palette-input"
            role="combobox"
            aria-expanded="true"
            aria-controls="cmd-list"
            aria-autocomplete="list"
            aria-activedescendant={results.length ? `cmd-opt-${active}` : undefined}
            aria-label={t("palette.label")}
            placeholder={t("palette.placeholder")}
            value={query}
            autoFocus
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            data-testid="palette-input"
          />
          <ul id="cmd-list" ref={list} className="palette-list" role="listbox" aria-label={t("palette.results")}>
            {results.map((r, i) => (
              <li
                key={r.item.cmd.id}
                id={`cmd-opt-${i}`}
                role="option"
                aria-selected={i === active}
                className="palette-item"
                data-cmd={r.item.cmd.id}
                onMouseMove={() => active !== i && setActive(i)}
                onClick={() => run(i)}
              >
                {r.item.cmd.icon ? <Icon name={r.item.cmd.icon} size={16} /> : <span style={{ inlineSize: 16 }} />}
                <span>
                  <Highlighted text={r.item.title} positions={r.match.positions} />
                </span>
                <span className="cat">{r.item.category}</span>
                {r.item.cmd.shortcut && <span className="kbd">{formatShortcut(r.item.cmd.shortcut)}</span>}
              </li>
            ))}
            {results.length === 0 && (
              <li role="option" aria-selected="false" aria-disabled="true" className="palette-item">
                {t("palette.empty")}
              </li>
            )}
          </ul>
          <div role="status" className="sr-only">
            {t("palette.count", { count: results.length })}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
