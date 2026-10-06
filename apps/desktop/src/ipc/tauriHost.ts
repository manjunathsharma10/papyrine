import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  CONTRACT_VERSION,
  HostError,
  type Annot,
  type CommandResult,
  type DocId,
  type DocumentInfo,
  type EngineCommand,
  type FileDialogResult,
  type FileOp,
  type FileOpResult,
  type FormWidget,
  type HostApi,
  type HostEvent,
  type HostEventType,
  type JobId,
  type KeystrokeRequest,
  type KeystrokeResult,
  type OpenSource,
  type OutlineNode,
  type PageInfo,
  type PageText,
  type PickedFile,
  type PrintOptions,
  type PrintSetup,
  type PrivacySettings,
  type RecoveryAction,
  type RecoveryEntry,
  type SaveOptions,
  type SaveReport,
  type SearchOptions,
  type SecurityUpdate,
  type Tile,
  type TileRequest,
  type UnfinishedAction,
  type Unsubscribe,
} from "./contract";

/** The host's JSON error shape (`HostErr` in Rust). */
interface WireError {
  code: HostError["code"];
  message: string;
  detail?: string;
}

/** `HostError` plus the machine-readable reason some errors carry (`signed-and-damaged`). */
export class DetailedHostError extends HostError {
  constructor(
    code: HostError["code"],
    message: string,
    public detail?: string,
  ) {
    super(code, message);
  }
}

export function toHostError(e: unknown): Error {
  if (e instanceof Error) return e;
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    const w = e as WireError;
    return new DetailedHostError(w.code, w.message, w.detail);
  }
  return new HostError("internal", String(e));
}

/** Events the host sends beyond `HostEvent` (the UI may adopt them later). */
export type ExtraEvent =
  | { type: "notice"; level: "info" | "warn"; code: string; message: string; docId: DocId | null }
  | { type: "recovery-available"; entries: RecoveryEntry[] };

type Handler = (e: never) => void;

/**
 * Everything the real hosts share: each `HostApi` method forwards to `call(method, args)`
 * (the Rust `dispatch::call`), events fan out to listeners, tiles and byte uploads are
 * transport specific. `TauriHost` and the dev-bridge client only implement the transport.
 */
export abstract class RemoteHost implements HostApi {
  readonly contractVersion = CONTRACT_VERSION;
  private listeners = new Map<string, Set<Handler>>();
  private anyListeners = new Set<(e: HostEvent | ExtraEvent) => void>();

  protected abstract call<T>(method: string, args: unknown[]): Promise<T>;
  protected abstract fetchTile(req: TileRequest, signal?: AbortSignal): Promise<Tile>;
  protected abstract uploadBytes(name: string, data: ArrayBuffer): Promise<DocumentInfo>;

  /** Transport hands every host event here. */
  protected dispatch(e: HostEvent | ExtraEvent): void {
    for (const h of this.listeners.get(e.type) ?? []) (h as (e: unknown) => void)(e);
    for (const h of this.anyListeners) h(e);
  }

  on<T extends HostEventType>(type: T, handler: (e: Extract<HostEvent, { type: T }>) => void): Unsubscribe {
    let set = this.listeners.get(type);
    if (!set) this.listeners.set(type, (set = new Set()));
    set.add(handler as Handler);
    return () => set.delete(handler as Handler);
  }

  /** Subscribe to every event, including the host-only ones (`notice`, `recovery-available`). */
  onAny(handler: (e: HostEvent | ExtraEvent) => void): Unsubscribe {
    this.anyListeners.add(handler);
    return () => this.anyListeners.delete(handler);
  }

  // --- Files -----------------------------------------------------------------
  showOpenDialog = (): Promise<FileDialogResult> => this.call("showOpenDialog", []);
  openDocument(source: OpenSource, _signal?: AbortSignal): Promise<DocumentInfo> {
    if (source.kind === "bytes") return this.uploadBytes(source.name, source.data);
    return this.call("openDocument", [source]);
  }
  closeDocument = (docId: DocId): Promise<void> => this.call("closeDocument", [docId]);
  recentFiles = (): Promise<{ name: string; path: string }[]> => this.call("recentFiles", []);
  clearRecentFiles = (): Promise<void> => this.call("clearRecentFiles", []);
  pickPdfs = (options: { multiple: boolean }): Promise<PickedFile[]> => this.call("pickPdfs", [options]);
  pickSavePath = (options: { suggestedName: string; directory?: boolean }): Promise<string | null> =>
    this.call("pickSavePath", [options]);
  reloadFromDisk = (docId: DocId): Promise<DocumentInfo> => this.call("reloadFromDisk", [docId]);
  keepMine = (docId: DocId): Promise<void> => this.call("keepMine", [docId]);
  resolveActionPrompt = (docId: DocId, promptId: string, allow: boolean): Promise<void> =>
    this.call("resolveActionPrompt", [docId, promptId, allow]);

  // --- Reading ---------------------------------------------------------------
  getPageInfo = (docId: DocId, page: number): Promise<PageInfo> => this.call("getPageInfo", [docId, page]);
  getOutline = (docId: DocId): Promise<OutlineNode[]> => this.call("getOutline", [docId]);
  /**
   * The host renders at sqrt(2) zoom buckets (ADR-009); the UI asks for tiles at its own
   * quantised scale. Tiles at a bucket scale pass straight through; anything else is
   * composed from the (at most four) bucket tiles that cover it, resampled once.
   */
  getTile = (req: TileRequest, signal?: AbortSignal): Promise<Tile> => {
    const b = bucketFor(req.scale);
    const bs = Math.SQRT2 ** b;
    if (Math.abs(bs / req.scale - 1) < 1e-4) return this.fetchTile({ ...req, scale: bs }, signal);
    return composeTile(req, bs, (tx, ty) => this.fetchTile({ ...req, scale: bs, tx, ty }, signal));
  };
  getPageText = (docId: DocId, page: number, _signal?: AbortSignal): Promise<PageText> =>
    this.call("getPageText", [docId, page]);

  // --- Search ----------------------------------------------------------------
  search = (docId: DocId, query: string, options: SearchOptions): Promise<JobId> =>
    this.call("search", [docId, query, options]);
  cancelJob = (jobId: JobId): Promise<void> => this.call("cancelJob", [jobId]);

  // --- Annotations and forms ---------------------------------------------------
  listAnnotations = (docId: DocId): Promise<Annot[]> => this.call("listAnnotations", [docId]);
  setShowAnnotations = (docId: DocId, show: boolean): Promise<CommandResult> =>
    this.call("setShowAnnotations", [docId, show]);
  getFormWidgets = (docId: DocId): Promise<FormWidget[]> => this.call("getFormWidgets", [docId]);
  fieldKeystroke = (docId: DocId, fieldId: string, req: KeystrokeRequest): Promise<KeystrokeResult> =>
    this.call("fieldKeystroke", [docId, fieldId, req]);

  // --- Editing -----------------------------------------------------------------
  execute = (docId: DocId, command: EngineCommand): Promise<CommandResult> => this.call("execute", [docId, command]);
  undo = (docId: DocId): Promise<CommandResult> => this.call("undo", [docId]);
  redo = (docId: DocId): Promise<CommandResult> => this.call("redo", [docId]);
  save = (docId: DocId, options?: SaveOptions): Promise<SaveReport> => this.call("save", [docId, options ?? null]);
  disableOptimizeSuggestion = (): Promise<void> => this.call("disableOptimizeSuggestion", []);

  // --- New-file operations -------------------------------------------------------
  runFileOp = (op: FileOp): Promise<FileOpResult> => this.call("runFileOp", [op]);

  // --- Recovery ------------------------------------------------------------------
  recoveryEntries = (): Promise<RecoveryEntry[]> => this.call("recoveryEntries", []);
  recover = (id: string, action: RecoveryAction): Promise<DocumentInfo | null> => this.call("recover", [id, action]);
  resolveUnfinished = (id: string, action: UnfinishedAction): Promise<void> =>
    this.call("resolveUnfinished", [id, action]);

  // --- Privacy and updates -------------------------------------------------------
  getPrivacy = (): Promise<PrivacySettings> => this.call("getPrivacy", []);
  setUpdateCheck = (choice: "on" | "off"): Promise<PrivacySettings> => this.call("setUpdateCheck", [choice]);
  checkForUpdates = (): Promise<SecurityUpdate | null> => this.call("checkForUpdates", []);
  downloadUpdate = (id: string): Promise<string> => this.call("downloadUpdate", [id]);

  // --- Printing ------------------------------------------------------------------
  getPrintSetup = (): Promise<PrintSetup> => this.call("getPrintSetup", []);
  print = (docId: DocId, options: PrintOptions): Promise<JobId> => this.call("print", [docId, options]);

  // --- Host extras (not in the contract) -------------------------------------------
  /** A document asked for a URI or Launch action; the host emits `action-prompt`. */
  requestAction = (docId: DocId, kind: "uri" | "launch", target: string): Promise<string> =>
    this.call("requestAction", [docId, kind, target]);
  /** Window focus: the host re-checks open files for external changes. */
  notifyFocus = (): Promise<void> => this.call("notifyFocus", []);
  hostStats = (): Promise<Record<string, unknown>> => this.call("hostStats", []);
}

export const TILE = 512;
const MIN_BUCKET = -8;
const MAX_BUCKET = 16;

/** Nearest zoom bucket (`scale = sqrt(2)^bucket`) for a device-px-per-point scale. */
export function bucketFor(scale: number): number {
  if (!Number.isFinite(scale) || scale <= 0) return 0;
  return Math.min(MAX_BUCKET, Math.max(MIN_BUCKET, Math.round(Math.log2(scale) * 2)));
}

/** Build tile (tx,ty) at `req.scale` from tiles of the bucket scale `bs`. */
export async function composeTile(
  req: TileRequest,
  bs: number,
  fetchBucket: (tx: number, ty: number) => Promise<Tile>,
): Promise<Tile> {
  const r = bs / req.scale; // bucket pixels per requested pixel
  const x0 = req.tx * TILE * r;
  const y0 = req.ty * TILE * r;
  const tx0 = Math.floor(x0 / TILE);
  const ty0 = Math.floor(y0 / TILE);
  const tx1 = Math.ceil((x0 + TILE * r) / TILE) - 1;
  const ty1 = Math.ceil((y0 + TILE * r) / TILE) - 1;
  const jobs: Promise<{ tx: number; ty: number; tile: Tile | null }>[] = [];
  for (let ty = ty0; ty <= ty1; ty++) {
    for (let tx = tx0; tx <= tx1; tx++) {
      jobs.push(
        fetchBucket(tx, ty).then(
          (tile) => ({ tx, ty, tile }),
          (e) => {
            // Past the page edge the host has no tile; that is not an error here.
            if (e instanceof HostError && e.code === "cancelled") throw e;
            return { tx, ty, tile: null };
          },
        ),
      );
    }
  }
  const got = (await Promise.all(jobs)).filter((j): j is { tx: number; ty: number; tile: Tile } => j.tile !== null);
  if (!got.length) throw new HostError("not-found", "tile outside the page");
  // Page size in bucket pixels comes from the last tile in each direction.
  const pageW = Math.max(...got.map((g) => g.tx * TILE + g.tile.width));
  const pageH = Math.max(...got.map((g) => g.ty * TILE + g.tile.height));
  const outW = Math.min(TILE, Math.round(pageW / r) - req.tx * TILE);
  const outH = Math.min(TILE, Math.round(pageH / r) - req.ty * TILE);
  if (outW <= 0 || outH <= 0) {
    for (const g of got) g.tile.bitmap.close();
    throw new HostError("not-found", "tile outside the page");
  }
  const canvas: OffscreenCanvas | HTMLCanvasElement =
    typeof OffscreenCanvas !== "undefined" ? new OffscreenCanvas(outW, outH) : Object.assign(document.createElement("canvas"), { width: outW, height: outH });
  const ctx = canvas.getContext("2d") as OffscreenCanvasRenderingContext2D | CanvasRenderingContext2D | null;
  if (!ctx) throw new HostError("internal", "no 2d canvas");
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, outW, outH);
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  for (const g of got) {
    ctx.drawImage(g.tile.bitmap, (g.tx * TILE - x0) / r, (g.ty * TILE - y0) / r, g.tile.width / r, g.tile.height / r);
    g.tile.bitmap.close();
  }
  const bitmap = await createImageBitmap(canvas);
  return { bitmap, width: outW, height: outH };
}

let nextRid = 1;
export const newRequestId = (): number => nextRid++;

/** Raw RGBA of one tile as the host sends it (ADR-009). */
export async function bitmapFromRgba(rgba: ArrayBuffer | Uint8Array, width: number, height: number): Promise<Tile> {
  const bytes = rgba instanceof Uint8Array ? rgba : new Uint8Array(rgba);
  if (bytes.length !== width * height * 4) throw new HostError("internal", "unexpected tile size");
  const copy = new Uint8ClampedArray(bytes.length);
  copy.set(bytes);
  const bitmap = await createImageBitmap(new ImageData(copy, width, height));
  return { bitmap, width, height };
}

// Windows/Android WebView2 maps custom schemes to http://<scheme>.localhost (ADR-037).
const isWindows = typeof navigator !== "undefined" && navigator.userAgent.includes("Windows");
const SCHEME_BASE = isWindows ? "http://papyrine.localhost" : "papyrine://localhost";

/** The real host: Tauri commands + events, tiles over `papyrine://`. */
export class TauriHost extends RemoteHost {
  constructor() {
    super();
    void listen<HostEvent | ExtraEvent>("host-event", (e) => this.dispatch(e.payload)).then(async () => {
      // Files that arrived before this window was listening (file association, argv).
      const sources = await this.call<OpenSource[]>("takePendingOpens", []).catch(() => []);
      if (sources.length) this.dispatch({ type: "open-requested", sources });
    });
  }

  protected async call<T>(method: string, args: unknown[]): Promise<T> {
    try {
      return await invoke<T>("host_call", { method, args });
    } catch (e) {
      throw toHostError(e);
    }
  }

  protected async uploadBytes(name: string, data: ArrayBuffer): Promise<DocumentInfo> {
    try {
      return await invoke<DocumentInfo>("open_bytes", new Uint8Array(data), {
        headers: { "x-papyrine-name": encodeURIComponent(name) },
      });
    } catch (e) {
      throw toHostError(e);
    }
  }

  protected async fetchTile(req: TileRequest, signal?: AbortSignal): Promise<Tile> {
    if (signal?.aborted) throw new HostError("cancelled", "tile cancelled");
    const rid = newRequestId();
    const url = `${SCHEME_BASE}/tile/${req.docId}/${req.page}/${req.scale}/${req.tx}/${req.ty}?rid=${rid}&v=${req.revision}`;
    const controller = new AbortController();
    const onAbort = () => {
      controller.abort();
      // A render that has not started is dropped; one that has is cancelled.
      void invoke("cancel_tile", { rid }).catch(() => undefined);
    };
    signal?.addEventListener("abort", onAbort, { once: true });
    try {
      const res = await fetch(url, { signal: controller.signal });
      if (!res.ok) {
        const why = res.headers.get("x-papyrine-error") ?? `tile request failed: ${res.status}`;
        throw new HostError(res.status === 410 ? "cancelled" : "not-found", why);
      }
      const w = Number(res.headers.get("x-papyrine-width"));
      const h = Number(res.headers.get("x-papyrine-height"));
      return await bitmapFromRgba(await res.arrayBuffer(), w, h);
    } catch (e) {
      if (controller.signal.aborted) throw new HostError("cancelled", "tile cancelled");
      throw e;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }
  }

  /** Low-resolution page preview (L2 cache, QOI inside the host). */
  async getPreview(docId: DocId, page: number, edge: number, signal?: AbortSignal): Promise<Tile> {
    const rid = newRequestId();
    const res = await fetch(`${SCHEME_BASE}/preview/${docId}/${page}/${edge}?rid=${rid}`, { signal });
    if (!res.ok) throw new HostError("not-found", res.headers.get("x-papyrine-error") ?? "preview failed");
    return bitmapFromRgba(await res.arrayBuffer(), Number(res.headers.get("x-papyrine-width")), Number(res.headers.get("x-papyrine-height")));
  }
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
