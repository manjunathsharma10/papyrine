/**
 * Host IPC contract (ARCHITECTURE 4.8 / 4.9).
 *
 * The UI only ever talks to the host through `HostApi`. Two implementations
 * exist: `MockHost` (src/ipc/mock.ts, used by Vite dev + E2E) and the Tauri
 * host (implemented in Rust next wave: Tauri commands + events + `papyrine://`
 * tiles). Every request carries a `RequestId` the host echoes in errors;
 * long jobs return a `JobId` with `job-progress` events and `cancelJob`.
 *
 * Units: page geometry is in PDF points (1/72 in), origin top-left of the
 * *rotated, cropped* page as displayed. Tile coordinates are in device pixels
 * at `scale` device-px per point, so tile (tx,ty) covers device rect
 * [tx*TILE_SIZE, ty*TILE_SIZE, +TILE_SIZE, +TILE_SIZE].
 */

export const TILE_SIZE = 512;
export const CONTRACT_VERSION = 1;

export type DocId = string;
export type JobId = string;
export type RequestId = string;

export interface PageInfo {
  /** Displayed width/height in points (rotation and CropBox applied). */
  width: number;
  height: number;
  rotation: 0 | 90 | 180 | 270;
  label?: string;
}

export interface DocumentMeta {
  title: string;
  author: string;
  subject: string;
  keywords: string;
  producer: string;
  pdfVersion: string;
  encrypted: boolean;
}

export type OpenSource =
  | { kind: "path"; path: string }
  /** Bytes handed over by drag-and-drop or the webview file picker. */
  | { kind: "bytes"; name: string; data: ArrayBuffer };

export interface DocumentInfo {
  docId: DocId;
  /** File name for tabs; never a full path in the UI. */
  name: string;
  path: string | null;
  pageCount: number;
  /** Sizes for every page so the viewer can lay out without per-page calls. */
  pages: PageInfo[];
  meta: DocumentMeta;
  /** Set when qpdf had to repair the file (RepairLog non-empty). */
  repaired: boolean;
  revision: number;
  dirty: boolean;
  canUndo: boolean;
  canRedo: boolean;
}

export interface OutlineNode {
  title: string;
  /** Zero-based page index or null for non-page destinations. */
  page: number | null;
  children: OutlineNode[];
}

export interface TileRequest {
  docId: DocId;
  page: number;
  /** Device pixels per point. */
  scale: number;
  tx: number;
  ty: number;
  /** Document revision; bumps invalidate cached tiles. */
  revision: number;
}

export interface Tile {
  bitmap: ImageBitmap;
  /** Pixel size actually rendered (edge tiles are smaller than TILE_SIZE). */
  width: number;
  height: number;
}

/** One positioned run of text, in points in the displayed page space. */
export interface TextRun {
  text: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface PageText {
  page: number;
  runs: TextRun[];
}

export interface SearchOptions {
  caseSensitive: boolean;
  wholeWord: boolean;
}

export interface SearchHit {
  page: number;
  snippet: string;
  /** Offset of the match inside `snippet`. */
  matchStart: number;
  matchLength: number;
}

/** Commands the UI can ask the engine to execute (grows per milestone). */
export type EngineCommand =
  | { type: "set-metadata"; fields: Partial<Pick<DocumentMeta, "title" | "author" | "subject" | "keywords">> }
  | { type: "rotate-pages"; pages: number[]; degrees: 90 | 180 | 270 }
  | { type: "delete-pages"; pages: number[] }
  | { type: "move-pages"; pages: number[]; to: number };

export interface CommandResult {
  info: DocumentInfo;
  /** Pages whose tiles must be re-requested. */
  invalidatedPages: number[];
}

export interface SaveOptions {
  /** Omit to save in place; set for Save As. */
  path?: string;
}

export interface FileDialogResult {
  sources: OpenSource[];
}

export type HostEvent =
  | { type: "document-changed"; info: DocumentInfo; invalidatedPages: number[] }
  | { type: "search-hit"; jobId: JobId; hit: SearchHit }
  | { type: "job-progress"; jobId: JobId; done: number; total: number; finished: boolean }
  | { type: "file-changed-on-disk"; docId: DocId }
  | { type: "action-prompt"; docId: DocId; kind: "uri" | "launch"; target: string }
  /** OS file association / second-instance launch asked the window to open files. */
  | { type: "open-requested"; sources: OpenSource[] };

export type HostEventType = HostEvent["type"];
export type Unsubscribe = () => void;

export class HostError extends Error {
  constructor(
    public code: "not-found" | "cancelled" | "encrypted" | "corrupt" | "io" | "internal",
    message: string,
  ) {
    super(message);
    this.name = "HostError";
  }
}

export interface HostApi {
  readonly contractVersion: typeof CONTRACT_VERSION;

  /** Native open dialog (host) or webview picker; resolves to nothing if cancelled. */
  showOpenDialog(): Promise<FileDialogResult>;
  openDocument(source: OpenSource, signal?: AbortSignal): Promise<DocumentInfo>;
  closeDocument(docId: DocId): Promise<void>;
  recentFiles(): Promise<{ name: string; path: string }[]>;

  getPageInfo(docId: DocId, page: number): Promise<PageInfo>;
  getOutline(docId: DocId): Promise<OutlineNode[]>;

  /** Render one tile. Abort cancels the render when it has not started yet. */
  getTile(req: TileRequest, signal?: AbortSignal): Promise<Tile>;
  getPageText(docId: DocId, page: number, signal?: AbortSignal): Promise<PageText>;

  /** Streams `search-hit` events; resolves with the JobId immediately. */
  search(docId: DocId, query: string, options: SearchOptions): Promise<JobId>;
  cancelJob(jobId: JobId): Promise<void>;

  execute(docId: DocId, command: EngineCommand): Promise<CommandResult>;
  undo(docId: DocId): Promise<CommandResult>;
  redo(docId: DocId): Promise<CommandResult>;
  save(docId: DocId, options?: SaveOptions): Promise<DocumentInfo>;

  on<T extends HostEventType>(type: T, handler: (e: Extract<HostEvent, { type: T }>) => void): Unsubscribe;
}
