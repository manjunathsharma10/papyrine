import {
  type Annot,
  type AnnotProps,
  type CommandResult,
  type DocId,
  type DocumentInfo,
  type DocumentMeta,
  type EngineCommand,
  type FieldValueResult,
  type FileDialogResult,
  type FileOp,
  type FileOpResult,
  type FormKind,
  type FormWidget,
  type HostApi,
  type HostEvent,
  type HostEventType,
  HostError,
  CONTRACT_VERSION,
  type JobId,
  type KeystrokeRequest,
  type KeystrokeResult,
  type OpenSource,
  type OutlineNode,
  type PageInfo,
  type PageText,
  type PickedFile,
  type Point,
  type PrintOptions,
  type PrintSetup,
  type PrivacySettings,
  type Quad,
  type Rect,
  type RecoveryAction,
  type RecoveryEntry,
  type SaveOptions,
  type SaveReport,
  type SearchHit,
  type SearchOptions,
  type SecurityUpdate,
  type Tile,
  type TileRequest,
  type UnfinishedAction,
  type Unsubscribe,
} from "./contract";
import { buildWidgets, initialValues, mockFormat, mockKeystroke, templateOf, widgetDefs } from "./mockForms";
import { LINE_STEP, LINE_X, LINE_Y0, FONT_PT, mockLines, mockPageText, renderMockTile } from "./mockPaint";

/**
 * In-memory host used by Vite dev and E2E. Documents are synthetic: a
 * mock path looks like `mock://name?pages=24&forms=1&signed=1`. Pages are
 * drawn on canvas so the viewer, text layer, annotations, form widgets and
 * search can be developed and tested without the Rust host.
 *
 * Query flags: `pages=N`, `forms=1`, `signed=1`, `xfa=static|dynamic`,
 * `scripts=1`, `encrypted=1`; a path containing `repaired` marks the file
 * as repaired. Test hooks (`emitEvent`, `pickQueue`, ...) are public.
 */

interface MockPage {
  /** Stable identity: survives reorder, never reused. */
  uid: number;
  /** Source page deciding the drawn content; negative = blank. */
  src: number;
  width: number;
  height: number;
  rotation: 0 | 90 | 180 | 270;
  /** Suffix that makes form field names unique on duplicated pages. */
  suffix: string;
}

interface MockAnnot {
  pageUid: number;
  a: Annot;
}

interface Snapshot {
  meta: DocumentMeta;
  pages: MockPage[];
  annots: MockAnnot[];
  values: Record<string, string>;
}

interface MockDoc {
  id: DocId;
  name: string;
  path: string;
  meta: DocumentMeta;
  pages: MockPage[];
  annots: MockAnnot[];
  values: Record<string, string>;
  revision: number;
  dirty: boolean;
  undo: Snapshot[];
  redo: Snapshot[];
  signed: boolean;
  repaired: boolean;
  forms: boolean;
  formKind: FormKind;
  formScripts: boolean;
  showAnnots: boolean;
  saves: number;
  origin: Snapshot;
}

function srcSize(src: number): { width: number; height: number } {
  if (src < 0) return { width: 612, height: 792 };
  if (src % 7 === 6) return { width: 792, height: 612 }; // landscape letter
  if (src % 5 === 4) return { width: 595, height: 842 }; // A4
  return { width: 612, height: 792 };
}

const DEFAULTS: Record<string, Partial<AnnotProps>> = {
  highlight: { color: "#ffd54a", opacity: 0.4 },
  underline: { color: "#d32f2f", opacity: 1, width: 1 },
  strikeout: { color: "#d32f2f", opacity: 1, width: 1 },
  squiggly: { color: "#d32f2f", opacity: 1, width: 1 },
  note: { color: "#ffe066", opacity: 1 },
  textbox: { color: "#1b2a4a", opacity: 1, width: 1, fill: null },
  ink: { color: "#1565c0", opacity: 1, width: 2 },
  rectangle: { color: "#d32f2f", opacity: 1, width: 2, fill: null },
  oval: { color: "#d32f2f", opacity: 1, width: 2, fill: null },
  line: { color: "#d32f2f", opacity: 1, width: 2 },
  arrow: { color: "#d32f2f", opacity: 1, width: 2 },
};

const fold = (c: string) => c.normalize("NFD").replace(/\p{M}/gu, "");

function boundsOf(points: Point[]): Rect {
  const xs = points.map((p) => p.x);
  const ys = points.map((p) => p.y);
  const x = Math.min(...xs);
  const y = Math.min(...ys);
  return { x, y, width: Math.max(...xs) - x, height: Math.max(...ys) - y };
}

function quadPoints(q: Quad): Point[] {
  return [
    { x: q[0], y: q[1] },
    { x: q[2], y: q[3] },
    { x: q[4], y: q[5] },
    { x: q[6], y: q[7] },
  ];
}

function rectQuad(r: Rect): Quad {
  return [r.x, r.y, r.x + r.width, r.y, r.x + r.width, r.y + r.height, r.x, r.y + r.height];
}

function pageCountOf(path: string): number {
  const m = /[?&]pages=(\d+)/.exec(path);
  return m ? Number(m[1]) : 3;
}

function nameOf(path: string): string {
  const m = /^mock:\/\/([^?]*)/.exec(path);
  return m?.[1] ?? path.split(/[\\/]/).pop() ?? path;
}

function readLs(key: string): string | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeLs(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore */
  }
}

export class MockHost implements HostApi {
  readonly contractVersion = CONTRACT_VERSION;
  private docs = new Map<DocId, MockDoc>();
  private listeners = new Map<HostEventType, Set<(e: never) => void>>();
  private jobs = new Map<JobId, { cancelled: boolean }>();
  private nextDoc = 1;
  private nextJob = 1;
  private nextUid = 1;
  private nextAnnot = 1;
  private sampleCounter = 0;
  /** Artificial tile latency in ms (tests can tune it). */
  tileDelayMs = 4;

  // Test hooks and observable side effects.
  /** Results for upcoming `pickPdfs` calls (first in, first out). */
  pickQueue: PickedFile[][] = [];
  /** Next `pickSavePath` answer; null simulates cancel. */
  nextSavePath: string | null | undefined;
  saveLog: { docId: DocId; options: SaveOptions | undefined }[] = [];
  fileOpLog: FileOp[] = [];
  printLog: { docId: DocId; options: PrintOptions }[] = [];
  promptLog: { promptId: string; allow: boolean }[] = [];
  unfinishedLog: { id: string; action: UnfinishedAction }[] = [];
  keepMineLog: DocId[] = [];
  /** Update returned by `checkForUpdates`. */
  nextUpdate: SecurityUpdate | null = null;
  updateChecks = 0;
  suggestDisabled = false;
  recoveryList: RecoveryEntry[] = [];
  private recent: { name: string; path: string }[] = [
    { name: "annual-report.pdf", path: "mock://annual-report.pdf?pages=40" },
    { name: "invoice.pdf", path: "mock://invoice.pdf?pages=2" },
  ];
  private privacy: PrivacySettings = { updateCheck: "unset", lockedByPolicy: false };

  constructor() {
    const p = readLs("papyrine.mock.privacy");
    if (p === "on" || p === "off" || p === "unset") this.privacy.updateCheck = p;
    if (readLs("papyrine.mock.privacyLocked") === "1") this.privacy.lockedByPolicy = true;
    const rec = readLs("papyrine.mock.recovery");
    if (rec) {
      try {
        this.recoveryList = JSON.parse(rec) as RecoveryEntry[];
      } catch {
        /* ignore */
      }
    }
  }

  async showOpenDialog(): Promise<FileDialogResult> {
    const n = ++this.sampleCounter;
    const pages = n === 1 ? 24 : 8 + n;
    return { sources: [{ kind: "path", path: `mock://sample-${n}.pdf?pages=${pages}` }] };
  }

  async openDocument(source: OpenSource): Promise<DocumentInfo> {
    let name: string;
    let path: string;
    if (source.kind === "path") {
      path = source.path;
      name = nameOf(path);
      if (path.includes("missing")) throw new HostError("not-found", "No such file");
    } else {
      name = source.name;
      path = `mock://${source.name}`;
    }
    const pageCount = source.kind === "path" && /[?&]pages=/.test(path) ? pageCountOf(path) : 12;
    const q = new URLSearchParams(path.split("?")[1] ?? "");
    const id = `doc-${this.nextDoc++}`;
    const pages: MockPage[] = Array.from({ length: pageCount }, (_, i) => ({
      uid: this.nextUid++,
      src: i,
      ...srcSize(i),
      rotation: 0,
      suffix: "",
    }));
    const xfa = q.get("xfa");
    const forms = q.get("forms") === "1" || xfa !== null;
    const meta: DocumentMeta = {
      title: name.replace(/\.pdf$/i, ""),
      author: "Papyrine Mock",
      subject: "",
      keywords: "",
      producer: "mock-host",
      pdfVersion: "1.7",
      encrypted: q.get("encrypted") === "1",
      creator: "Mock Writer",
      created: "2026-01-15T09:30:00Z",
      modified: "2026-03-02T16:45:00Z",
      fileSize: 48_231 + pageCount * 1024,
    };
    const doc: MockDoc = {
      id,
      name,
      path,
      meta,
      pages,
      annots: [],
      values: forms ? initialValues("") : {},
      revision: 0,
      dirty: false,
      undo: [],
      redo: [],
      signed: q.get("signed") === "1",
      repaired: path.includes("repaired"),
      forms,
      formKind: xfa === "dynamic" ? "xfa-dynamic" : xfa === "static" ? "xfa-static" : forms ? "acroform" : "none",
      formScripts: q.get("scripts") === "1",
      showAnnots: true,
      saves: 0,
      origin: { meta, pages, annots: [], values: {} },
    };
    doc.origin = this.snapshot(doc);
    this.docs.set(id, doc);
    if (source.kind === "path" && !path.includes("insert-")) {
      this.recent = [{ name, path }, ...this.recent.filter((r) => r.path !== path)].slice(0, 10);
    }
    return this.info(doc);
  }

  async closeDocument(docId: DocId): Promise<void> {
    this.docs.delete(docId);
  }

  async recentFiles() {
    return [...this.recent];
  }

  async clearRecentFiles(): Promise<void> {
    this.recent = [];
  }

  async pickPdfs(options: { multiple: boolean }): Promise<PickedFile[]> {
    const queued = this.pickQueue.shift();
    if (queued) return queued;
    const a: PickedFile = { path: "mock://insert-a.pdf?pages=3", name: "insert-a.pdf", pageCount: 3, size: 12_345 };
    const b: PickedFile = { path: "mock://insert-b.pdf?pages=2", name: "insert-b.pdf", pageCount: 2, size: 9_876 };
    return options.multiple ? [a, b] : [a];
  }

  async pickSavePath(options: { suggestedName: string; directory?: boolean }): Promise<string | null> {
    if (this.nextSavePath !== undefined) {
      const v = this.nextSavePath;
      this.nextSavePath = undefined;
      return v;
    }
    return options.directory ? "/mock/out" : `/mock/out/${options.suggestedName}`;
  }

  async reloadFromDisk(docId: DocId): Promise<DocumentInfo> {
    const doc = this.get(docId);
    this.restore(doc, doc.origin);
    doc.undo = [];
    doc.redo = [];
    doc.revision++;
    doc.dirty = false;
    const info = this.info(doc);
    this.emit({ type: "document-changed", info, invalidatedPages: doc.pages.map((_, i) => i) });
    return info;
  }

  async keepMine(docId: DocId): Promise<void> {
    this.keepMineLog.push(docId);
  }

  async resolveActionPrompt(_docId: DocId, promptId: string, allow: boolean): Promise<void> {
    this.promptLog.push({ promptId, allow });
  }

  async getPageInfo(docId: DocId, page: number): Promise<PageInfo> {
    return this.pageInfo(this.get(docId).pages[page] ?? this.fail(page));
  }

  async getOutline(docId: DocId): Promise<OutlineNode[]> {
    const doc = this.get(docId);
    const n = doc.pages.length;
    const chapters: OutlineNode[] = [];
    for (let start = 0, c = 1; start < n; start += 5, c++) {
      chapters.push({
        title: `Chapter ${c}`,
        page: start,
        children: [start + 1, start + 3]
          .filter((p) => p < n)
          .map((p, i) => ({ title: `Section ${c}.${i + 1}`, page: p, children: [] })),
      });
    }
    return chapters;
  }

  async getTile(req: TileRequest, signal?: AbortSignal): Promise<Tile> {
    const doc = this.get(req.docId);
    const page = doc.pages[req.page] ?? this.fail(req.page);
    await new Promise<void>((resolve, reject) => {
      const t = setTimeout(resolve, this.tileDelayMs);
      signal?.addEventListener("abort", () => {
        clearTimeout(t);
        reject(new HostError("cancelled", "tile cancelled"));
      });
    });
    const annots = doc.showAnnots ? doc.annots.filter((m) => m.pageUid === page.uid && !m.a.replyTo).map((m) => ({ ...m.a, page: req.page })) : [];
    const widgets = doc.forms ? this.widgetsOf(doc, req.page) : [];
    return renderMockTile(this.pageInfo(page), page.src, req.page, req.scale, req.tx, req.ty, { annots, widgets });
  }

  async getPageText(docId: DocId, page: number): Promise<PageText> {
    const p = this.get(docId).pages[page] ?? this.fail(page);
    return p.src < 0 ? { page, runs: [] } : mockPageText(p.src, page);
  }

  async search(docId: DocId, query: string, options: SearchOptions): Promise<JobId> {
    const doc = this.get(docId);
    const jobId = `job-${this.nextJob++}`;
    const job = { cancelled: false };
    this.jobs.set(jobId, job);
    const norm = (s: string) => {
      let out = options.diacriticInsensitive ? [...s].map(fold).join("") : s;
      if (!options.caseSensitive) out = out.toLowerCase();
      return out;
    };
    const q = norm(query);
    const total = doc.pages.length;
    const boundary = (c: string | undefined) => !c || !/[\p{L}\p{N}_]/u.test(c);
    const find = (text: string): { at: number }[] => {
      const hay = norm(text);
      const out: { at: number }[] = [];
      let from = 0;
      for (;;) {
        const at = q ? hay.indexOf(q, from) : -1;
        if (at < 0) break;
        from = at + q.length;
        if (options.wholeWord && !(boundary(hay[at - 1]) && boundary(hay[at + q.length]))) continue;
        out.push({ at });
      }
      return out;
    };
    let page = 0;
    const step = () => {
      if (job.cancelled || !this.docs.has(docId)) return;
      const p = doc.pages[page];
      const hits: SearchHit[] = [];
      if (p && p.src >= 0) {
        mockLines(p.src).forEach((line, i) => {
          for (const { at } of find(line)) {
            const x = LINE_X + at * FONT_PT * 0.5;
            const y = LINE_Y0 + i * LINE_STEP;
            const r: Rect = { x, y, width: q.length * FONT_PT * 0.5, height: FONT_PT + 2 };
            hits.push({ page, snippet: line, matchStart: at, matchLength: q.length, source: "text", quads: [rectQuad(r)] });
          }
        });
      }
      if (p && options.includeComments) {
        for (const m of doc.annots) {
          if (m.pageUid !== p.uid || !m.a.props.contents) continue;
          for (const { at } of find(m.a.props.contents)) {
            hits.push({ page, snippet: m.a.props.contents, matchStart: at, matchLength: q.length, source: "comment", quads: [rectQuad(m.a.rect)] });
          }
        }
      }
      if (p && options.includeFormValues && doc.forms) {
        for (const w of this.widgetsOf(doc, page)) {
          if (w.kind !== "text" && w.kind !== "combo") continue;
          const text = w.display;
          if (!text || w.password) continue;
          for (const { at } of find(text)) {
            hits.push({ page, snippet: text, matchStart: at, matchLength: q.length, source: "form", quads: [rectQuad(w.rect)] });
          }
        }
      }
      for (const hit of hits) this.emit({ type: "search-hit", jobId, hit });
      page++;
      const finished = page >= total;
      this.emit({ type: "job-progress", jobId, done: page, total, finished });
      if (!finished) setTimeout(step, 0);
    };
    setTimeout(step, 0);
    return jobId;
  }

  async cancelJob(jobId: JobId): Promise<void> {
    const job = this.jobs.get(jobId);
    if (job) job.cancelled = true;
  }

  async listAnnotations(docId: DocId): Promise<Annot[]> {
    const doc = this.get(docId);
    return doc.annots
      .map((m) => ({ ...m.a, page: this.indexOfUid(doc, m.pageUid) }))
      .filter((a) => a.page >= 0)
      .sort((a, b) => a.page - b.page || a.rect.y - b.rect.y || a.rect.x - b.rect.x);
  }

  async setShowAnnotations(docId: DocId, show: boolean): Promise<CommandResult> {
    const doc = this.get(docId);
    doc.showAnnots = show;
    doc.revision++;
    const info = this.info(doc);
    return { info, invalidatedPages: doc.pages.map((_, i) => i) };
  }

  async getFormWidgets(docId: DocId): Promise<FormWidget[]> {
    const doc = this.get(docId);
    if (!doc.forms) return [];
    const out: FormWidget[] = [];
    doc.pages.forEach((_, i) => out.push(...this.widgetsOf(doc, i, out.length)));
    return out;
  }

  async fieldKeystroke(docId: DocId, fieldId: string, req: KeystrokeRequest): Promise<KeystrokeResult> {
    this.get(docId);
    const base = fieldId.replace(/_\d+$/, "");
    const t = templateOf(base);
    return mockKeystroke(t?.format ?? "none", req);
  }

  async execute(docId: DocId, command: EngineCommand): Promise<CommandResult> {
    const doc = this.get(docId);
    const before = this.snapshot(doc);
    let invalidated: number[] = [];
    const created: string[] = [];
    let fieldValues: FieldValueResult[] | undefined;
    const all = () => doc.pages.map((_, i) => i);
    switch (command.type) {
      case "set-metadata":
        doc.meta = { ...doc.meta, ...command.fields };
        break;
      case "rotate-pages":
        for (const i of command.pages) {
          const p = doc.pages[i];
          if (p) p.rotation = (((p.rotation + command.degrees) % 360) as PageInfo["rotation"]);
        }
        invalidated = command.pages;
        break;
      case "delete-pages": {
        const gone = new Set(command.pages.map((i) => doc.pages[i]?.uid));
        doc.pages = doc.pages.filter((_, i) => !command.pages.includes(i));
        doc.annots = doc.annots.filter((m) => !gone.has(m.pageUid));
        invalidated = all();
        break;
      }
      case "move-pages": {
        const moving = doc.pages.filter((_, i) => command.pages.includes(i));
        const rest = doc.pages.filter((_, i) => !command.pages.includes(i));
        rest.splice(Math.min(command.to, rest.length), 0, ...moving);
        doc.pages = rest;
        invalidated = all();
        break;
      }
      case "duplicate-pages": {
        const sorted = [...command.pages].sort((a, b) => a - b);
        const out: MockPage[] = [];
        doc.pages.forEach((p, i) => {
          out.push(p);
          if (sorted.includes(i)) {
            const dup: MockPage = { ...p, uid: this.nextUid++, suffix: `${p.suffix}_${this.nextUid}` };
            out.push(dup);
            if (doc.forms) Object.assign(doc.values, initialValues(dup.suffix));
            for (const m of doc.annots.filter((x) => x.pageUid === p.uid && !x.a.replyTo)) {
              doc.annots.push({ pageUid: dup.uid, a: { ...structuredClone(m.a), id: `a${this.nextAnnot++}` } });
            }
          }
        });
        doc.pages = out;
        invalidated = all();
        break;
      }
      case "insert-blank-page": {
        const ref = doc.pages[Math.max(0, Math.min(command.at, doc.pages.length - 1))];
        const page: MockPage = {
          uid: this.nextUid++,
          src: -1,
          width: command.width ?? ref?.width ?? 612,
          height: command.height ?? ref?.height ?? 792,
          rotation: 0,
          suffix: "",
        };
        doc.pages.splice(Math.max(0, Math.min(command.at, doc.pages.length)), 0, page);
        invalidated = all();
        break;
      }
      case "insert-pages": {
        const count = pageCountOf(command.sourcePath);
        const picks = command.pages ?? Array.from({ length: count }, (_, i) => i);
        const add: MockPage[] = picks.map((i) => ({ uid: this.nextUid++, src: 100 + i, ...srcSize(100 + i), rotation: 0, suffix: "" }));
        doc.pages.splice(Math.max(0, Math.min(command.at, doc.pages.length)), 0, ...add);
        invalidated = all();
        break;
      }
      case "add-annotation": {
        const n = command.annot;
        const page = doc.pages[n.page] ?? this.fail(n.page);
        const id = `a${this.nextAnnot++}`;
        const now = new Date().toISOString();
        const props: AnnotProps = { color: "#000000", opacity: 1, width: 1, fill: null, author: "", subject: "", contents: "", ...DEFAULTS[n.type], ...n.props };
        const points = n.quads ? n.quads.flatMap(quadPoints) : n.ink ? n.ink.flat() : n.line ? [...n.line] : [];
        const rect = n.rect ?? (points.length ? boundsOf(points) : { x: 0, y: 0, width: 0, height: 0 });
        const a: Annot = { id, page: n.page, type: n.type, rect, quads: n.quads, ink: n.ink, line: n.line, props, created: now, modified: now, editable: true };
        doc.annots.push({ pageUid: page.uid, a });
        created.push(id);
        invalidated = [n.page];
        break;
      }
      case "update-annotation": {
        const m = this.annotOf(doc, command.id);
        if (command.props) m.a.props = { ...m.a.props, ...command.props };
        if (command.rect) this.remap(m.a, command.rect);
        if (command.ink) m.a.ink = command.ink;
        if (command.line) m.a.line = command.line;
        m.a.modified = new Date().toISOString();
        invalidated = [this.indexOfUid(doc, m.pageUid)];
        break;
      }
      case "delete-annotations": {
        const ids = new Set(command.ids);
        invalidated = [...new Set(doc.annots.filter((m) => ids.has(m.a.id)).map((m) => this.indexOfUid(doc, m.pageUid)))];
        doc.annots = doc.annots.filter((m) => !ids.has(m.a.id) && !(m.a.replyTo && ids.has(m.a.replyTo)));
        break;
      }
      case "erase-ink": {
        const m = this.annotOf(doc, command.id);
        const strokes: Point[][] = [];
        for (const s of m.a.ink ?? []) {
          let cur: Point[] = [];
          for (const pt of s) {
            const hit = command.path.some((e) => Math.hypot(e.x - pt.x, e.y - pt.y) <= command.radius);
            if (hit) {
              if (cur.length > 1) strokes.push(cur);
              cur = [];
            } else cur.push(pt);
          }
          if (cur.length > 1) strokes.push(cur);
        }
        invalidated = [this.indexOfUid(doc, m.pageUid)];
        if (strokes.length === 0) doc.annots = doc.annots.filter((x) => x !== m);
        else {
          m.a.ink = strokes;
          m.a.rect = boundsOf(strokes.flat());
        }
        break;
      }
      case "add-reply": {
        const parent = this.annotOf(doc, command.parentId);
        const id = `a${this.nextAnnot++}`;
        const now = new Date().toISOString();
        doc.annots.push({
          pageUid: parent.pageUid,
          a: {
            id,
            page: parent.a.page,
            type: "note",
            rect: { ...parent.a.rect },
            props: { color: "#ffe066", opacity: 1, width: 1, fill: null, author: command.author, subject: "", contents: command.contents },
            created: now,
            modified: now,
            replyTo: parent.a.id,
            editable: true,
          },
        });
        created.push(id);
        break;
      }
      case "set-field-value": {
        const res = this.setField(doc, command.fieldId, command.value);
        if (res.rejected) return { info: this.info(doc), invalidatedPages: [], rejected: res.rejected };
        fieldValues = res.values;
        invalidated = all();
        break;
      }
      case "reset-form": {
        const ids = command.fieldIds;
        fieldValues = [];
        for (const p of doc.pages) {
          const init = initialValues(p.suffix);
          for (const [k, v] of Object.entries(init)) {
            if (ids && !ids.includes(k)) continue;
            doc.values[k] = v;
          }
        }
        for (const w of this.allWidgets(doc)) if (!ids || ids.includes(w.fieldId)) fieldValues.push({ fieldId: w.fieldId, value: w.value, display: w.display });
        invalidated = all();
        break;
      }
      case "add-flat-text": {
        const page = doc.pages[command.page] ?? this.fail(command.page);
        const id = `a${this.nextAnnot++}`;
        const now = new Date().toISOString();
        doc.annots.push({
          pageUid: page.uid,
          a: {
            id,
            page: command.page,
            type: "fill-text",
            rect: command.rect,
            props: { color: command.color, opacity: 1, width: command.fontSize, fill: null, author: "", subject: "", contents: command.text },
            created: now,
            modified: now,
            editable: true,
          },
        });
        created.push(id);
        invalidated = [command.page];
        break;
      }
      case "add-flat-mark": {
        const page = doc.pages[command.page] ?? this.fail(command.page);
        const id = `a${this.nextAnnot++}`;
        const now = new Date().toISOString();
        const s = command.size;
        doc.annots.push({
          pageUid: page.uid,
          a: {
            id,
            page: command.page,
            type: "fill-mark",
            rect: { x: command.at.x - s / 2, y: command.at.y - s / 2, width: s, height: s },
            props: { color: command.color, opacity: 1, width: s, fill: null, author: "", subject: "", contents: command.mark },
            created: now,
            modified: now,
            editable: true,
          },
        });
        created.push(id);
        invalidated = [command.page];
        break;
      }
    }
    doc.undo.push(before);
    doc.redo = [];
    const res = this.commit(doc, invalidated);
    return { ...res, created: created.length ? created : undefined, fieldValues };
  }

  async undo(docId: DocId): Promise<CommandResult> {
    const doc = this.get(docId);
    const snap = doc.undo.pop();
    if (!snap) return { info: this.info(doc), invalidatedPages: [] };
    doc.redo.push(this.snapshot(doc));
    this.restore(doc, snap);
    return this.commit(doc, doc.pages.map((_, i) => i));
  }

  async redo(docId: DocId): Promise<CommandResult> {
    const doc = this.get(docId);
    const snap = doc.redo.pop();
    if (!snap) return { info: this.info(doc), invalidatedPages: [] };
    doc.undo.push(this.snapshot(doc));
    this.restore(doc, snap);
    return this.commit(doc, doc.pages.map((_, i) => i));
  }

  async save(docId: DocId, options?: SaveOptions): Promise<SaveReport> {
    const doc = this.get(docId);
    this.saveLog.push({ docId, options });
    const mode = options?.mode ?? "default";
    if (doc.signed && !options?.choice) {
      if (mode === "optimized") return { status: "decision-needed", reason: "signed-optimized", choices: ["save-copy", "optimize-and-invalidate"] };
      if (doc.repaired && !options?.path) return { status: "decision-needed", reason: "signed-repaired", choices: ["append-anyway", "save-copy"] };
    }
    if (options?.choice === "save-copy" && !options.path) throw new HostError("rejected", "Save a copy needs a destination");
    if (options?.path) {
      doc.path = options.path;
      doc.name = options.path.split(/[\\/]/).pop() ?? doc.name;
    }
    let historyTruncated: boolean | undefined;
    if (mode === "optimized") {
      historyTruncated = doc.undo.length + doc.redo.length > 0;
      doc.undo = [];
      doc.redo = [];
    }
    const rewrittenBecause = doc.repaired && !doc.signed && mode === "default" ? ("repaired" as const) : undefined;
    doc.saves++;
    doc.dirty = false;
    doc.origin = this.snapshot(doc);
    const suggestOptimize = mode === "default" && doc.saves === 3 && !this.suggestDisabled ? true : undefined;
    return { status: "saved", info: this.info(doc), suggestOptimize, rewrittenBecause, historyTruncated };
  }

  async disableOptimizeSuggestion(): Promise<void> {
    this.suggestDisabled = true;
  }

  async runFileOp(op: FileOp): Promise<FileOpResult> {
    this.fileOpLog.push(op);
    const jobId = `job-${this.nextJob++}`;
    let outputs: string[];
    if (op.type === "extract") {
      outputs = op.mode === "single" ? [op.destination] : op.pages.map((p) => `${op.destination}/page-${p + 1}.pdf`);
    } else if (op.type === "merge") {
      if (op.inputs.length === 0) throw new HostError("rejected", "Nothing to merge");
      outputs = [op.destination];
    } else {
      const doc = this.get(op.docId);
      const total = doc.pages.length;
      const ranges = op.by.kind === "every" ? this.everyRanges(total, op.by.n) : this.parseRanges(op.by.ranges, total);
      if (ranges.length === 0) throw new HostError("rejected", "Invalid ranges");
      outputs = ranges.map((r, i) => `${op.destinationDir}/${doc.name.replace(/\.pdf$/i, "")}-part-${i + 1}-p${r[0] + 1}-${r[1] + 1}.pdf`);
    }
    outputs.forEach((_, i) => this.emit({ type: "job-progress", jobId, done: i + 1, total: outputs.length, finished: i + 1 === outputs.length }));
    return { jobId, outputs };
  }

  async recoveryEntries(): Promise<RecoveryEntry[]> {
    return this.recoveryList.map((e) => ({ ...e }));
  }

  async recover(id: string, action: RecoveryAction): Promise<DocumentInfo | null> {
    const entry = this.recoveryList.find((e) => e.id === id);
    if (!entry) throw new HostError("not-found", "No such recovery entry");
    if (action !== "discard" && entry.unfinished) {
      /* the prompt is answered separately via resolveUnfinished */
    }
    if (action === "discard") {
      this.recoveryList = this.recoveryList.filter((e) => e.id !== id);
      return null;
    }
    if (action === "restore" && entry.state === "original-changed") throw new HostError("rejected", "The original file changed");
    const info = await this.openDocument({ kind: "path", path: entry.path ?? `mock://${entry.name}?pages=6` });
    const doc = this.get(info.docId);
    doc.dirty = true;
    doc.revision = 1;
    doc.undo.push(this.snapshot(doc));
    doc.meta.title = `${doc.meta.title} (recovered)`;
    if (!entry.unfinished) this.recoveryList = this.recoveryList.filter((e) => e.id !== id);
    return this.info(doc);
  }

  async resolveUnfinished(id: string, action: UnfinishedAction): Promise<void> {
    this.unfinishedLog.push({ id, action });
    this.recoveryList = this.recoveryList.filter((e) => e.id !== id);
  }

  async getPrivacy(): Promise<PrivacySettings> {
    return { ...this.privacy };
  }

  async setUpdateCheck(choice: "on" | "off"): Promise<PrivacySettings> {
    if (this.privacy.lockedByPolicy) throw new HostError("rejected", "Managed by policy");
    this.privacy = { ...this.privacy, updateCheck: choice };
    writeLs("papyrine.mock.privacy", choice);
    return { ...this.privacy };
  }

  async checkForUpdates(): Promise<SecurityUpdate | null> {
    if (this.privacy.updateCheck !== "on") throw new HostError("rejected", "Update check is off");
    this.updateChecks++;
    this.privacy.lastChecked = "2026-10-06T10:00:00Z";
    return this.nextUpdate;
  }

  async downloadUpdate(): Promise<string> {
    return "/mock/Downloads/papyrine-update.pkg";
  }

  async getPrintSetup(): Promise<PrintSetup> {
    if (readLs("papyrine.mock.print") === "unsupported") return { supported: false, printers: [] };
    return {
      supported: true,
      printers: [
        { id: "office", name: "Office Printer", isDefault: true },
        { id: "label", name: "Label Printer", isDefault: false },
      ],
    };
  }

  async print(docId: DocId, options: PrintOptions): Promise<JobId> {
    this.get(docId);
    this.printLog.push({ docId, options });
    return `job-${this.nextJob++}`;
  }

  on<T extends HostEventType>(type: T, handler: (e: Extract<HostEvent, { type: T }>) => void): Unsubscribe {
    let set = this.listeners.get(type);
    if (!set) this.listeners.set(type, (set = new Set()));
    set.add(handler as (e: never) => void);
    return () => set.delete(handler as (e: never) => void);
  }

  /** Test hook: simulate the file changing on disk. */
  simulateExternalChange(docId: DocId): void {
    this.emit({ type: "file-changed-on-disk", docId });
  }

  /** Test hook: deliver any host event. */
  emitEvent(e: HostEvent): void {
    this.emit(e);
  }

  /** Test hook: the document as the engine holds it, without going through the UI. */
  peek(docId: DocId): { pages: number; srcOrder: number[]; rotations: number[]; annots: number; values: Record<string, string> } {
    const doc = this.get(docId);
    return { pages: doc.pages.length, srcOrder: doc.pages.map((p) => p.src), rotations: doc.pages.map((p) => p.rotation), annots: doc.annots.length, values: { ...doc.values } };
  }

  private emit(e: HostEvent): void {
    for (const h of this.listeners.get(e.type) ?? []) (h as (e: HostEvent) => void)(e);
  }

  private commit(doc: MockDoc, invalidatedPages: number[]): CommandResult {
    doc.revision++;
    doc.dirty = true;
    const info = this.info(doc);
    this.emit({ type: "document-changed", info, invalidatedPages });
    return { info, invalidatedPages };
  }

  private snapshot(doc: MockDoc): Snapshot {
    return {
      meta: { ...doc.meta },
      pages: doc.pages.map((p) => ({ ...p })),
      annots: structuredClone(doc.annots),
      values: { ...doc.values },
    };
  }

  private restore(doc: MockDoc, s: Snapshot): void {
    doc.meta = { ...s.meta };
    doc.pages = s.pages.map((p) => ({ ...p }));
    doc.annots = structuredClone(s.annots);
    doc.values = { ...s.values };
  }

  private pageInfo(p: MockPage): PageInfo {
    const swap = p.rotation === 90 || p.rotation === 270;
    return { width: swap ? p.height : p.width, height: swap ? p.width : p.height, rotation: p.rotation };
  }

  private info(doc: MockDoc): DocumentInfo {
    return {
      docId: doc.id,
      name: doc.name,
      path: doc.path,
      pageCount: doc.pages.length,
      pages: doc.pages.map((p) => this.pageInfo(p)),
      meta: { ...doc.meta },
      repaired: doc.repaired,
      revision: doc.revision,
      dirty: doc.dirty,
      canUndo: doc.undo.length > 0,
      canRedo: doc.redo.length > 0,
      signed: doc.signed,
      formKind: doc.formKind,
      formScripts: doc.formScripts,
    };
  }

  private widgetsOf(doc: MockDoc, pageIndex: number, tabStart = 0): FormWidget[] {
    const p = doc.pages[pageIndex];
    if (!p || p.src < 0) return [];
    // Tab order is document-wide; callers that need it pass the running count.
    const start = tabStart || doc.pages.slice(0, pageIndex).filter((x) => x.src >= 0).length * widgetDefs(0).length;
    return buildWidgets(pageIndex, p.uid, p.suffix, doc.values, start);
  }

  private allWidgets(doc: MockDoc): FormWidget[] {
    const out: FormWidget[] = [];
    doc.pages.forEach((_, i) => out.push(...this.widgetsOf(doc, i, out.length)));
    return out;
  }

  private setField(doc: MockDoc, fieldId: string, value: string): { rejected?: { fieldId: string; message: string }; values: FieldValueResult[] } {
    const base = fieldId.replace(/_\d+$/, "");
    const t = templateOf(base);
    if (!t) throw new HostError("not-found", `no field ${fieldId}`);
    if (t.readOnly) return { values: [], rejected: { fieldId, message: "This field is read only" } };
    if (t.kind === "text") {
      const v = mockKeystroke(t.format ?? "none", { value: "", change: value, selStart: 0, selEnd: 0, commit: true });
      if (!v.accept) return { values: [], rejected: { fieldId, message: v.message ?? "Invalid value" } };
    }
    doc.values[fieldId] = value;
    const out: FieldValueResult[] = [{ fieldId, value, display: t.kind === "text" ? mockFormat(t.format ?? "none", value) : value }];
    // A calculated read-only field follows the amount, like AFSimple_Calculate would.
    if (base === "amount") {
      const suffix = fieldId.slice(base.length);
      const total = `Total ${mockFormat("number", value)}`.trim();
      doc.values[`locked${suffix}`] = value ? total : "Read only";
      out.push({ fieldId: `locked${suffix}`, value: doc.values[`locked${suffix}`] as string, display: doc.values[`locked${suffix}`] as string });
    }
    return { values: out };
  }

  private annotOf(doc: MockDoc, id: string): MockAnnot {
    return doc.annots.find((m) => m.a.id === id) ?? this.fail(id);
  }

  private indexOfUid(doc: MockDoc, uid: number): number {
    return doc.pages.findIndex((p) => p.uid === uid);
  }

  /** Move/resize an annotation to `to`, carrying its geometry along proportionally. */
  private remap(a: Annot, to: Rect): void {
    const from = a.rect;
    const sx = from.width > 0 ? to.width / from.width : 1;
    const sy = from.height > 0 ? to.height / from.height : 1;
    const map = (p: Point): Point => ({ x: to.x + (p.x - from.x) * sx, y: to.y + (p.y - from.y) * sy });
    if (a.quads) a.quads = a.quads.map((q) => quadPoints(q).flatMap((p) => Object.values(map(p))) as Quad);
    if (a.ink) a.ink = a.ink.map((s) => s.map(map));
    if (a.line) a.line = [map(a.line[0]), map(a.line[1])];
    a.rect = { ...to };
  }

  private parseRanges(spec: string, total: number): [number, number][] {
    const out: [number, number][] = [];
    for (const part of spec.split(",").map((s) => s.trim()).filter(Boolean)) {
      const m = /^(\d*)\s*-\s*(\d*)$/.exec(part) ?? /^(\d+)$/.exec(part);
      if (!m) return [];
      const a = Number(m[1] || 1);
      const b = m.length === 2 ? a : Number(m[2] || total);
      if (a < 1 || b < a || b > total) return [];
      out.push([a - 1, b - 1]);
    }
    return out;
  }

  private everyRanges(total: number, n: number): [number, number][] {
    const out: [number, number][] = [];
    for (let s = 0; s < total; s += Math.max(1, n)) out.push([s, Math.min(total, s + n) - 1]);
    return out;
  }

  private get(id: DocId): MockDoc {
    return this.docs.get(id) ?? this.fail(id);
  }

  private fail(what: unknown): never {
    throw new HostError("not-found", `not found: ${String(what)}`);
  }
}
