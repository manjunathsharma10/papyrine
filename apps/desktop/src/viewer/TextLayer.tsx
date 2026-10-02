import { memo, useLayoutEffect, useRef, useState } from "react";
import type { DocId, PageText } from "../ipc/contract";
import { findMatches } from "./textMatch";
import { getTextCache } from "./services";
import type { SearchState } from "../store/app";

interface Props {
  docId: DocId;
  page: number;
  revision: number;
  /** Page size in points: the layer is laid out in points and scaled by CSS. */
  widthPt: number;
  heightPt: number;
  /** CSS px per point. */
  cssScale: number;
  search: SearchState | null;
}

/**
 * Invisible positioned text over the page, for selection, find highlights and
 * screen readers. Laid out once in point units; zoom is a single CSS scale, so
 * zooming never re-measures.
 */
export const TextLayer = memo(function TextLayer({ docId, page, revision, widthPt, heightPt, cssScale, search }: Props) {
  const [text, setText] = useState<PageText | null>(null);
  const root = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    let live = true;
    setText(null);
    getTextCache()
      .get(docId, page, revision)
      .then((t) => live && setText(t))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [docId, page, revision]);

  // Stretch each run horizontally so the selectable text matches the drawn glyph widths.
  useLayoutEffect(() => {
    const el = root.current;
    if (!el || !text) return;
    const spans = Array.from(el.querySelectorAll<HTMLElement>("[data-run]"));
    for (const s of spans) s.style.transform = "";
    const natural = spans.map((s) => s.scrollWidth);
    spans.forEach((s, i) => {
      const run = text.runs[i];
      const w = natural[i] ?? 0;
      if (run && w > 0) s.style.transform = `scaleX(${run.width / w})`;
    });
  }, [text]);

  const match = search && search.docId === docId && search.query ? search : null;

  return (
    <div
      ref={root}
      className="text-layer"
      style={{ width: widthPt, height: heightPt, transform: `scale(${cssScale})` }}
    >
      {text?.runs.map((run, i) => {
        const ranges = match ? findMatches(run.text, match.query, match.caseSensitive, match.wholeWord) : [];
        return (
          <span
            key={i}
            data-run=""
            style={{ left: run.x, top: run.y, height: run.height, fontSize: run.height * 0.85, lineHeight: `${run.height}px` }}
          >
            {ranges.length === 0 ? run.text : renderMarked(run.text, ranges)}
          </span>
        );
      })}
    </div>
  );
});

function renderMarked(text: string, ranges: { start: number; length: number }[]) {
  const out: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach((r, i) => {
    if (r.start > at) out.push(text.slice(at, r.start));
    out.push(<mark key={i}>{text.slice(r.start, r.start + r.length)}</mark>);
    at = r.start + r.length;
  });
  if (at < text.length) out.push(text.slice(at));
  return out;
}
