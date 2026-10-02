import { useVirtualizer } from "@tanstack/react-virtual";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useApp, type DocState } from "../store/app";
import { PageView } from "./PageView";
import { type ViewportRect, ViewportContext, type ViewportSource } from "./viewport";
import {
  CSS_PX_PER_PT,
  PAGE_GAP,
  VIEW_PAD,
  clampZoom,
  fitPageZoom,
  fitWidthZoom,
  layoutPages,
  wheelZoom,
} from "./zoom";

/** Effective zoom for the doc's mode given the container size. */
export function effectiveZoom(doc: DocState, w: number, h: number): number {
  if (doc.zoomMode === "custom" || doc.zoomMode === "actual" || w === 0) return doc.zoom;
  const pages = doc.info.pages;
  if (doc.zoomMode === "fit-width") {
    const widest = doc.viewMode === "single" ? pages[doc.currentPage] : pages.reduce<(typeof pages)[number] | undefined>((a, b) => (!a || b.width > a.width ? b : a), undefined);
    return widest ? fitWidthZoom(w, widest.width) : 1;
  }
  if (doc.viewMode === "single") {
    const p = pages[doc.currentPage];
    return p ? fitPageZoom(w, h, p) : 1;
  }
  return pages.length ? Math.min(...pages.map((p) => fitPageZoom(w, h, p))) : 1;
}

interface Anchor {
  x: number;
  y: number;
}

export function Viewer({ doc, docId }: { doc: DocState; docId: string }) {
  const { t } = useTranslation();
  const scroller = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const search = useApp((s) => s.search);

  const single = doc.viewMode === "single";
  const itemIndexes = useMemo(
    () => (single ? [Math.min(doc.currentPage, doc.info.pageCount - 1)] : Array.from({ length: doc.info.pageCount }, (_, i) => i)),
    [single, doc.currentPage, doc.info.pageCount],
  );
  const zoom = effectiveZoom(doc, size.w, size.h);
  const layout = useMemo(
    () => layoutPages(itemIndexes.map((i) => doc.info.pages[i]!), zoom, size.w),
    [itemIndexes, doc.info.pages, zoom, size.w],
  );

  // Fit modes: publish the computed zoom so the status bar and zoom commands see it.
  useLayoutEffect(() => {
    useApp.getState().reportEffectiveZoom(zoom);
  }, [zoom]);

  // A freshly opened document takes keyboard focus unless the user is already somewhere deliberate.
  useEffect(() => {
    const a = document.activeElement;
    if (!a || a === document.body) scroller.current?.focus({ preventScroll: true });
  }, []);

  // Container size.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const apply = () => setSize((s) => (s.w === el.clientWidth && s.h === el.clientHeight ? s : { w: el.clientWidth, h: el.clientHeight }));
    apply();
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Viewport publisher for pages (they pick their own visible tiles).
  const vpRect = useRef<ViewportRect>({ top: 0, left: 0, width: 0, height: 0 });
  const listeners = useRef(new Set<() => void>());
  const notifyRaf = useRef(0);
  const viewport = useMemo<ViewportSource>(
    () => ({
      get: () => vpRect.current,
      subscribe: (cb) => {
        listeners.current.add(cb);
        return () => listeners.current.delete(cb);
      },
    }),
    [],
  );
  const notify = useCallback(() => {
    cancelAnimationFrame(notifyRaf.current);
    notifyRaf.current = requestAnimationFrame(() => listeners.current.forEach((cb) => cb()));
  }, []);
  // Keep the rect fresh synchronously for new pages, and tell old ones.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    vpRect.current = { top: el.scrollTop, left: el.scrollLeft, width: size.w, height: size.h };
    listeners.current.forEach((cb) => cb());
  }, [size.w, size.h, zoom, itemIndexes]);

  const virtualizer = useVirtualizer({
    count: itemIndexes.length,
    getScrollElement: () => scroller.current,
    estimateSize: (i) => (layout.heights[i] ?? 0) + PAGE_GAP,
    overscan: 1,
    paddingStart: VIEW_PAD,
    paddingEnd: VIEW_PAD - PAGE_GAP,
    getItemKey: (i) => `${docId}:${itemIndexes[i]}`,
  });
  useLayoutEffect(() => {
    virtualizer.measure();
  }, [virtualizer, layout]);

  // Scroll bookkeeping.
  const lastScroll = useRef({ top: 0, left: 0 });
  /** Where our own scrolling landed; the matching scroll event must not change the current page. */
  const programmaticTarget = useRef<{ top: number; left: number } | null>(null);
  const prevLayout = useRef({ zoom, layout, items: itemIndexes });
  const anchor = useRef<Anchor | null>(null);

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    lastScroll.current = { top: el.scrollTop, left: el.scrollLeft };
    vpRect.current = { top: el.scrollTop, left: el.scrollLeft, width: el.clientWidth, height: el.clientHeight };
    notify();
    const target = programmaticTarget.current;
    programmaticTarget.current = null;
    if (target && Math.abs(el.scrollTop - target.top) <= 1 && Math.abs(el.scrollLeft - target.left) <= 1) return;
    if (single) return;
    // Current page: the one under the upper third of the viewport.
    const probe = el.scrollTop + el.clientHeight * 0.33;
    let idx = 0;
    for (let i = 0; i < layout.tops.length; i++) {
      if ((layout.tops[i] as number) <= probe) idx = i;
      else break;
    }
    const page = itemIndexes[idx];
    if (page !== undefined) useApp.getState().reportPage(page);
  }, [layout, itemIndexes, notify, single]);

  // Keep the point under the cursor (or viewport centre) fixed across zoom changes.
  useLayoutEffect(() => {
    const el = scroller.current;
    const prev = prevLayout.current;
    if (el && prev.zoom !== zoom && prev.items.length === itemIndexes.length && prev.items[0] === itemIndexes[0]) {
      const a = anchor.current ?? { x: el.clientWidth / 2, y: el.clientHeight / 2 };
      const contentY = lastScroll.current.top + a.y;
      const contentX = lastScroll.current.left + a.x;
      let idx = 0;
      for (let i = 0; i < prev.layout.tops.length; i++) if ((prev.layout.tops[i] as number) <= contentY) idx = i;
      const pt = prev.layout.tops[idx] as number;
      const ph = prev.layout.heights[idx] as number;
      const pw = prev.layout.widths[idx] as number;
      const fy = ph > 0 ? (contentY - pt) / ph : 0;
      const pLeft = (prev.layout.totalWidth - pw) / 2;
      const fx = pw > 0 ? (contentX - pLeft) / pw : 0.5;
      const nt = layout.tops[idx] as number;
      const nh = layout.heights[idx] as number;
      const nw = layout.widths[idx] as number;
      const nLeft = (layout.totalWidth - nw) / 2;
      el.scrollTop = nt + fy * nh - a.y;
      el.scrollLeft = nLeft + fx * nw - a.x;
      lastScroll.current = { top: el.scrollTop, left: el.scrollLeft };
      vpRect.current = { top: el.scrollTop, left: el.scrollLeft, width: el.clientWidth, height: el.clientHeight };
      listeners.current.forEach((cb) => cb());
    }
    anchor.current = null;
    prevLayout.current = { zoom, layout, items: itemIndexes };
  }, [zoom, layout, itemIndexes]);

  // Go-to-page requests, and restoring position when the tab is shown again.
  const handled = useRef(0);
  const scrollTo = useCallback(
    (page: number) => {
      const el = scroller.current;
      if (!el) return;
      const top = single ? 0 : Math.max(0, (layout.tops[page] ?? 0) - VIEW_PAD / 2);
      el.scrollTop = top;
      // Read back: the browser clamps to the scrollable range.
      programmaticTarget.current = { top: el.scrollTop, left: el.scrollLeft };
    },
    [layout, single],
  );
  const request = useApp((s) => s.scrollRequest);
  useLayoutEffect(() => {
    if (request && request.docId === docId && request.nonce !== handled.current) {
      handled.current = request.nonce;
      scrollTo(request.page);
    }
  }, [request, docId, scrollTo]);
  const restored = useRef(false);
  useLayoutEffect(() => {
    if (!restored.current && size.w > 0) {
      restored.current = true;
      scrollTo(doc.currentPage);
    }
  }, [size.w, scrollTo, doc.currentPage]);

  // Zoom gestures: Ctrl/Cmd-wheel, trackpad pinch (wheel+ctrl in Chromium, gesture* in WebKit), touch pinch.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const zoomAt = (next: number, clientX: number, clientY: number) => {
      const r = el.getBoundingClientRect();
      anchor.current = { x: clientX - r.left, y: clientY - r.top };
      useApp.getState().setZoom(next);
    };
    const current = () => {
      const d = useApp.getState().docs[docId];
      return d ? effectiveZoom(d, el.clientWidth, el.clientHeight) : 1;
    };
    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      zoomAt(wheelZoom(current(), e.deltaY, e.deltaMode), e.clientX, e.clientY);
    };
    let gestureBase = 1;
    const onGestureStart = (e: Event) => {
      e.preventDefault();
      gestureBase = current();
    };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as Event & { scale: number; clientX: number; clientY: number };
      zoomAt(clampZoom(gestureBase * g.scale), g.clientX, g.clientY);
    };
    const pointers = new Map<number, { x: number; y: number }>();
    let pinchStart: { dist: number; zoom: number } | null = null;
    const dist = () => {
      const [a, b] = [...pointers.values()];
      return a && b ? Math.hypot(a.x - b.x, a.y - b.y) : 0;
    };
    const onDown = (e: PointerEvent) => {
      if (e.pointerType !== "touch") return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pointers.size === 2) pinchStart = { dist: dist(), zoom: current() };
    };
    const onMove = (e: PointerEvent) => {
      if (!pointers.has(e.pointerId)) return;
      pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (pointers.size === 2 && pinchStart && pinchStart.dist > 0) {
        const [a, b] = [...pointers.values()] as [{ x: number; y: number }, { x: number; y: number }];
        zoomAt(clampZoom(pinchStart.zoom * (dist() / pinchStart.dist)), (a.x + b.x) / 2, (a.y + b.y) / 2);
      }
    };
    const onUp = (e: PointerEvent) => {
      pointers.delete(e.pointerId);
      if (pointers.size < 2) pinchStart = null;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("pointerdown", onDown);
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("pointercancel", onUp);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("pointerdown", onDown);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
      el.removeEventListener("pointercancel", onUp);
    };
  }, [docId]);

  // Page-level keys. Arrow keys and PageUp/Down scroll natively in continuous mode.
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const s = useApp.getState();
    const el = scroller.current;
    if (!el) return;
    const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 2;
    const atTop = el.scrollTop <= 0;
    switch (e.key) {
      case "Home":
        e.preventDefault();
        s.goToPage(0);
        break;
      case "End":
        e.preventDefault();
        s.goToPage(Number.MAX_SAFE_INTEGER);
        break;
      case "PageDown":
      case " ":
        if (single && (atBottom || e.key === "PageDown")) {
          e.preventDefault();
          if (e.shiftKey && e.key === " ") s.stepPage(-1);
          else s.stepPage(1);
        }
        break;
      case "PageUp":
        if (single) {
          e.preventDefault();
          s.stepPage(-1);
        }
        break;
      case "ArrowDown":
        if (single && atBottom) {
          e.preventDefault();
          s.stepPage(1);
        }
        break;
      case "ArrowUp":
        if (single && atTop) {
          e.preventDefault();
          s.stepPage(-1);
        }
        break;
      case "ArrowRight":
        if (single && el.scrollLeft + el.clientWidth >= el.scrollWidth - 2) {
          e.preventDefault();
          s.stepPage(1);
        }
        break;
      case "ArrowLeft":
        if (single && el.scrollLeft <= 0) {
          e.preventDefault();
          s.stepPage(-1);
        }
        break;
    }
  };

  const items = virtualizer.getVirtualItems();
  return (
    <ViewportContext.Provider value={viewport}>
      <div
        ref={scroller}
        className="viewer"
        dir="ltr"
        tabIndex={0}
        role="region"
        aria-label={t("viewer.regionLabel", { name: doc.info.name })}
        data-region="viewer"
        data-testid="viewer"
        data-zoom={zoom.toFixed(4)}
        data-css-per-pt={CSS_PX_PER_PT}
        onScroll={onScroll}
        onKeyDown={onKeyDown}
      >
        <div className="viewer-content" style={{ height: layout.totalHeight, width: layout.totalWidth }}>
          {items.map((v) => {
            const pageIdx = itemIndexes[v.index] as number;
            const info = doc.info.pages[pageIdx]!;
            const w = layout.widths[v.index] as number;
            return (
              <PageView
                key={v.key}
                docId={docId}
                index={pageIdx}
                total={doc.info.pageCount}
                info={info}
                zoom={zoom}
                dpr={dpr}
                top={layout.tops[v.index] as number}
                left={(layout.totalWidth - w) / 2}
                width={w}
                height={layout.heights[v.index] as number}
                revision={doc.info.revision}
                search={search}
              />
            );
          })}
        </div>
      </div>
    </ViewportContext.Provider>
  );
}
