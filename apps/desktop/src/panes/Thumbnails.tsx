import { useVirtualizer } from "@tanstack/react-virtual";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Scroll } from "../components/Scroll";
import { type DocState, useApp } from "../store/app";
import { getTileCache } from "../viewer/services";
import { TILE_SIZE, type PageInfo } from "../ipc/contract";

const THUMB_W = 120;
const LABEL_H = 22;
const PAD = 12;

const thumbHeight = (p: PageInfo) => Math.round((THUMB_W * p.height) / p.width);

/** Virtualised page thumbnails; one tab stop, arrows move between pages (listbox pattern). */
export function Thumbnails({ doc, docId }: { doc: DocState; docId: string }) {
  const { t } = useTranslation();
  const viewport = useRef<HTMLDivElement>(null);
  const goTo = useApp((s) => s.goToPage);
  const pages = doc.info.pages;
  const v = useVirtualizer({
    count: pages.length,
    getScrollElement: () => viewport.current,
    estimateSize: (i) => thumbHeight(pages[i]!) + LABEL_H + PAD,
    overscan: 4,
  });
  useEffect(() => v.measure(), [v, pages]);

  // Keep the current page's thumbnail visible.
  useEffect(() => {
    v.scrollToIndex(doc.currentPage, { align: "auto" });
  }, [v, doc.currentPage]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const cur = doc.currentPage;
    const next =
      e.key === "ArrowDown" ? cur + 1 : e.key === "ArrowUp" ? cur - 1 : e.key === "Home" ? 0 : e.key === "End" ? pages.length - 1 : e.key === "PageDown" ? cur + 5 : e.key === "PageUp" ? cur - 5 : null;
    if (next === null) return;
    e.preventDefault();
    goTo(Math.max(0, Math.min(pages.length - 1, next)));
  };

  return (
    <Scroll ref={viewport} label={t("thumbs.scrollLabel")}>
      <div
        className="thumbs"
        role="listbox"
        tabIndex={0}
        aria-label={t("thumbs.label")}
        aria-activedescendant={`thumb-${docId}-${doc.currentPage}`}
        style={{ height: v.getTotalSize() }}
        onKeyDown={onKeyDown}
        data-region="left"
        data-testid="thumbs"
      >
        {v.getVirtualItems().map((item) => {
          const p = pages[item.index]!;
          const selected = item.index === doc.currentPage;
          return (
            <div
              key={item.key}
              id={`thumb-${docId}-${item.index}`}
              role="option"
              aria-selected={selected}
              aria-label={t("viewer.pageLabel", { page: item.index + 1, total: pages.length })}
              className="thumb"
              style={{ top: 0, transform: `translateY(${item.start}px)`, height: item.size }}
              onClick={() => goTo(item.index)}
              data-testid="thumb"
            >
              <ThumbImage docId={docId} page={item.index} info={p} revision={doc.info.revision} />
              <span className="thumb-label" aria-hidden="true">
                {item.index + 1}
              </span>
            </div>
          );
        })}
      </div>
    </Scroll>
  );
}

function ThumbImage({ docId, page, info, revision }: { docId: string; page: number; info: PageInfo; revision: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const [ready, setReady] = useState(false);
  const dpr = window.devicePixelRatio || 1;
  const h = thumbHeight(info);
  const scale = (THUMB_W * dpr) / info.width;

  useEffect(() => {
    const cache = getTileCache();
    const rows = Math.ceil((h * dpr) / TILE_SIZE);
    const handles = Array.from({ length: rows }, (_, ty) => cache.acquire({ docId, page, scale, tx: 0, ty, revision }));
    let live = true;
    Promise.all(handles.map((x) => x.promise))
      .then((tiles) => {
        const c = ref.current;
        if (!live || !c) return;
        c.width = Math.round(THUMB_W * dpr);
        c.height = tiles.reduce((a, t) => a + t.height, 0);
        const ctx = c.getContext("2d");
        let y = 0;
        for (const t of tiles) {
          ctx?.drawImage(t.bitmap, 0, y);
          y += t.height;
        }
        setReady(true);
      })
      .catch(() => undefined);
    return () => {
      live = false;
      handles.forEach((x) => x.release());
    };
  }, [docId, page, scale, h, dpr, revision]);

  return <canvas ref={ref} className="thumb-img" aria-hidden="true" data-ready={ready} style={{ width: THUMB_W, height: h }} />;
}
