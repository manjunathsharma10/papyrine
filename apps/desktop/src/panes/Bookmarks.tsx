import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Scroll } from "../components/Scroll";
import { Icon } from "../icons/Icon";
import type { OutlineNode } from "../ipc/contract";
import { type DocState, useApp } from "../store/app";

interface Row {
  id: string;
  node: OutlineNode;
  depth: number;
  hasChildren: boolean;
  parent: string | null;
}

function flatten(nodes: OutlineNode[], open: Set<string>, depth = 0, prefix = "", parent: string | null = null, out: Row[] = []): Row[] {
  nodes.forEach((node, i) => {
    const id = `${prefix}${i}`;
    out.push({ id, node, depth, hasChildren: node.children.length > 0, parent });
    if (open.has(id)) flatten(node.children, open, depth + 1, `${id}.`, id, out);
  });
  return out;
}

/** Outline tree (WAI-ARIA tree pattern with aria-activedescendant). */
export function Bookmarks({ doc }: { doc: DocState }) {
  const { t } = useTranslation();
  const goTo = useApp((s) => s.goToPage);
  const [open, setOpen] = useState<Set<string>>(() => new Set(["0", "1", "2"]));
  const [active, setActive] = useState("0");
  const rows = useMemo(() => flatten(doc.outline ?? [], open), [doc.outline, open]);

  if (doc.outline === null) return <p className="empty-note">{t("bookmarks.loading")}</p>;
  if (doc.outline.length === 0) return <p className="empty-note">{t("bookmarks.none")}</p>;

  const idx = Math.max(0, rows.findIndex((r) => r.id === active));
  const row = rows[idx];
  const toggle = (id: string, to?: boolean) =>
    setOpen((o) => {
      const n = new Set(o);
      if (to ?? !n.has(id)) n.add(id);
      else n.delete(id);
      return n;
    });
  const activate = (r: Row) => {
    setActive(r.id);
    if (r.node.page !== null) goTo(r.node.page);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!row) return;
    const move = (i: number) => {
      const r = rows[Math.max(0, Math.min(rows.length - 1, i))];
      if (r) setActive(r.id);
    };
    switch (e.key) {
      case "ArrowDown": move(idx + 1); break;
      case "ArrowUp": move(idx - 1); break;
      case "Home": move(0); break;
      case "End": move(rows.length - 1); break;
      case "ArrowRight":
      case "ArrowLeft": {
        const forward = (e.key === "ArrowRight") !== (document.documentElement.dir === "rtl");
        if (forward) {
          if (row.hasChildren && !open.has(row.id)) toggle(row.id, true);
          else move(idx + 1);
        } else if (row.hasChildren && open.has(row.id)) toggle(row.id, false);
        else if (row.parent) setActive(row.parent);
        break;
      }
      case "Enter":
      case " ": activate(row); break;
      default: return;
    }
    e.preventDefault();
  };

  return (
    <Scroll label={t("bookmarks.scrollLabel")}>
      <ul className="tree" role="tree" tabIndex={0} aria-label={t("bookmarks.label")} aria-activedescendant={`bm-${active}`} onKeyDown={onKeyDown} data-region="left" data-testid="bookmarks">
        {rows.map((r) => (
          <li key={r.id} role="none" style={{ paddingInlineStart: r.depth * 16 }}>
            <div
              id={`bm-${r.id}`}
              role="treeitem"
              aria-level={r.depth + 1}
              aria-expanded={r.hasChildren ? open.has(r.id) : undefined}
              aria-selected={r.id === active}
              className="tree-item"
              data-active={r.id === active}
              onClick={() => activate(r)}
            >
              <span
                className="twisty"
                aria-hidden="true"
                onClick={(e) => {
                  e.stopPropagation();
                  if (r.hasChildren) toggle(r.id);
                }}
              >
                {r.hasChildren && <Icon name={open.has(r.id) ? "chevrondown" : "chevronright"} size={14} className={open.has(r.id) ? undefined : "icon-mirror"} />}
              </span>
              <span>{r.node.title}</span>
              {r.node.page !== null && <span className="page-no">{r.node.page + 1}</span>}
            </div>
          </li>
        ))}
      </ul>
    </Scroll>
  );
}
