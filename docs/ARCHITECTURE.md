# Folio — Architecture

Status: **Draft for approval** · Last updated: 2026-09-30

This document describes how Folio is structured: processes, crates, data flow,
threading, persistence, security boundaries and third-party dependencies. It is
the reference every phase builds against. Significant choices are justified in
[DECISIONS.md](DECISIONS.md) (ADR-NNN references below).

---

## 1. Goals that shape the architecture

| Goal | Architectural consequence |
|---|---|
| Survive malformed, hostile PDFs | Parsing/rendering in separate, sandboxed, restartable processes; fuzzed code paths; repair logged, never silent |
| Acrobat-class editing with undo | Every change is a `Command` producing an object-level delta; undo = restore before-images |
| Preserve digital signatures | First-class **incremental writer** (append-only) alongside full rewrite |
| Best-in-class compression | Dedicated `folio-optimize` crate with a parallel, per-object pipeline and verification stage |
| 60 fps on 2,000+ page docs | Virtualized page list, tile renderer, render process pool, bounded LRU caches |
| Offline-first, permissive open source | No network in core paths; permissive-only dependency policy enforced in CI (ADR-001) |
| One engine, many front ends | Engine is a library + process; the desktop app and `folio` CLI both drive it |

---

## 2. Process model

```
┌──────────────────────────────────────────────────────────────────────────┐
│ folio-desktop  (Tauri 2 host process — trusted, full user privileges)    │
│                                                                          │
│  ┌─────────────────────────┐   Tauri IPC (commands + events)             │
│  │ WebView: React UI       │◄──────────────────────────────┐             │
│  │  - Zustand store        │   folio:// URI scheme (tiles) │             │
│  │  - virtualized pages    │◄──────────────────────┐       │             │
│  └─────────────────────────┘                       │       │             │
│                                                    │       │             │
│  ┌──────────────────────────────────────────────────┴───────┴─────────┐  │
│  │ Broker (Rust): file access, dialogs, recent files, autosave,       │  │
│  │ tile cache (LRU, byte-bounded), job registry, cancellation,        │  │
│  │ supervisor (spawns / restarts child processes)                     │  │
│  └───────┬─────────────────────────────────────┬──────────────────────┘  │
└──────────┼─────────────────────────────────────┼─────────────────────────┘
           │ ipc-channel (framed messages,       │ ipc-channel + shared-memory
           │ fd/handle passing)                  │ tile buffers
┌──────────▼───────────────────────┐   ┌─────────▼──────────────────────────┐
│ folio-engine (sandboxed)         │   │ folio-render worker × N (sandboxed)│
│  - qpdf object graph per doc     │   │  - PDFium instance per process     │
│  - document model, commands,     │   │  - renders tiles/thumbnails        │
│    undo journal                  │   │  - text extraction, hit testing    │
│  - writer (incremental/full/lin.)│   │  - form appearance (Phase 3/5)     │
│  - optimizer, codecs (Phase 2)   │   └────────────────────────────────────┘
│  - search, text layout analysis  │
└──────────────────────────────────┘
```

**Why three kinds of process (ADR-004):**

- **PDFium is not thread-safe.** `pdfium-render`'s thread-safe mode serializes
  every call behind one global mutex. Real rendering parallelism therefore
  requires multiple PDFium *processes*. N = `clamp(cores/2, 1, 4)` by default.
- **Crash isolation.** A malformed file that crashes PDFium or qpdf kills a
  child process, not the window. The supervisor restarts it, reloads the
  document from the broker's bytes and shows a non-intrusive notice.
- **Sandboxing.** Children hold no ambient filesystem or network authority
  (§8). The broker opens files on the user's behalf and passes read-only handles
  or bytes.

The **CLI** (`folio`) links the engine and render crates directly, in process,
without sandboxing. It is a scripting tool running on files the user names
explicitly. An optional `--isolate` flag runs the same process split.

### 2.1 Document bytes and snapshots

The broker memory-maps the original file read-only and hands a handle to the
engine and each render worker. Render workers never see the engine's
in-memory object graph. After edits they receive a **render snapshot**:

```
snapshot = original bytes (shared mapping) + incremental delta (shared memory)
```

The delta is an in-memory incremental update: changed and new objects plus a
new xref section with `/Prev` pointing at the original. The engine produces it
with the same code as the real incremental writer. PDFium opens the
concatenation via `FPDF_LoadCustomDocument` with a reader that spans both
buffers. Reopening is lazy (PDFium parses the xref chain, then pages on
demand), so updating the view after an edit costs O(xref), not O(file). The
broker debounces snapshot republishing (≈50 ms) and invalidates only tiles of
pages whose objects changed. ADR-005 records this choice and its measured cost.

---

## 3. Repository layout

```
folio/
├── Cargo.toml                 # Rust workspace
├── package.json               # pnpm workspace root (frontend + tooling)
├── crates/
│   ├── folio-core/            # ids, errors, geometry, units, progress, cancellation
│   ├── qpdf-sys/              # vendored qpdf (C++) built via cmake; raw FFI
│   ├── folio-cos/             # safe wrapper: object graph, repair log, raw write
│   ├── folio-content/         # content-stream lexer/parser/interpreter/serializer
│   ├── folio-model/           # typed model: pages, resources, annots, forms, outlines…
│   ├── folio-ops/             # Command trait, object journal, undo/redo, history
│   ├── folio-writer/          # incremental / full / linearized save; atomic file replace
│   ├── folio-render/          # PDFium wrapper, tile math, render worker main()
│   ├── folio-text/            # extraction, search, layout analysis (blocks/paragraphs)
│   ├── folio-codecs/          # (P2) JPEG/JP2/JBIG2/CCITT/Flate encode, resampling
│   ├── folio-optimize/        # (P2) audit, presets, pipeline, target-size, verify
│   ├── folio-ipc/             # protocol types (serde) + TS type generation (ts-rs)
│   ├── folio-sandbox/         # per-OS sandbox setup for child processes
│   ├── folio-engine/          # engine process main(): doc actors, job scheduler
│   └── folio-cli/             # `folio` binary
├── apps/desktop/
│   ├── src-tauri/             # Tauri host = broker (Rust)
│   └── src/                   # React + TypeScript UI
├── fuzz/                      # cargo-fuzz targets
├── bench/                     # criterion benches + perf harness + compression benchmark
├── corpus/                    # manifest + download/verify scripts (files via LFS / cache)
├── tools/                     # license gate, THIRD_PARTY generation, golden-image tools
└── docs/
```

Crate dependency direction (acyclic, enforced by `cargo-deny`'s ban on
unexpected edges plus review):

```
core ← cos ← content ← model ← ops ← writer
                          ↑        ↑
                        text     optimize ← codecs
render (depends on core, ipc only — never on cos/model)
engine = model + ops + writer + text + optimize + ipc + sandbox
cli    = engine crates + render (in-process)
desktop/src-tauri = ipc + sandbox (supervisor side) + core
```

---

## 4. Layers

### 4.1 Parser / object layer — `qpdf-sys`, `folio-cos` (ADR-002)

- **qpdf is the primary object layer.** It parses, repairs, decrypts
  (RC4-40/128, AES-128, AES-256 R5/R6), and writes full rewrites with object
  streams, xref streams, linearization and encryption. It has the most mature
  repair logic among permissive-licensed libraries: broken or missing xref
  (it reconstructs by scanning), wrong offsets, bad `/Length`, missing
  `endobj`/`endstream`, truncation.
- It is built from vendored source with the **native crypto provider**, so no
  OpenSSL or GnuTLS. It is linked statically and reached through qpdf's C API
  (`qpdf-c.h`, `qpdf_oh_*` object-handle functions). qpdf ≥ 11 is required.
- `folio-cos` exposes a safe Rust API: `CosDoc`, `ObjRef`, `Object` (an enum
  view), dictionary/array/stream access, stream data get/replace, new-object
  creation and raw unparse. All qpdf warnings are captured into a
  **RepairLog** (`Vec<RepairEvent { kind, object, offset, message }>`). The UI
  shows the log as a dismissible banner ("This file was damaged and has been
  repaired — details").
- A `CosDoc` is `!Sync`. It is owned by exactly one **document actor** thread
  in the engine (§6).
- **What qpdf does not do**, and we build ourselves:
  - Incremental updates (`folio-writer`, §4.5).
  - A content-stream model (`folio-content`).
  - Anything above COS (`folio-model`).

### 4.2 Content streams — `folio-content`

A pure-Rust, fuzzed lexer, parser and interpreter for content streams:

- `Lexer` → tokens; `Parser` → `Vec<Op>` (operator + operands, with byte spans
  kept for diagnostics and the object inspector).
- An `Interpreter` tracks the graphics state (CTM, text state, colour spaces,
  clipping as geometry, marked content, optional content). It emits
  positioned events: glyph runs with font refs and user-space quads, image
  draws with their effective CTM, path paints, form XObject enter/leave.
- A `Serializer` writes ops back with configurable number precision (used by
  the optimizer and editing).

The same interpreter feeds effective-DPI computation (Phase 2), text-block
reconstruction (Phase 4), redaction (Phase 5) and the object inspector.

### 4.3 Document model — `folio-model`

Typed, lazily materialized views over COS:

- `Document`: catalog, page tree with inherited attributes resolved, page
  labels, name trees (dests, embedded files, JavaScript), outlines, AcroForm,
  StructTreeRoot, OCProperties, Info and XMP metadata, output intents, DSS,
  permissions and encryption summary, the signature list.
- `Page`: boxes (Media/Crop/Bleed/Trim/Art), rotation, resources, content
  (`folio-content`), annotations.
- The model never caches derived state across a mutation without an
  invalidation key. Each COS object has a generation counter bumped by the
  journal (§4.4).

### 4.4 Operations — `folio-ops`

```rust
pub trait Command: Send {
    fn describe(&self) -> LocalizedText;              // "Rotate pages 3–5 by 90°"
    fn apply(&mut self, cx: &mut EditContext) -> Result<ChangeSet>;
    fn revert(&mut self, cx: &mut EditContext, cs: &ChangeSet) -> Result<()>;
    fn params(&self) -> serde_json::Value;            // for macro recording / Action Wizard
}
pub struct ChangeSet {
    pub before: Vec<(ObjRef, Option<RawObject>)>,     // None = object was created
    pub after:  Vec<ObjRef>,
    pub affected_pages: PageSet,
    pub repaint: RepaintHint,
}
```

- `EditContext` records every object touched through it, capturing a
  **before-image** (raw serialized object plus stream bytes) at first touch.
  So `revert` has a correct generic default: restore the before-images.
  Commands override it only for efficiency.
- `History` holds the undo and redo stacks with a memory budget; old entries
  spill to the autosave directory. Each entry = command params + ChangeSet.
- The set of objects dirty since load (or since the last save) is exactly
  what the incremental writer and render snapshots need.
- Batch/macro: a `CompositeCommand` wraps many commands into one undo step.
  `params()` + a registry `name → constructor` make every command replayable
  from JSON (Action Wizard, Phase 9).

### 4.5 Writer — `folio-writer`

Three save modes, all ending in the same **atomic replace** routine:

| Mode | Implementation | Used for |
|---|---|---|
| Incremental | Own Rust: append dirty/new objects serialized via qpdf unparse; new xref **table** if the original used tables, else xref **stream**; `/Prev` chain; encrypt appended objects with the document key if the doc is encrypted (RustCrypto) | Default save when signatures exist; always available; render snapshots |
| Full rewrite | qpdf `QPDFWriter` (object streams, xref streams, compress streams, preserve or strip encryption) | Default "Save" when there are no signatures and changes are large; optimization; sanitize; redaction |
| Linearized | qpdf linearization | "Fast Web View" option; compression output option |

Atomic replace:
1. Write to `<dir>/.<name>.folio-tmp-<rand>`.
2. `fsync` it (`F_FULLFSYNC` on macOS).
3. Re-open the temp file and validate it: qpdf parse check, plus a render of
   page 1 in a worker.
4. Atomically replace the target (`rename` on Unix, `ReplaceFileW` on
   Windows), preserving permissions, extended attributes and ACLs.
5. `fsync` the directory.

On any failure the temp file is removed and the original is untouched.

### 4.6 Rendering — `folio-render` + broker tile cache

- **Tile grid:** 512×512 device pixels. Zoom is bucketed to powers of √2
  between buckets. Tiles are rendered at the bucket scale and the GPU scales
  them to the exact zoom, which avoids re-rendering on every zoom tick.
- **Progressive rendering:** each page gets a low-res preview at a scale of
  ≈1/8 (cached as a thumbnail too), then sharp tiles for visible regions, then
  prefetch for ±1 viewport.
- **Priority queue** in the broker, ordered: visible tiles for the current
  zoom > previews for visible pages > prefetch > thumbnails pane. Stale
  requests (scrolled away, zoom changed) are cancelled before dispatch. In
  flight, PDFium's progressive render API (`FPDF_RenderPageBitmap_Start` +
  pause callback) checks a cancellation flag.
- **Caches** (all LRU, byte-bounded, sized from system RAM with a default
  total of 256 MB):
  - L1: decoded RGBA tiles in the broker.
  - L2: previews and thumbnails, compressed with QOI, in memory.
  - L3 (optional): an on-disk thumbnail cache keyed by file hash.
  - Each render worker keeps PDFium's own page cache limited to recently used
    pages.
- **Transport to UI:** a `folio://tile/{doc}/{snapshot}/{page}/{bucket}/{tx}/{ty}`
  custom URI scheme returns raw RGBA bytes with width, height and stride in
  headers. The UI does
  `fetch → ImageData → createImageBitmap → drawImage` onto a per-page canvas.
  No base64, no PNG encode on the hot path. Snapshot ids in the URL make
  caching trivially correct.
- **Text layer:** for selection, search highlights and screen readers, each
  visible page gets an invisible, positioned DOM text layer (char boxes from
  PDFium's text API), like PDF.js.

### 4.7 Text — `folio-text`

- Extraction and hit testing via PDFium (`FPDFText_*`) in render workers.
- Search runs in the engine or workers with normalization: NFKC, ligature
  expansion, soft-hyphen and line-end hyphen joining, optional diacritic
  folding, case folding, bidi-aware logical order.
- Results stream back as `SearchHit` events per page.
- Folder catalogue index (Phase 1 stretch / Phase 9): `tantivy`, stored under
  app data.
- Layout analysis (text blocks, paragraphs, columns, reading order) lives here
  and is built on `folio-content` events. Phases 4, 6, 7, 8 use it.

### 4.8 UI — `apps/desktop/src`

- React 18 + TypeScript (strict), Vite, Zustand. Separate slices for
  documents, view state, tools and jobs. The store is the single source of
  truth, and views are pure functions of it.
- Accessible primitives from Radix UI. Virtualization with TanStack Virtual.
  i18n with i18next (ICU message format; RTL via logical CSS properties and a
  `dir` switch).
- **Design tokens** (CSS custom properties) for colour, spacing, type and
  radius, with light, dark and high-contrast themes. The icon set is
  original, hand-built SVG in `apps/desktop/src/icons`.
- **Layout:** top contextual toolbar · left nav pane (thumbnails, bookmarks,
  …) · centre document views (tabs, split) · right tool pane · status bar.
  Command palette (⌘/Ctrl-K) and a Tools hub page.
- **IPC client:** generated TS types (`ts-rs`) plus a thin `engine` module.
  Every request carries a `RequestId`. Long jobs return a `JobId`; progress and
  results arrive via Tauri events. `cancel(jobId)` is always available.

### 4.9 IPC — `folio-ipc`

- **UI ⇄ broker:** Tauri commands (JSON), Tauri events (progress, results,
  document-changed) and the `folio://` scheme for binary data.
- **Broker ⇄ children:** `ipc-channel` with `serde` + `postcard` framing.
  Large payloads (tiles, image bytes) go through shared-memory regions;
  handles to the original file are passed natively (SCM_RIGHTS or
  DuplicateHandle).
- **Messages:** `Request { id, doc, body }`, `Response { id, result }`,
  `Event::Progress { job, done, total, stage }`, `Event::Log`, `Cancel { job }`.
- **Cancellation:** a `CancelToken` (an `Arc<AtomicBool>` plus a deadline)
  is checked at every loop and stage boundary. Cancelling a mutating job
  aborts before its ChangeSet is committed, so the document is never
  half-edited.
- The protocol is versioned. The broker refuses a child with a mismatched
  version (both ship in the same bundle, so this only catches packaging bugs).

---

## 5. Data flows

**Open:**
1. The UI calls `openFile(path)`. The broker mmaps the file, hashes the first
   and last MiB for a quick identity check, and registers the document.
2. The engine loads it: qpdf parse and repair, model summary.
3. In parallel, the render pool loads the file: page count, page 1 sizes, then
   page 1 preview and tiles. This is the 500 ms target path; it does not wait
   for the engine.
4. The engine returns metadata, outline and the repair log. The UI fills the
   panes.

**Edit:**
1. UI action → engine `Execute(command)`.
2. The doc actor applies it: ChangeSet, history push, new render snapshot delta.
3. The broker bumps the snapshot id, invalidates tiles for the affected pages
   and tells the UI.
4. The UI re-requests only visible invalidated tiles.

**Save:**
1. The engine picks a mode (§4.5, or the user's choice).
2. It writes to the temp path, validates, atomically replaces, and marks
   history clean.
3. The file watcher ignores the self-caused change event.

**Compress (Phase 2):**
1. Audit → plan (per-object actions from preset) → parallel execution on a
   rayon pool (engine) → reassemble on the doc actor.
2. Full rewrite to a new file.
3. Verify: re-parse, render at low DPI in the workers, SSIM against the
   original renders.
4. Report.

---

## 6. Threading model

| Where | Threads |
|---|---|
| UI (WebView) | Main JS thread only renders; decoding of tile bytes happens off-main via `createImageBitmap` |
| Broker | Tokio runtime (IPC, file I/O, watchers), plus one blocking pool for mmap/hash |
| Engine | One **document actor** thread per open document (owns `CosDoc`, serial command queue) · a shared **rayon** pool for CPU work (image codecs, hashing, search, optimization) — work is extracted from the actor as owned data, processed in parallel, and results applied back on the actor |
| Render workers | Single-threaded each (PDFium constraint); parallelism = number of processes |

Invariants:

- `CosDoc` is only touched on its actor.
- No lock is held across IPC.
- All long work is chunked with cancellation checks at most ~50 ms apart.

---

## 7. Persistence, recovery, change detection

- **App data dir** (platform-specific via Tauri paths):
  - `recovery/`, `thumbs/`, `index/`, `presets/`, `stamps/`, `actions/`.
  - `settings.json`: versioned, migrated on load.
- **Autosave** (default every 5 min, configurable, only when dirty):
  - Writes `recovery/<doc-id>/delta.pdfu` (the incremental delta, i.e. the
    exact bytes the incremental writer would append) plus `meta.json`
    (original path, original size, mtime, content hash, history descriptions).
  - Recovery = original bytes + delta. The recovered document keeps its undo
    labels but not full undo, which is documented.
- **Crash recovery:** on launch the broker scans `recovery/`. If the original
  still matches its recorded hash, it offers "Restore unsaved changes" in a
  dialog. If the original changed, it offers "Open recovered copy".
- **External changes:** a file watcher (`notify` crate) plus an mtime/size
  check on focus. If the file changed on disk while open, a banner offers
  "Reload" or "Keep mine (Save As)".
- The original file is never written except by an explicit Save or Save As.

---

## 8. Security architecture

Threat: a malicious PDF exploiting a memory-safety bug in qpdf (C++), PDFium
(C++) or codecs (C) to read or write the user's files or exfiltrate data.

| Control | Implementation |
|---|---|
| Least privilege processes | Engine and render workers run sandboxed; only the broker has filesystem, dialog and network access |
| macOS | Children are launched with a Seatbelt profile via `sandbox_init` (deny default; allow reading the bundle and a per-child temp dir; no network; no file write outside temp). App Sandbox for the bundle as a whole is evaluated separately for Mac App Store distribution (not in scope) |
| Windows | Restricted token + low integrity level + Job object (no child processes, UI restrictions, memory cap); AppContainer evaluated in Phase 9 |
| Linux | `landlock` (fs allowlist) + `seccomp-bpf` (syscall allowlist, no `socket`/`connect`) + `PR_SET_NO_NEW_PRIVS`; user/net namespaces when available |
| File access | Brokered: children receive already-open read-only handles or bytes; writes go through the broker's atomic-save routine using bytes produced by the engine |
| PDF JavaScript | Disabled by default. When enabled (Phase 5), it runs in QuickJS (`rquickjs`) in the engine sandbox with only the form-related Acrobat JS API surface (`AFNumber_*`, `AFDate_*`, `event`, `this.getField`, …); no `app.launchURL`, no network, no filesystem, CPU and memory limits per event |
| Actions | Launch, URI, GoToR and embedded-file-open actions are surfaced to the broker, which always prompts with the target shown |
| Secrets | Passwords and private keys are kept in zeroizing buffers (`zeroize`), never logged (the logging layer has a `Secret<T>` type that redacts), and persisted only via the OS keychain on opt-in |
| Fuzzing | cargo-fuzz targets for `folio-content`, `folio-cos` load (qpdf), font loaders, codecs; nightly CI job + OSS-Fuzz application once public |
| Supply chain | `cargo-deny` (licenses, advisories, sources), `cargo-audit`, `pnpm audit`, lockfiles committed, vendored C/C++ sources pinned by hash |

The sandbox is introduced in Phase 1 as the process split plus the macOS and
Linux profiles. The Windows restricted token lands in Phase 1 if the CI runner
permits testing it, otherwise early Phase 2. Either way this is tracked in the
roadmap.

---

## 9. Build, CI, release

- **Toolchains:** Rust stable (pinned in `rust-toolchain.toml`), Node 22 LTS
  or newer, pnpm, CMake + a C++17 compiler for qpdf and codecs.
- **PDFium:** prebuilt binaries from `bblanchon/pdfium-binaries` (PDFium
  license), pinned by version and SHA-256, downloaded by a build script into
  `target/pdfium/` and bundled as a Tauri resource. The V8/XFA builds are not
  used (ADR-006).
- **CI** (GitHub Actions matrix: `macos-14` arm64, `macos-13` x64,
  `windows-latest` x64, `windows-11-arm`, `ubuntu-24.04` x64):
  - fmt, clippy `-D warnings`, `tsc --noEmit`, ESLint, Prettier check.
  - Unit and integration tests.
  - License gate: `cargo-deny` + a JS license checker. Fails on any license
    not in the allowlist (ADR-001).
  - `THIRD_PARTY_LICENSES.md` is regenerated (`cargo-about` + JS) and a diff
    fails CI.
  - Interop checks: qpdf `--check`, Poppler `pdftoppm`, PDF.js (Node).
    Test-time tools only, never distributed.
  - Benchmarks (§10).
  - Nightly fuzzing.
- **E2E:** WebDriver via `tauri-driver` works on Windows and Linux only, so
  full-app E2E runs there. On macOS, Playwright drives the UI in a browser
  against a real engine exposed through a dev-only WebSocket bridge. No mocked
  engine behaviour.
- **Release:** `tauri build` produces `.dmg`/`.app` (universal or per arch),
  `.msi` + NSIS `.exe`, AppImage, `.deb`, and Flatpak (manifest in
  `packaging/flatpak`). Signing and notarization need certificates the
  project does not yet have; release jobs are wired but skip signing when
  secrets are absent. Reproducibility: pinned toolchains, `SOURCE_DATE_EPOCH`,
  and `--locked`.

---

## 10. Performance measurement

- `bench/perf`: a headless harness that drives the engine and render workers
  exactly as the app does. It measures:
  - time-to-first-page (20-page and 2,000-page files);
  - full-text search time (1,000 pages);
  - peak RSS across processes (1,000-page image-heavy file);
  - compression wall time (100-page 300 dpi colour scan).
- Scrolling fps is measured in E2E with a scripted scroll and the Performance
  API frame timings, on Windows and Linux runners.
- Hosted CI runners are noisy. CI tracks **trends** (fail on >20% regression
  vs. the rolling median), while the absolute targets in spec §4.2 are
  verified on a reference machine (this Mac, M-series) and recorded in each
  phase report.
- Compression benchmark (Phase 2): fixed file set; output bytes, SSIM and
  time per commit, stored as a JSON artifact and a markdown summary. CI fails
  if total size grows by more than 2% or mean SSIM drops by more than 0.005
  vs. the committed baseline.

---

## 11. Dependencies and licenses

Product license: **MIT OR Apache-2.0** (ADR-001). Allowed dependency licenses:
MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0 (file-level copyleft,
compatible), Unicode-3.0, FTL, IJG, BSL-1.0, CC0. Disallowed: GPL, AGPL, and
LGPL unless dynamically loaded at runtime and optional (ADR-008).
**Banned by name:** MuPDF, Ghostscript, jbig2dec, dssim (AGPL), Poppler as a
linked library.

### 11.1 Rust / native (shipped)

| Dependency | Purpose | License | Phase |
|---|---|---|---|
| Tauri 2 (+ plugins: dialog, fs-scope, window-state, updater) | App shell | MIT/Apache-2.0 | 1 |
| qpdf (vendored, C++) | Object layer, repair, full write, linearize, encrypt | Apache-2.0 | 1 |
| zlib-ng / zlib-rs | Flate for qpdf and ours | Zlib | 1 |
| libjpeg-turbo (qpdf dep) | DCT decode in qpdf | IJG/BSD-3/Zlib | 1 |
| PDFium (prebuilt, no V8/XFA) | Rendering, text, forms | BSD-3 / Apache-2.0 | 1 |
| pdfium-render | PDFium bindings | MIT/Apache-2.0 | 1 |
| ipc-channel | Cross-process channels, shmem | MIT/Apache-2.0 | 1 |
| serde, postcard, serde_json | Serialization | MIT/Apache-2.0 | 1 |
| ts-rs | TS type generation | MIT | 1 |
| tokio, rayon, crossbeam | Async and parallelism | MIT/Apache-2.0 | 1 |
| notify | File watching | CC0/MIT/Apache | 1 |
| tracing (+ subscriber) | Logging with redaction | MIT | 1 |
| thiserror, anyhow (bins only) | Errors | MIT/Apache-2.0 | 1 |
| unicode-normalization, unicode-bidi, unicode-segmentation | Search normalization | MIT/Apache-2.0 | 1 |
| qoi | Preview cache compression | MIT/Apache-2.0 | 1 |
| landlock, seccompiler | Linux sandbox | MIT/Apache-2.0 | 1 |
| windows (windows-rs) | Win32 APIs (sandbox, ReplaceFileW, WIC, stores) | MIT/Apache-2.0 | 1 |
| objc2 family | macOS APIs | MIT | 1 |
| zeroize, secrecy | Secret hygiene | MIT/Apache-2.0 | 1 |
| clap | CLI | MIT/Apache-2.0 | 1 |
| mozjpeg (libjpeg-turbo fork) + `mozjpeg` crate | JPEG encode (trellis, optimized Huffman) | IJG/BSD-3/Zlib | 2 |
| OpenJPEG + `openjpeg-sys` | JPEG 2000 encode/decode | BSD-2 | 2 |
| jbig2enc (vendored) + Leptonica | JBIG2 encode (generic, symbol) | Apache-2.0 / BSD-2 | 2 |
| fax | CCITT G3/G4 encode/decode | MIT | 2 |
| image | Pixel buffers, PNG/TIFF/WebP/BMP/GIF I/O | MIT/Apache-2.0 | 2 |
| fast_image_resize | Bicubic/bilinear/area/Lanczos resampling (SIMD) | MIT/Apache-2.0 | 2 |
| zopfli | Optional max-effort Flate | Apache-2.0 | 2 |
| blake3 | Content hashing for dedup | CC0/Apache-2.0 | 2 |
| lcms2 (Little CMS) | ICC colour conversion | MIT | 2 |
| subsetter, allsorts | Font subsetting | MIT/Apache-2.0 · Apache-2.0 | 2 |
| ttf-parser, rustybuzz, fontdb | Font parsing, shaping, system font discovery | MIT/Apache-2.0 · MIT · MIT | 4 |
| spellbook | Hunspell-compatible spell check (Linux fallback) | MPL-2.0 | 4 |
| rquickjs (QuickJS) | Sandboxed form JavaScript | MIT | 5 |
| RustCrypto (aes, cbc, sha2, rsa, p256, p384, cms, x509-cert, der, x509-ocsp) | Encryption, signatures | MIT/Apache-2.0 | 5, 7 |
| cryptoki | PKCS#11 tokens | Apache-2.0 | 7 |
| security-framework | macOS Keychain | MIT/Apache-2.0 | 7 |
| ureq + rustls + webpki-roots | TSA/OCSP/CRL fetch (user-initiated only) | MIT/Apache-2.0 · ISC/Apache/MIT · MPL-2.0 | 7 |
| Tesseract 5 (vendored) | OCR | Apache-2.0 | 6 |
| docx-rs, rust_xlsxwriter | DOCX/XLSX export | MIT · MIT/Apache-2.0 | 6 |
| tantivy | Folder search index | MIT | 9 |
| tts | Read Aloud via OS TTS | MIT | 1/9 |

### 11.2 Frontend (shipped)

React, React DOM, Zustand, TanStack Virtual, Radix UI primitives, i18next +
react-i18next, clsx: all MIT. Tooling (not shipped): Vite, Vitest,
Playwright, ESLint, Prettier, TypeScript (MIT/Apache-2.0).

### 11.3 Optional runtime integrations (not bundled, detected at runtime)

| Tool | Use | License | Notes |
|---|---|---|---|
| LibreOffice (headless) | Office → PDF | MPL-2.0 | Subprocess; guidance shown if absent |
| veraPDF | PDF/A, PDF/UA validation | GPL-3.0+ / MPL-2.0 dual | Subprocess (Java); used under MPL; in-app rule checks do not depend on it |
| libheif | HEIC on Linux | LGPL-3.0 | `dlopen` only if installed; macOS uses ImageIO, Windows uses WIC |
| SANE | Scanning on Linux | GPL + linking exception | Runtime `dlopen`/`scanimage` subprocess, never linked |

### 11.4 Test-only (never distributed)

Poppler utils (GPL), PDF.js (Apache-2.0), pyHanko (MIT), pikepdf (MPL-2.0),
veraPDF, EU DSS (LGPL; CI container only).

---

## 12. Known hard problems and how the design addresses them

| Problem | Approach |
|---|---|
| Two parsers (qpdf for editing, PDFium for display) could disagree on damaged files | The engine normalizes: when repair occurred, the render snapshot base is qpdf's **repaired rewrite**, not the raw original, so both see the same objects. Recorded in RepairLog |
| Edit latency with PDFium reload | Incremental in-memory snapshot (§2.1), page-scoped tile invalidation; measured in Phase 1, with a fallback option of PDFium's own page-edit API for live previews during drags |
| Paragraph text editing in arbitrary PDFs | Phase 4; built on `folio-content` + `folio-text` layout analysis + rustybuzz; prototypes in Phase 1–3 to de-risk |
| PDFium without V8 still needs form JS | Our QuickJS runtime is driven from form events; PDFium's form-fill environment is used for appearance/interaction only (Phase 5) |
| Lossy JBIG2 symbol substitution can change characters (the Xerox 6/8 bug) | Lossy JBIG2 text-region mode is off in every preset except Maximum, always shows a warning, and uses conservative matching thresholds |
| Hosted-runner benchmark noise | Trend-based CI gates + reference-machine absolute numbers (§10) |
