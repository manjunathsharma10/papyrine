import {
  type CommandResult,
  type DocId,
  type DocumentInfo,
  type DocumentMeta,
  type EngineCommand,
  type FileDialogResult,
  type HostApi,
  type HostEvent,
  type HostEventType,
  HostError,
  CONTRACT_VERSION,
  type JobId,
  type OpenSource,
  type OutlineNode,
  type PageInfo,
  type PageText,
  type SaveOptions,
  type SearchOptions,
  type Tile,
  type TileRequest,
  type Unsubscribe,
} from "./contract";
import { mockLines, mockPageText, renderMockTile } from "./mockPaint";

/**
 * In-memory host used by Vite dev and E2E. Documents are synthetic: a
 * mock path looks like `mock://name?pages=24`. Pages are drawn on canvas so
 * the viewer, text layer and search can be developed without the Rust host.
 */

interface MockPage {
  /** Stable identity of the source page (deterministic content). */
  src: number;
  width: number;
  height: number;
  rotation: 0 | 90 | 180 | 270;
}

interface MockDoc {
  id: DocId;
  name: string;
  path: string;
  meta: DocumentMeta;
  pages: MockPage[];
  revision: number;
  dirty: boolean;
  undo: Snapshot[];
  redo: Snapshot[];
}

interface Snapshot {
  meta: DocumentMeta;
  pages: MockPage[];
}

function srcSize(src: number): { width: number; height: number } {
  if (src % 7 === 6) return { width: 792, height: 612 }; // landscape letter
  if (src % 5 === 4) return { width: 595, height: 842 }; // A4
  return { width: 612, height: 792 };
}

export class MockHost implements HostApi {
  readonly contractVersion = CONTRACT_VERSION;
  private docs = new Map<DocId, MockDoc>();
  private listeners = new Map<HostEventType, Set<(e: never) => void>>();
  private jobs = new Map<JobId, { cancelled: boolean }>();
  private nextDoc = 1;
  private nextJob = 1;
  private sampleCounter = 0;
  /** Artificial tile latency in ms (tests can tune it). */
  tileDelayMs = 4;

  async showOpenDialog(): Promise<FileDialogResult> {
    const n = ++this.sampleCounter;
    const pages = n === 1 ? 24 : 8 + n;
    return { sources: [{ kind: "path", path: `mock://sample-${n}.pdf?pages=${pages}` }] };
  }

  async openDocument(source: OpenSource): Promise<DocumentInfo> {
    let name: string;
    let path: string;
    let pageCount = 12;
    if (source.kind === "path") {
      path = source.path;
      const m = /^mock:\/\/([^?]*)(?:\?(.*))?$/.exec(path);
      name = m?.[1] ?? path.split(/[\\/]/).pop() ?? path;
      const q = new URLSearchParams(m?.[2] ?? "");
      pageCount = Number(q.get("pages") ?? 12);
      if (path.includes("missing")) throw new HostError("not-found", "No such file");
    } else {
      name = source.name;
      path = `mock://${source.name}`;
    }
    const id = `doc-${this.nextDoc++}`;
    const pages: MockPage[] = Array.from({ length: pageCount }, (_, i) => ({
      src: i,
      ...srcSize(i),
      rotation: 0,
    }));
    const doc: MockDoc = {
      id,
      name,
      path,
      meta: {
        title: name.replace(/\.pdf$/i, ""),
        author: "Papyrine Mock",
        subject: "",
        keywords: "",
        producer: "mock-host",
        pdfVersion: "1.7",
        encrypted: false,
      },
      pages,
      revision: 0,
      dirty: false,
      undo: [],
      redo: [],
    };
    this.docs.set(id, doc);
    return this.info(doc);
  }

  async closeDocument(docId: DocId): Promise<void> {
    this.docs.delete(docId);
  }

  async recentFiles() {
    return [
      { name: "annual-report.pdf", path: "mock://annual-report.pdf?pages=40" },
      { name: "invoice.pdf", path: "mock://invoice.pdf?pages=2" },
    ];
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
    return renderMockTile(this.pageInfo(page), page.src, doc.pages.indexOf(page), req.scale, req.tx, req.ty);
  }

  async getPageText(docId: DocId, page: number): Promise<PageText> {
    const p = this.get(docId).pages[page] ?? this.fail(page);
    return mockPageText(p.src, page);
  }

  async search(docId: DocId, query: string, options: SearchOptions): Promise<JobId> {
    const doc = this.get(docId);
    const jobId = `job-${this.nextJob++}`;
    const job = { cancelled: false };
    this.jobs.set(jobId, job);
    const q = options.caseSensitive ? query : query.toLowerCase();
    const total = doc.pages.length;
    let page = 0;
    const step = () => {
      if (job.cancelled || !this.docs.has(docId)) return;
      const lines = mockLines(doc.pages[page]?.src ?? 0);
      for (const line of lines) {
        const hay = options.caseSensitive ? line : line.toLowerCase();
        let from = 0;
        for (;;) {
          const at = q ? hay.indexOf(q, from) : -1;
          if (at < 0) break;
          from = at + q.length;
          const before = at === 0 ? " " : hay[at - 1] ?? " ";
          const after = hay[at + q.length] ?? " ";
          if (options.wholeWord && (/\w/.test(before) || /\w/.test(after))) continue;
          this.emit({ type: "search-hit", jobId, hit: { page, snippet: line, matchStart: at, matchLength: q.length } });
        }
      }
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

  async execute(docId: DocId, command: EngineCommand): Promise<CommandResult> {
    const doc = this.get(docId);
    const before = this.snapshot(doc);
    let invalidated: number[] = [];
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
      case "delete-pages":
        doc.pages = doc.pages.filter((_, i) => !command.pages.includes(i));
        invalidated = doc.pages.map((_, i) => i);
        break;
      case "move-pages": {
        const moving = doc.pages.filter((_, i) => command.pages.includes(i));
        const rest = doc.pages.filter((_, i) => !command.pages.includes(i));
        rest.splice(Math.min(command.to, rest.length), 0, ...moving);
        doc.pages = rest;
        invalidated = doc.pages.map((_, i) => i);
        break;
      }
    }
    doc.undo.push(before);
    doc.redo = [];
    return this.commit(doc, invalidated);
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

  async save(docId: DocId, options?: SaveOptions): Promise<DocumentInfo> {
    const doc = this.get(docId);
    if (options?.path) {
      doc.path = options.path;
      doc.name = options.path.split(/[\\/]/).pop() ?? doc.name;
    }
    doc.dirty = false;
    return this.info(doc);
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
    return { meta: { ...doc.meta }, pages: doc.pages.map((p) => ({ ...p })) };
  }

  private restore(doc: MockDoc, s: Snapshot): void {
    doc.meta = s.meta;
    doc.pages = s.pages;
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
      repaired: doc.path.includes("repaired"),
      revision: doc.revision,
      dirty: doc.dirty,
      canUndo: doc.undo.length > 0,
      canRedo: doc.redo.length > 0,
    };
  }

  private get(id: DocId): MockDoc {
    return this.docs.get(id) ?? this.fail(id);
  }

  private fail(what: unknown): never {
    throw new HostError("not-found", `not found: ${String(what)}`);
  }
}
