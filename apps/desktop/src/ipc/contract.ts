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
  /** Application that created the original content (Info /Creator). */
  creator: string;
  /** ISO 8601 dates from the Info dictionary; empty when absent. */
  created: string;
  modified: string;
  /** Size of the file on disk in bytes (0 for unsaved bytes). */
  fileSize: number;
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
  /** At least one signature field is signed (v0.1: signatures are read-only). */
  signed: boolean;
  /** Form kind (ADR-006): dynamic XFA is shown read-only with a notice. */
  formKind: FormKind;
  /** The document has scripts outside the AF subset and the Adobe boilerplate allowlist. */
  formScripts: boolean;
}

export type FormKind = "none" | "acroform" | "xfa-static" | "xfa-dynamic";

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
  /** Fold accents/diacritics (on by default in the UI; ADR on search normalisation). */
  diacriticInsensitive: boolean;
  /** Also search annotation contents and replies. */
  includeComments: boolean;
  /** Also search form field values. */
  includeFormValues: boolean;
}

export type SearchSource = "text" | "comment" | "form";

/** A quadrilateral in displayed page points, as x1,y1 .. x4,y4 (any winding). */
export type Quad = [number, number, number, number, number, number, number, number];

export interface SearchHit {
  page: number;
  snippet: string;
  /** Offset of the match inside `snippet`. */
  matchStart: number;
  matchLength: number;
  /** Where the match lives; the UI labels comment and form hits. */
  source: SearchSource;
  /** Highlight quads for the match on the page (text layer coordinates). */
  quads: Quad[];
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/** Rectangle in displayed page points, origin top-left (x,y = top-left corner). */
export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Point {
  x: number;
  y: number;
}

// ---------------------------------------------------------------------------
// Annotations (ROADMAP 1.12). The host renders annotations into tiles; the UI
// only draws selection handles and in-progress shapes. Colours are `#rrggbb`.
// ---------------------------------------------------------------------------

export type AnnotType =
  | "highlight"
  | "underline"
  | "strikeout"
  | "squiggly"
  | "note"
  | "textbox"
  | "ink"
  | "rectangle"
  | "oval"
  | "line"
  | "arrow"
  /** Fill & Sign text placed on a flat form (not a comment). */
  | "fill-text"
  /** Fill & Sign mark (check, cross, dot) on a flat form (not a comment). */
  | "fill-mark"
  /** Any other subtype: displayed and preserved, never edited. */
  | "other";

export interface AnnotProps {
  color: string;
  /** 0..1 */
  opacity: number;
  /** Stroke width in points. */
  width: number;
  /** Interior colour for rectangle, oval and text box; null for none. */
  fill: string | null;
  author: string;
  subject: string;
  /** The comment text (popup contents, text box text). */
  contents: string;
}

export interface Annot {
  id: string;
  page: number;
  type: AnnotType;
  /** Bounding rectangle in page points. */
  rect: Rect;
  /** Markup quads (highlight, underline, strikeout, squiggly). */
  quads?: Quad[];
  /** Ink strokes (pen). */
  ink?: Point[][];
  /** Line and arrow end points. */
  line?: [Point, Point];
  props: AnnotProps;
  /** ISO 8601. */
  created: string;
  modified: string;
  /** Id of the annotation this one replies to. */
  replyTo?: string;
  /** False for annotations we display but do not edit (`other`). */
  editable: boolean;
}

/** What the UI sends for a new annotation; the engine builds the appearance stream. */
export interface NewAnnot {
  page: number;
  type: Exclude<AnnotType, "other" | "fill-text" | "fill-mark">;
  rect?: Rect;
  quads?: Quad[];
  ink?: Point[][];
  line?: [Point, Point];
  props: Partial<AnnotProps>;
}

export type FlatMark = "check" | "cross" | "dot";

// ---------------------------------------------------------------------------
// Forms (ROADMAP 1.13)
// ---------------------------------------------------------------------------

export type FieldKind = "text" | "checkbox" | "radio" | "combo" | "list" | "button" | "signature";

export interface ChoiceOption {
  label: string;
  value: string;
}

export interface FormWidget {
  /** Stable id of the widget annotation. */
  id: string;
  /** Fully qualified field name; radio and checkbox groups share it. */
  fieldId: string;
  name: string;
  page: number;
  rect: Rect;
  kind: FieldKind;
  /** Text and combo: the value; checkbox: "true"/"false"; radio group: the chosen export value; list: values joined by "\n". */
  value: string;
  /** Text shown now (after AF formatting) when the field is not focused. */
  display: string;
  /** Checkbox and radio: this widget's "on" export value. */
  exportValue?: string;
  options?: ChoiceOption[];
  multiline: boolean;
  password: boolean;
  /** Comb field: number of cells (0 = not a comb). */
  comb: number;
  maxLength: number;
  readOnly: boolean;
  required: boolean;
  /** Combo: free text allowed. List: more than one choice allowed. */
  editable: boolean;
  multiSelect: boolean;
  /** Font size in points; 0 = auto. */
  fontSize: number;
  align: "left" | "center" | "right";
  /** Position in the document tab order (0-based, across pages). */
  tabIndex: number;
  /** Tooltip (TU). */
  tooltip: string;
}

/** Result of asking the AF engine whether a keystroke is acceptable. */
export interface KeystrokeResult {
  accept: boolean;
  /** Replacement for the proposed value when the filter edits it (e.g. digits only). */
  value: string;
  /** Short message to show next to the field when rejected. */
  message?: string;
}

export interface KeystrokeRequest {
  /** Field text before the edit. */
  value: string;
  /** Text being typed or pasted. */
  change: string;
  selStart: number;
  selEnd: number;
  /** True for the final commit check (blur/Enter), false per keystroke. */
  commit: boolean;
}

export interface FieldValueResult {
  fieldId: string;
  value: string;
  display: string;
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/** Commands the UI can ask the engine to execute (grows per milestone). */
export type EngineCommand =
  | { type: "set-metadata"; fields: Partial<Pick<DocumentMeta, "title" | "author" | "subject" | "keywords">> }
  | { type: "rotate-pages"; pages: number[]; degrees: 90 | 180 | 270 }
  | { type: "delete-pages"; pages: number[] }
  | { type: "move-pages"; pages: number[]; to: number }
  | { type: "duplicate-pages"; pages: number[] }
  /** Blank page at `at`; size defaults to the neighbouring page. */
  | { type: "insert-blank-page"; at: number; width?: number; height?: number }
  /** Pages of another PDF (`pages` zero-based in the source; all when omitted) placed at `at`. */
  | { type: "insert-pages"; sourcePath: string; pages?: number[]; at: number }
  | { type: "add-annotation"; annot: NewAnnot }
  | { type: "update-annotation"; id: string; props?: Partial<AnnotProps>; rect?: Rect; ink?: Point[][]; line?: [Point, Point] }
  | { type: "delete-annotations"; ids: string[] }
  /** Removes ink near `path` (radius in points); strokes split rather than vanish. */
  | { type: "erase-ink"; id: string; path: Point[]; radius: number }
  | { type: "add-reply"; parentId: string; contents: string; author: string }
  /** Commits a field value through the AF pipeline (validate, format, recalculate). */
  | { type: "set-field-value"; fieldId: string; value: string }
  | { type: "reset-form"; fieldIds?: string[] }
  | { type: "add-flat-text"; page: number; rect: Rect; text: string; fontSize: number; color: string }
  | { type: "add-flat-mark"; page: number; at: Point; mark: FlatMark; size: number; color: string };

export type EngineCommandType = EngineCommand["type"];

export interface CommandResult {
  info: DocumentInfo;
  /** Pages whose tiles must be re-requested. */
  invalidatedPages: number[];
  /** Ids created by the command (annotations, replies, flat marks). */
  created?: string[];
  /** set-field-value and reset-form: displayed values of every field the command changed. */
  fieldValues?: FieldValueResult[];
  /** set-field-value: the AF validation rejected the value; nothing changed. */
  rejected?: { fieldId: string; message: string };
}

// ---------------------------------------------------------------------------
// Save, recovery
// ---------------------------------------------------------------------------

export type SaveMode = "default" | "optimized";

/** The user's answer to a `decision-needed` save. */
export type SaveChoice = "append-anyway" | "save-copy" | "optimize-and-invalidate";

export interface SaveOptions {
  /** Omit to save in place; set for Save As. */
  path?: string;
  mode?: SaveMode;
  /** Set when answering a previous `decision-needed` report. */
  choice?: SaveChoice;
}

export type DecisionReason =
  /** Signed file whose xref is damaged: an incremental save would append to a broken file. */
  | "signed-repaired"
  /** An optimized rewrite renumbers objects and invalidates every signature. */
  | "signed-optimized";

export type SaveReport =
  | {
      status: "saved";
      info: DocumentInfo;
      /** Many incremental sections: suggest "Save optimized" once (user can turn it off). */
      suggestOptimize?: boolean;
      /** The file needed repair, so the host did an optimized rewrite and says so. */
      rewrittenBecause?: "repaired";
      /** An optimized save dropped undo entries it could not map. */
      historyTruncated?: boolean;
    }
  | { status: "decision-needed"; reason: DecisionReason; choices: SaveChoice[] };

export interface RecoveryEntry {
  id: string;
  name: string;
  /** Original path (null for documents opened from bytes). */
  path: string | null;
  /** ISO 8601 time of the last journal record. */
  lastActivity: string;
  /** `restorable`: the original still matches; `original-changed`: only the recovered copy can be opened. */
  state: "restorable" | "original-changed";
  /** Set when the last command has an Intent record but no Commit (ARCHITECTURE 7). */
  unfinished?: { title: string };
}

export type RecoveryAction = "restore" | "open-copy" | "discard";

export type UnfinishedAction = "redo" | "skip";

// ---------------------------------------------------------------------------
// File operations that produce new files (not undoable)
// ---------------------------------------------------------------------------

export interface PickedFile {
  path: string;
  name: string;
  pageCount: number;
  size: number;
}

export interface MergeInput {
  path: string;
  /** Zero-based pages to take; all when omitted. */
  pages?: number[];
}

export type FileOp =
  | { type: "extract"; docId: DocId; pages: number[]; mode: "single" | "each"; destination: string }
  | { type: "merge"; inputs: MergeInput[]; outline: "per-file" | "merged"; destination: string }
  /** `ranges` like "1-3, 4, 7-" (one-based); or every N pages. */
  | { type: "split"; docId: DocId; by: { kind: "ranges"; ranges: string } | { kind: "every"; n: number }; destinationDir: string };

export interface FileOpResult {
  jobId: JobId;
  /** Files written, in order. */
  outputs: string[];
}

// ---------------------------------------------------------------------------
// Privacy and updates (ROADMAP 1.16)
// ---------------------------------------------------------------------------

/** `unset` until the person answers the first-run question; there is no default. */
export type UpdateCheckChoice = "unset" | "on" | "off";

export interface PrivacySettings {
  updateCheck: UpdateCheckChoice;
  /** ISO 8601 of the last successful check, if any. */
  lastChecked?: string;
  /** Managed installs can force the setting; the UI then disables the control. */
  lockedByPolicy: boolean;
}

export interface SecurityUpdate {
  id: string;
  version: string;
  severity: "low" | "moderate" | "high" | "critical";
  summary: string;
}

// ---------------------------------------------------------------------------
// Printing (ROADMAP 1.17)
// ---------------------------------------------------------------------------

export interface PrinterInfo {
  id: string;
  name: string;
  isDefault: boolean;
}

export interface PrintSetup {
  /** False on platforms without a print back end yet (Windows in v0.1). */
  supported: boolean;
  printers: PrinterInfo[];
}

export interface PrintOptions {
  /** Zero-based pages to print, in order. */
  pages: number[];
  scaling: "fit" | "shrink" | "actual";
  autoRotate: boolean;
  /** Print annotations and comments. */
  comments: boolean;
  copies: number;
  destination: { kind: "printer"; printerId: string } | { kind: "file"; path: string };
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export interface FileDialogResult {
  sources: OpenSource[];
}

export type HostEvent =
  | { type: "document-changed"; info: DocumentInfo; invalidatedPages: number[] }
  | { type: "search-hit"; jobId: JobId; hit: SearchHit }
  | { type: "job-progress"; jobId: JobId; done: number; total: number; finished: boolean }
  | { type: "file-changed-on-disk"; docId: DocId }
  /** URI / launch / GoToR actions are never run silently: the UI shows `target` and asks. */
  | { type: "action-prompt"; docId: DocId; promptId: string; kind: "uri" | "launch"; target: string }
  /** OS file association / second-instance launch asked the window to open files. */
  | { type: "open-requested"; sources: OpenSource[] }
  /** The engine process was restarted and the journal replayed. */
  | { type: "engine-restarted"; docId: DocId; skippedAction?: string }
  /** The opt-in update check found a security update. */
  | { type: "security-update"; update: SecurityUpdate };

export type HostEventType = HostEvent["type"];
export type Unsubscribe = () => void;

export class HostError extends Error {
  constructor(
    public code: "not-found" | "cancelled" | "encrypted" | "corrupt" | "io" | "internal" | "rejected",
    message: string,
  ) {
    super(message);
    this.name = "HostError";
  }
}

export interface HostApi {
  readonly contractVersion: typeof CONTRACT_VERSION;

  // --- Files -------------------------------------------------------------
  /** Native open dialog (host) or webview picker; resolves to nothing if cancelled. */
  showOpenDialog(): Promise<FileDialogResult>;
  openDocument(source: OpenSource, signal?: AbortSignal): Promise<DocumentInfo>;
  closeDocument(docId: DocId): Promise<void>;
  recentFiles(): Promise<{ name: string; path: string }[]>;
  clearRecentFiles(): Promise<void>;
  /** Native picker for PDFs the UI needs paths of (insert, merge). Empty when cancelled. */
  pickPdfs(options: { multiple: boolean }): Promise<PickedFile[]>;
  /** Native save dialog; `directory` picks a folder instead. Null when cancelled. */
  pickSavePath(options: { suggestedName: string; directory?: boolean }): Promise<string | null>;
  /** Reload the file from disk, discarding the in-memory document state (banner: Reload). */
  reloadFromDisk(docId: DocId): Promise<DocumentInfo>;
  /** Acknowledge a disk change and stop warning until the file changes again (banner: Keep mine). */
  keepMine(docId: DocId): Promise<void>;
  /** Answer a URI/launch prompt. The host acts only on `allow: true`. */
  resolveActionPrompt(docId: DocId, promptId: string, allow: boolean): Promise<void>;

  // --- Reading -----------------------------------------------------------
  getPageInfo(docId: DocId, page: number): Promise<PageInfo>;
  getOutline(docId: DocId): Promise<OutlineNode[]>;
  /** Render one tile. Abort cancels the render when it has not started yet. */
  getTile(req: TileRequest, signal?: AbortSignal): Promise<Tile>;
  getPageText(docId: DocId, page: number, signal?: AbortSignal): Promise<PageText>;

  // --- Search ------------------------------------------------------------
  /** Streams `search-hit` events; resolves with the JobId immediately. */
  search(docId: DocId, query: string, options: SearchOptions): Promise<JobId>;
  cancelJob(jobId: JobId): Promise<void>;

  // --- Annotations -------------------------------------------------------
  /** Every annotation (comments, replies, fill marks, other) in document order. */
  listAnnotations(docId: DocId): Promise<Annot[]>;
  /** View-only switch: hide or show annotations in rendering. Never saved in the file. */
  setShowAnnotations(docId: DocId, show: boolean): Promise<CommandResult>;

  // --- Forms -------------------------------------------------------------
  /** Every widget in document tab order. */
  getFormWidgets(docId: DocId): Promise<FormWidget[]>;
  /** Per-keystroke AF filter/validate (AFNumber_Keystroke etc.). */
  fieldKeystroke(docId: DocId, fieldId: string, req: KeystrokeRequest): Promise<KeystrokeResult>;

  // --- Editing -----------------------------------------------------------
  execute(docId: DocId, command: EngineCommand): Promise<CommandResult>;
  undo(docId: DocId): Promise<CommandResult>;
  redo(docId: DocId): Promise<CommandResult>;
  save(docId: DocId, options?: SaveOptions): Promise<SaveReport>;
  /** Turn off the one-time "Save optimized" suggestion (stored by the host). */
  disableOptimizeSuggestion(): Promise<void>;

  // --- New-file operations ----------------------------------------------
  runFileOp(op: FileOp): Promise<FileOpResult>;

  // --- Recovery ----------------------------------------------------------
  recoveryEntries(): Promise<RecoveryEntry[]>;
  recover(id: string, action: RecoveryAction): Promise<DocumentInfo | null>;
  resolveUnfinished(id: string, action: UnfinishedAction): Promise<void>;

  // --- Privacy and updates ----------------------------------------------
  getPrivacy(): Promise<PrivacySettings>;
  /** Records the person's answer; "on" is the only state in which anything is fetched. */
  setUpdateCheck(choice: "on" | "off"): Promise<PrivacySettings>;
  /** User-initiated check now (requires "on"). Resolves to an update, or null when up to date. */
  checkForUpdates(): Promise<SecurityUpdate | null>;
  /** Verified download into the user's Downloads folder; never installs. Resolves to the file path. */
  downloadUpdate(id: string): Promise<string>;

  // --- Printing ----------------------------------------------------------
  getPrintSetup(): Promise<PrintSetup>;
  print(docId: DocId, options: PrintOptions): Promise<JobId>;

  on<T extends HostEventType>(type: T, handler: (e: Extract<HostEvent, { type: T }>) => void): Unsubscribe;
}
