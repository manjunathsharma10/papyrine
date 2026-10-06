import { create } from "zustand";
import {
  type DocId,
  type DocumentInfo,
  type EngineCommand,
  type OpenSource,
  type OutlineNode,
  HostError,
} from "../ipc/contract";
import { getHost } from "../ipc";
import { type LocaleId, setLocale } from "../i18n";
import { clampZoom, type ViewMode, type ZoomMode } from "../viewer/zoom";
import { readJson, writeJson } from "./storage";

export type ThemePref = "system" | "light" | "dark" | "hc";
export type LeftPane = "thumbnails" | "bookmarks";
export type Banner = "repaired" | "changed-on-disk";

export interface DocState {
  info: DocumentInfo;
  currentPage: number;
  zoomMode: ZoomMode;
  /** Effective zoom: user value in custom mode, computed by the viewer in fit modes. */
  zoom: number;
  viewMode: ViewMode;
  outline: OutlineNode[] | null;
  back: number[];
  forward: number[];
  banners: Banner[];
  /** Bumps whenever pages should re-request tiles. */
  tileEpoch: number;
}

export interface ScrollRequest {
  docId: DocId;
  page: number;
  nonce: number;
}

export interface ActionPrompt {
  docId: DocId;
  kind: "uri" | "launch";
  target: string;
}

export interface SearchState {
  docId: DocId;
  query: string;
  caseSensitive: boolean;
  wholeWord: boolean;
}

export interface Toast {
  id: number;
  kind: "error" | "info";
  /** Catalog key; rendered with `params` so toasts follow the locale. */
  key: string;
  params?: Record<string, string | number>;
}

interface Prefs {
  theme: ThemePref;
  locale: LocaleId;
  pinned: string[];
  leftPane: LeftPane | null;
  leftWidth: number;
  rightWidth: number;
}

const PREFS_KEY = "papyrine.prefs.v1";
const DEFAULT_PREFS: Prefs = { theme: "system", locale: "en", pinned: [], leftPane: "thumbnails", leftWidth: 232, rightWidth: 320 };

export interface AppState extends Prefs {
  docs: Record<DocId, DocState>;
  tabs: DocId[];
  activeId: DocId | null;
  /** Right pane content: a tool id, "all-tools", or null (closed). */
  rightTool: string | null;
  paletteOpen: boolean;
  goToOpen: boolean;
  propsOpen: boolean;
  confirmClose: DocId | null;
  prompt: ActionPrompt | null;
  scrollRequest: ScrollRequest | null;
  search: SearchState | null;
  toasts: Toast[];
  recents: { name: string; path: string }[];
  opening: number;

  // Actions
  openSources(sources: OpenSource[]): Promise<void>;
  openDialog(): Promise<void>;
  closeTab(id: DocId, force?: boolean): Promise<void>;
  activate(id: DocId): void;
  cycleTab(dir: 1 | -1): void;
  goToPage(page: number, opts?: { history?: boolean }): void;
  /** Viewer reports the page under the viewport; does not scroll. */
  reportPage(page: number): void;
  stepPage(delta: number): void;
  historyBack(): void;
  historyForward(): void;
  setZoomMode(mode: ZoomMode): void;
  setZoom(zoom: number): void;
  reportEffectiveZoom(zoom: number): void;
  setViewMode(mode: ViewMode): void;
  setTheme(t: ThemePref): void;
  setLocalePref(l: LocaleId): Promise<void>;
  setLeftPane(p: LeftPane | null): void;
  toggleLeftPane(): void;
  setRightTool(id: string | null): void;
  toggleTool(id: string): void;
  togglePin(id: string): void;
  setPalette(open: boolean): void;
  setGoTo(open: boolean): void;
  setProps(open: boolean): void;
  setPrompt(p: ActionPrompt | null): void;
  setSearch(s: SearchState | null): void;
  loadOutline(id: DocId): Promise<void>;
  dismissBanner(id: DocId, b: Banner): void;
  run(command: EngineCommand): Promise<void>;
  undo(): Promise<void>;
  redo(): Promise<void>;
  save(): Promise<void>;
  applyInfo(info: DocumentInfo, invalidated: number[]): void;
  notify(kind: Toast["kind"], key: string, params?: Toast["params"]): void;
  dismissToast(id: number): void;
  loadRecents(): Promise<void>;
  setWidths(w: Partial<Pick<Prefs, "leftWidth" | "rightWidth">>): void;
}

let toastId = 1;
let scrollNonce = 1;

function errorKey(e: unknown): string {
  return e instanceof HostError ? `error.${e.code}` : "error.internal";
}

function errorDetail(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function persist(s: AppState): void {
  const { theme, locale, pinned, leftPane, leftWidth, rightWidth } = s;
  writeJson(PREFS_KEY, { theme, locale, pinned, leftPane, leftWidth, rightWidth } satisfies Prefs);
}

export function applyTheme(t: ThemePref): void {
  const root = document.documentElement;
  if (t === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
}

const initialPrefs: Prefs = { ...DEFAULT_PREFS, ...readJson<Partial<Prefs>>(PREFS_KEY, {}) };

function patchDoc(s: AppState, id: DocId, patch: Partial<DocState>): Pick<AppState, "docs"> {
  const d = s.docs[id];
  if (!d) return { docs: s.docs };
  return { docs: { ...s.docs, [id]: { ...d, ...patch } } };
}

export const useApp = create<AppState>()((set, get) => ({
  ...initialPrefs,
  docs: {},
  tabs: [],
  activeId: null,
  rightTool: null,
  paletteOpen: false,
  goToOpen: false,
  propsOpen: false,
  confirmClose: null,
  prompt: null,
  scrollRequest: null,
  search: null,
  toasts: [],
  recents: [],
  opening: 0,

  async openSources(sources) {
    const host = getHost();
    for (const source of sources) {
      set((s) => ({ opening: s.opening + 1 }));
      try {
        const info = await host.openDocument(source);
        const doc: DocState = {
          info,
          currentPage: 0,
          zoomMode: "fit-width",
          zoom: 1,
          viewMode: "continuous",
          outline: null,
          back: [],
          forward: [],
          banners: info.repaired ? ["repaired"] : [],
          tileEpoch: 0,
        };
        set((s) => ({ docs: { ...s.docs, [info.docId]: doc }, tabs: [...s.tabs, info.docId], activeId: info.docId }));
        void get().loadOutline(info.docId);
      } catch (e) {
        get().notify("error", errorKey(e), { detail: errorDetail(e) });
      } finally {
        set((s) => ({ opening: s.opening - 1 }));
      }
    }
  },

  async openDialog() {
    try {
      const { sources } = await getHost().showOpenDialog();
      if (sources.length) await get().openSources(sources);
    } catch (e) {
      get().notify("error", errorKey(e), { detail: errorDetail(e) });
    }
  },

  async closeTab(id, force = false) {
    const s = get();
    const d = s.docs[id];
    if (!d) return;
    if (d.info.dirty && !force) {
      set({ confirmClose: id });
      return;
    }
    const idx = s.tabs.indexOf(id);
    const tabs = s.tabs.filter((t) => t !== id);
    const { [id]: _gone, ...docs } = s.docs;
    const activeId = s.activeId === id ? (tabs[Math.min(idx, tabs.length - 1)] ?? null) : s.activeId;
    set({ docs, tabs, activeId, confirmClose: null, search: s.search?.docId === id ? null : s.search });
    await getHost().closeDocument(id).catch(() => undefined);
  },

  activate(id) {
    if (get().docs[id]) set({ activeId: id });
  },

  cycleTab(dir) {
    const { tabs, activeId } = get();
    if (tabs.length < 2 || !activeId) return;
    const i = tabs.indexOf(activeId);
    const next = tabs[(i + dir + tabs.length) % tabs.length];
    if (next) set({ activeId: next });
  },

  goToPage(page, opts) {
    const s = get();
    const id = s.activeId;
    const d = id ? s.docs[id] : undefined;
    if (!id || !d) return;
    const target = Math.max(0, Math.min(d.info.pageCount - 1, Math.floor(page)));
    const pushHistory = opts?.history !== false && target !== d.currentPage;
    set((st) => ({
      ...patchDoc(st, id, {
        currentPage: target,
        back: pushHistory ? [...d.back, d.currentPage].slice(-100) : d.back,
        forward: pushHistory ? [] : d.forward,
      }),
      scrollRequest: { docId: id, page: target, nonce: scrollNonce++ },
    }));
  },

  reportPage(page) {
    const s = get();
    const id = s.activeId;
    const d = id ? s.docs[id] : undefined;
    if (!id || !d || d.currentPage === page) return;
    set((st) => patchDoc(st, id, { currentPage: page }));
  },

  stepPage(delta) {
    const d = activeDoc(get());
    if (d) get().goToPage(d.currentPage + delta);
  },

  historyBack() {
    const s = get();
    const id = s.activeId;
    const d = activeDoc(s);
    if (!id || !d || d.back.length === 0) return;
    const prev = d.back[d.back.length - 1] as number;
    set((st) => ({
      ...patchDoc(st, id, { back: d.back.slice(0, -1), forward: [...d.forward, d.currentPage], currentPage: prev }),
      scrollRequest: { docId: id, page: prev, nonce: scrollNonce++ },
    }));
  },

  historyForward() {
    const s = get();
    const id = s.activeId;
    const d = activeDoc(s);
    if (!id || !d || d.forward.length === 0) return;
    const next = d.forward[d.forward.length - 1] as number;
    set((st) => ({
      ...patchDoc(st, id, { forward: d.forward.slice(0, -1), back: [...d.back, d.currentPage], currentPage: next }),
      scrollRequest: { docId: id, page: next, nonce: scrollNonce++ },
    }));
  },

  setZoomMode(mode) {
    const id = get().activeId;
    if (id) set((s) => patchDoc(s, id, { zoomMode: mode, ...(mode === "actual" ? { zoom: 1 } : {}) }));
  },

  setZoom(zoom) {
    const id = get().activeId;
    if (id) set((s) => patchDoc(s, id, { zoomMode: "custom", zoom: clampZoom(zoom) }));
  },

  reportEffectiveZoom(zoom) {
    const id = get().activeId;
    const d = activeDoc(get());
    if (id && d && Math.abs(d.zoom - zoom) > 1e-9) set((s) => patchDoc(s, id, { zoom }));
  },

  setViewMode(mode) {
    const id = get().activeId;
    if (id) set((s) => patchDoc(s, id, { viewMode: mode }));
  },

  setTheme(theme) {
    applyTheme(theme);
    set({ theme });
    persist(get());
  },

  async setLocalePref(locale) {
    await setLocale(locale);
    set({ locale });
    persist(get());
  },

  setLeftPane(p) {
    set({ leftPane: p });
    persist(get());
  },

  toggleLeftPane() {
    set((s) => ({ leftPane: s.leftPane ? null : "thumbnails" }));
    persist(get());
  },

  setRightTool(id) {
    set({ rightTool: id });
  },

  toggleTool(id) {
    set((s) => ({ rightTool: s.rightTool === id ? null : id }));
  },

  togglePin(id) {
    set((s) => ({ pinned: s.pinned.includes(id) ? s.pinned.filter((p) => p !== id) : [...s.pinned, id] }));
    persist(get());
  },

  setPalette: (open) => set({ paletteOpen: open }),
  setGoTo: (open) => set({ goToOpen: open }),
  setProps: (open) => set({ propsOpen: open }),
  setPrompt: (prompt) => set({ prompt }),
  setSearch: (search) => set({ search }),

  async loadOutline(id) {
    try {
      const outline = await getHost().getOutline(id);
      set((s) => patchDoc(s, id, { outline }));
    } catch {
      set((s) => patchDoc(s, id, { outline: [] }));
    }
  },

  dismissBanner(id, b) {
    const d = get().docs[id];
    if (d) set((s) => patchDoc(s, id, { banners: d.banners.filter((x) => x !== b) }));
  },

  async run(command) {
    const id = get().activeId;
    if (!id) return;
    try {
      const res = await getHost().execute(id, command);
      get().applyInfo(res.info, res.invalidatedPages);
    } catch (e) {
      get().notify("error", errorKey(e), { detail: errorDetail(e) });
    }
  },

  async undo() {
    const id = get().activeId;
    if (!id) return;
    const res = await getHost().undo(id);
    get().applyInfo(res.info, res.invalidatedPages);
  },

  async redo() {
    const id = get().activeId;
    if (!id) return;
    const res = await getHost().redo(id);
    get().applyInfo(res.info, res.invalidatedPages);
  },

  async save() {
    const id = get().activeId;
    if (!id) return;
    try {
      const rep = await getHost().save(id);
      if (rep.status === "saved") {
        get().applyInfo(rep.info, []);
        get().notify("info", "toast.saved");
      }
    } catch (e) {
      get().notify("error", errorKey(e), { detail: errorDetail(e) });
    }
  },

  applyInfo(info, invalidated) {
    set((s) => {
      const d = s.docs[info.docId];
      if (!d) return {};
      const currentPage = Math.min(d.currentPage, info.pageCount - 1);
      return patchDoc(s, info.docId, {
        info,
        currentPage: Math.max(0, currentPage),
        tileEpoch: invalidated.length ? d.tileEpoch + 1 : d.tileEpoch,
      });
    });
  },

  notify(kind, key, params) {
    const id = toastId++;
    set((s) => ({ toasts: [...s.toasts, { id, kind, key, params }].slice(-4) }));
    setTimeout(() => get().dismissToast(id), 6000);
  },

  dismissToast(id) {
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) }));
  },

  async loadRecents() {
    try {
      set({ recents: await getHost().recentFiles() });
    } catch {
      set({ recents: [] });
    }
  },

  setWidths(w) {
    set(w);
    persist(get());
  },
}));

export function activeDoc(s: AppState): DocState | undefined {
  return s.activeId ? s.docs[s.activeId] : undefined;
}

/** Hook-friendly selector. */
export const useActiveDoc = (): DocState | undefined => useApp((s) => (s.activeId ? s.docs[s.activeId] : undefined));
