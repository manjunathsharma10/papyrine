# Papyrine — Architecture

Status: **Draft v3** (direction approved by the owner; revision 3 applies
the second review) · Last updated: 2026-09-30

**Product name:** **Papyrine** (ADR-019).
- Crates: `papyrine-*`.
- CLI: `papyrine`.
- Bundle ID: `io.github.manjunathsharma10.papyrine`.
- Repo: `github.com/manjunathsharma10/papyrine`.

This document describes processes, crates, data flow, threading, persistence,
security boundaries, optional components and dependencies. Choices are
justified in [DECISIONS.md](DECISIONS.md) (ADR-NNN).

**Revision 2:**
- Lightweight budgets became hard CI gates.
- Optional components.
- One render worker.
- Snapshot compaction.
- The qpdf shim.
- A change-driven journal.
- HEIC via OS decoders.
- License gate and notices.

**Revision 3:**
- Budgets accepted (§1.1).
- A large-document memory gate (§1.2).
- Write-ahead journal in the host (§7).
- qpdf/PDFium consistency under the command-mirroring fallback (§4.6).
- qpdf crypto provider pinned to native (§4.1, §12).
- Opt-in security update check (§9.2).
- Signing-key storage and rotation (§9.3).
- Helper notarization (§9.4).
- Per-platform printing plan (§11.2).
- The Windows HEVC message (§11.4).

---

## 1. Product direction and lightweight budgets

Papyrine is **simple and lightweight while feature-rich.** The spec's
Acrobat-class scope stays the long-term target. Where the spec and these
budgets conflict, the budgets win (ADR-010).

### 1.1 Hard budgets (CI regression gates)

| Budget | Target | Measured how | CI gate |
|---|---|---|---|
| Installer size (base, per platform/arch) | **≤ 50 MB** for `.dmg` (per-arch, not universal), `.msi`/NSIS, `.deb`, `.rpm`, Flatpak bundle · **AppImage ≤ 100 MB** (it bundles WebKitGTK). Internal target **≤ 30 MB** to keep headroom | Size of the release artifact | Fail above the budget, or if a PR grows an artifact > 5% vs `main` without a `size-increase-approved` label |
| Cold launch → first rendered page | **< 1.0 s** on the reference Mac (this machine, Apple Silicon) · warm **< 0.5 s** | Launch with a typical doc as an argument; the timestamp from process start to the UI's first-tile-painted event. "Cold" = after `sudo purge`, excluding the first-ever Gatekeeper verification | Absolute check on the reference machine each milestone. CI (macOS runner) fails on > 15% regression vs the rolling median, and on an absolute > 2.0 s |
| Idle memory, one typical doc open | **≤ 150 MB** macOS · **≤ 175 MB** Linux · **≤ 200 MB** Windows (accepted) | Sum over **all** app processes, including webview helpers. macOS `phys_footprint`, Linux PSS, Windows private working set. Sampled 10 s after first paint with no interaction | Absolute gate on the macOS and Linux runners; trend gate on Windows |
| Large-document memory (§1.2) | **≤ 400 MB** peak and **≤ 250 MB** settled (idle 10 s after a full scroll) across all processes, for each large benchmark file; engine and renderer each **≤ 120 MB** settled | Same accounting as idle memory, sampled during and after a scripted full scroll, a search of the whole document, and 50 edits | Absolute gate on macOS and Linux; trend on Windows |
| Startup cost per feature | **Zero** work before first paint for non-core features | A startup trace (`tracing` spans) records subsystem initialization before first paint | Fail if any subsystem not on the startup allowlist initializes before first paint. Initial JS bundle ≤ **200 KB gzipped** (fail above); any single lazy chunk ≤ 150 KB gzipped (warn) |

"Typical document" = the benchmark file `typical-20p.pdf`: 20 pages, mixed
text and images, ~4 MB (generated, §14).

### 1.2 Budget notes and the large-document memory test

**Accepted by the owner:**
- Idle memory of 200 MB on Windows and 175 MB on Linux. The webview process
  floor is WebView2 at ~80–120 MB and WebKitGTK at ~70–100 MB.
- AppImage ≤ 100 MB.
- `.deb` and `.rpm` stay ≤ 50 MB.

The optional Windows "offline" installer (it bundles a fixed WebView2
runtime, ~180 MB) is exempt and labelled as such. Spike 0.1 measures the bare
shell on all three OSes. If the bare shell alone breaks a budget, I stop and
report.

**Large-document memory.** qpdf (engine) and PDFium (renderer) each open the
same document, so a naive design pays twice. Mitigations:
- **No heap copy of the file in either process.** Both read through the
  host's shared read-only mmap:
  - qpdf via a custom `InputSource` (not `qpdf_read_memory`);
  - PDFium via `FPDF_LoadCustomDocument`.
  Clean file-backed pages are shared by the OS and excluded from
  `phys_footprint`/private working set.
- **qpdf holds parsed objects, not stream data.** Stream bytes are read from
  the input source on demand. The engine only resolves objects it needs
  (model summaries, commands). Search and rendering are PDFium's job.
- **PDFium caches are capped:** the page cache (8 pages) and the image-decode
  cache.
- The render snapshot sections (§4.6) count toward the renderer's budget, and
  L2 compaction caps them.

**Benchmark files** (generated at test time):
- `large-2000p-text.pdf` (~40 MB);
- `large-1000p-images.pdf` (~450 MB, 300 dpi colour);
- `large-objects.pdf` (≥ 1.5 M objects, stress-testing qpdf's object
  table);
- `large-single-page.pdf` (one 5 m × 5 m engineering drawing, stress-testing
  PDFium's tile path).

The test runs this script: open → scripted full scroll → whole-document
search → 50 edits → idle 10 s. It samples every process at 10 Hz and asserts
the §1.1 budgets. Per-process peaks are reported so we can see which copy
grows. If qpdf's object table dominates on `large-objects.pdf`, the fallback
is to open the engine's `QPDF` lazily per operation for view-only documents.
This is decided from the measurement, not assumed.

### 1.3 Size and startup estimate for the base install (to be verified in Spike 0.1)

| Part | Est. on disk | Notes |
|---|---|---|
| Single multi-role executable (host + engine + renderer + CLI roles) | 18–24 MB | qpdf, mozjpeg, OpenJPEG, jbig2enc+Leptonica (generic region), lcms2, Rust code. One binary avoids linking qpdf/codecs twice (ADR-004) |
| PDFium (no V8, no XFA) | 6.9 MB | 3 MB compressed; measured (chromium/8076, mac-arm64) |
| Frontend assets | 1.5–2.5 MB | Code-split; ~200 KB loaded at start |
| Base fonts (Latin/Greek/Cyrillic sans, serif, mono for typing and annotations) | ~2 MB | OFL; CJK and other scripts are a component (§9) |
| Notices, ICC sRGB, CMaps | < 1 MB | |
| **Total** | **~30–36 MB** on disk → **~13–18 MB** compressed installer | Well inside 50 MB |

### 1.4 Simple by default: tool tiers

Every tool is registered with a **tier** in the tool registry:

- **Core** — on the main toolbar: View (zoom/modes), Search, Annotate, Fill &
  Sign, Organize Pages, Compress, Save. Each appears only once its milestone
  ships; there are no placeholders.
- **Standard** — in the **All Tools** panel, grouped (Edit, Protect, Convert,
  Scan & OCR, Forms, Compare, Print Production, Accessibility, Automation,
  Measure).
- **Advanced** — also in All Tools, under an "Advanced" disclosure, and in the
  command palette: preflight, output preview, object inspector, XMP raw
  editor, JBIG2 lossy, etc.

The **command palette** (⌘/Ctrl-K) reaches every tool and command by name.
Users can pin any tool to the toolbar. Right-pane tool options show only the
common settings by default, with an "More options" disclosure for the rest.

---

## 2. Process model

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ papyrine (role=host) — Tauri 2, trusted, user privileges                     │
│  WebView: React UI ◄── Tauri IPC (commands/events) + papyrine:// (tiles)     │
│  Broker: file access, dialogs, JOURNAL WRITER (write-ahead), tile cache,     │
│          job registry, cancellation, supervisor, component manager, updater  │
└──────┬───────────────────────────┬──────────────────────────┬────────────────┘
       │ ipc-channel               │ ipc-channel + shmem      │ ipc-channel
┌──────▼────────────────────┐ ┌────▼─────────────────────┐ ┌──▼─────────────────────────┐
│ papyrine (role=engine)    │ │ papyrine (role=render)   │ │ component helpers          │
│ sandboxed                 │ │ sandboxed, ONE process   │ │ (optional, on demand,      │
│ qpdf docs via shim,       │ │ PDFium on one thread:    │ │  sandboxed): ocr, mrc,     │
│ model, commands,          │ │ tiles, text, hit-test,   │ │  form-scripts…             │
│ after-images, writer,     │ │ form widgets             │ │ spawned at first use,      │
│ optimizer                 │ │                          │ │ exit after idle timeout    │
└───────────────────────────┘ └──────────────────────────┘ └────────────────────────────┘
```

- **One executable, several roles** (like Chromium). The host re-executes
  itself with `--role=engine` or `--role=render`. The CLI is the same binary
  in `cli` role (a `papyrine` symlink or shim).
- **One render worker to start** (ADR-004). PDFium is single-threaded, so
  rendering is serialized, but a single worker with a priority queue and
  cancellation is enough for one visible viewport. Adding workers is decided
  by benchmark: only if p95 tile latency on the 2,000-page and image-heavy
  benchmarks misses its target *and* the memory budget still holds with a
  second worker. The render pool code supports N workers, but the default is
  N = 1. Thumbnails and search share the worker at lower priority.
- **Crash isolation:** if the engine or renderer crashes, the supervisor
  restarts it. The renderer reloads from the current snapshot. The engine
  **replays the journal** (§7), so no committed edit is lost.
- **Lazy start:** at launch only the host and the renderer start, because the
  renderer is on the first-page path. The engine starts in parallel but does
  not block first paint. Component helpers start only when their feature is
  first used.

### 2.1 Document bytes and render snapshots

The broker memory-maps the original file read-only and passes the handle to
the engine and the renderer. The renderer never sees the engine's object
graph. It renders a **render snapshot**:

```
snapshot = base (mmap) ‖ section_1 ‖ section_2 ‖ … ‖ section_k    (sections in shared memory)
```

Each committed command appends one small incremental section (the changed
objects plus an xref section with `/Prev`). The renderer re-opens through a
custom `FPDF_FILEACCESS` reader that spans the buffers. Compaction keeps `k`
and the total section size bounded (§4.6). **Benchmark B1 measures this
before anything depends on it** (ROADMAP task 0.2).

---

## 3. Repository layout

```
papyrine/
├── Cargo.toml / package.json (pnpm)   # workspaces
├── crates/
│   ├── papyrine-core/        # ids, errors, geometry, units, progress, cancellation, startup trace
│   ├── qpdf-sys/          # vendored qpdf + C++ shim (cxx bridge) — §4.1
│   ├── papyrine-cos/         # safe Rust API over the shim; RepairLog; object serializer
│   ├── papyrine-content/     # content-stream lexer/parser/serializer (+ interpreter, v0.2)
│   ├── papyrine-model/       # typed model: pages, annots, AcroForm, outlines, …
│   ├── papyrine-ops/         # Command trait, EditContext, ChangeSet, History, journal records
│   ├── papyrine-writer/      # incremental sections, ID-preserving full write, qpdf rewrite, atomic replace
│   ├── papyrine-render/      # PDFium wrapper, tile math, render role main()
│   ├── papyrine-text/        # search normalization; layout analysis (later)
│   ├── papyrine-codecs/      # (v0.2) encoders/resampling
│   ├── papyrine-optimize/    # (v0.2) audit, presets, pipeline, verify
│   ├── papyrine-components/  # manifest, signature verification, install, helper launch
│   ├── papyrine-ipc/         # protocol types; TS generation (ts-rs)
│   ├── papyrine-sandbox/     # per-OS child sandboxes
│   ├── papyrine-engine/      # engine role main(): doc actors, job scheduler
│   └── papyrine-app/         # the single binary: role dispatch, CLI (clap)
├── apps/desktop/          # Tauri config + src-tauri (host role) + src (React UI)
├── components/            # sources/build recipes for optional components
├── third_party/           # vendored native sources + native.toml (license manifest)
├── fuzz/  bench/  tools/  docs/
└── corpus/                # manifest.toml + fetch scripts ONLY (files never committed, ADR-017)
```

The crate dependency direction is acyclic:
`core ← cos ← content ← model ← ops ← writer`. `render` depends only on
`core` and `ipc`.

---

## 4. Layers

### 4.1 Object layer: how Rust talks to qpdf (ADR-002)

**Crypto provider: native only.** qpdf can be built with native (qpdf's
own, Apache-2.0), OpenSSL or GnuTLS (LGPL) crypto. We build with:

```
-DUSE_IMPLICIT_CRYPTO=OFF -DREQUIRE_CRYPTO_NATIVE=ON
-DREQUIRE_CRYPTO_OPENSSL=OFF -DREQUIRE_CRYPTO_GNUTLS=OFF -DDEFAULT_CRYPTO=native
```

Why native rather than OpenSSL:
- qpdf only uses crypto for PDF encryption (RC4, AES, MD5, SHA-2) on local
  files, where native is adequate.
- It avoids a ~5 MB dependency and its per-platform build and patching.
- Our own new cryptography (incremental-section encryption, and signatures in
  v0.3) uses RustCrypto, not qpdf.

The gate that enforces this:
- a test calls the shim's `QPDFCryptoProvider::getRegisteredImpls()` and
  asserts it returns exactly `["native"]`;
- bundle inspection fails on `libgnutls*`, `libssl*` and `libcrypto*`;
- the vendored-build CMake cache is checked in CI (§12).

**Mechanism:** qpdf's **C API** where it is sufficient, plus a thin **C++
shim** compiled into `qpdf-sys` and bridged with the [`cxx`] crate (typed,
safe bindings with no hand-written `extern "C"` glue). Every shim function
catches all C++ exceptions and returns `Result<T, QpdfError>`. No exception
ever crosses into Rust.

**What the C API covers** (qpdf 12, checked against `qpdf-c.h`):
- read from file or memory with a password; attempt recovery; warnings;
- object handles: get and replace dictionary keys; arrays; all scalar types;
  stream data get and replace;
- new objects, `make_indirect_object`, `replace_object`;
- `copy_foreign_object` for cross-document copying;
- page list add, remove and find, and pushing inherited attributes;
- the writer: object-stream mode, stream compression, linearization,
  R2–R6 encryption parameters, preserve encryption;
- a progress reporter; JSON import/export.

**What the C API cannot do, and the shim adds:**

| Need | C++ API used by the shim |
|---|---|
| Object renumbering map after a full rewrite (history rebasing, §4.5) | `QPDFWriter::getRenumberedObjGen` |
| Document-level encryption key and per-object keys, for encrypting incremental sections | `QPDF` encryption-parameter accessors (`getEncryptionKey`-family; exact calls pinned in Spike 0.2) |
| Lazy or large stream replacement without copying through a C buffer | `StreamDataProvider` subclass that pulls from a Rust callback |
| Helpers that would take months to replicate: AcroForm, annotation flattening and appearance-stream generation, outlines, page labels, embedded files, name/number trees | `QPDFAcroFormDocumentHelper`, `QPDFPageObjectHelper`, `QPDFOutlineDocumentHelper`, `QPDFPageLabelDocumentHelper`, `QPDFEmbeddedFileDocumentHelper`, `QPDFNameTreeObjectHelper`, `QPDFNumberTreeObjectHelper` — exposed as coarse operations, not full class wrappers |
| Structured warning capture with object and offset (the RepairLog) | `QPDFLogger` subclass + `QPDFExc` fields |
| Custom input source over an mmap without copying | `InputSource` subclass |
| Cheap object fingerprint for journal and consistency checks | `unparseBinary` + stream raw data hashing, done in C++ to avoid round trips |

Content streams are **not** parsed with qpdf's tokenizer. `papyrine-content` is
our own pure-Rust, fuzzed parser.

**Threading:** a `QPDF` instance is not thread-safe. Each open document has
one **document actor** thread in the engine that owns its `QPDF` and every
handle derived from it. Handles are `!Send` in Rust.

**Why not lopdf** (MIT, actively maintained; now writes object streams and
decrypts):

1. **No linearization writer.** Fast Web View is a spec requirement, and
   writing it ourselves is a large, error-prone subsystem.
2. **Eager loading.** `Document::load` materializes every object into an
   in-memory map. A 2,000-page or 500 MB file would blow the 150 MB idle
   budget. qpdf resolves objects lazily from the xref.
3. **Repair depth.** qpdf reconstructs broken xrefs by scanning, fixes
   `/Length`, recovers from missing `endstream`/`endobj`, and has 15+ years of
   hostile-file hardening.
4. **Breadth.** Encryption *writing* across all revisions (including
   RC4-128 for compatibility and AES-256 R6), hybrid-reference files, and the
   helper classes above.

The costs of qpdf are C++ inside the trust boundary (mitigated by the sandbox
and fuzzing) and a CMake build.

Spike 0.3 checks points 2 and 3 with data. It runs both libraries over the
corpus (open success, repair success, peak RSS on the 2,000-page file). If
lopdf turns out comparable on repair and memory, I'll bring the decision back
to you.

### 4.2 Content streams — `papyrine-content`

This is a pure-Rust lexer, a parser into `Op`s with byte spans, and a
serializer with configurable number precision. The interpreter (graphics and
text state, XObject recursion, positioned glyph, image and path events)
arrives in v0.2 for effective DPI, and is reused by editing, redaction and
the inspector. v0.1 needs the parser and serializer, for annotation
appearance streams and page-level operations. The crate is fuzzed from
day one.

### 4.3 Document model — `papyrine-model`

This layer provides typed, lazily materialized views: the page tree with
inheritance, boxes, rotation, labels, outlines, name trees, AcroForm fields,
annotations, Info/XMP, the fonts inventory, attachments, OCGs, the struct-tree
flag, an encryption summary, and signature fields (read-only in v0.1). Each
COS object has a generation counter bumped by the journal. Derived state is
cached against those counters.

### 4.4 Operations — `papyrine-ops`

```rust
pub trait Command: Send {
    fn describe(&self) -> LocalizedText;
    fn apply(&mut self, cx: &mut EditContext) -> Result<ChangeSet>;
    fn revert(&mut self, cx: &mut EditContext, cs: &ChangeSet) -> Result<()>; // default: restore before-images
    fn params(&self) -> serde_json::Value;                                     // replay / macros
}
```

- `EditContext` records every touched or created object and captures a
  **before-image** at first touch.
- `ChangeSet` = before-images, after-object list, affected pages, repaint
  hint.
- `History` keeps undo/redo within a memory budget, spilling to the journal
  directory.
- `CompositeCommand` = one undo step made of several commands.

**Consistency guard** (so our dirty tracking can never miss an object qpdf
changed):

1. Shim operations that create objects report the new object IDs. qpdf
   allocates new IDs above the current maximum, so creation is detectable
   by comparing `max_id` before and after.
2. In debug builds and in every test, a **shadow verifier** fingerprints
   *all* objects before and after each command. Any object whose fingerprint
   changed but is missing from the `ChangeSet` fails the test. This runs in
   the round-trip suite for every command.

### 4.5 Writer — one object model, three outputs

The in-memory `QPDF` instance on the document actor is the **only** object
model. Both writers serialize from it.

| Output | Who writes it | Object IDs | Used for |
|---|---|---|---|
| **Incremental section** | `papyrine-writer` (Rust): serializes dirty and new objects by walking qpdf handles; writes an xref table if the file used tables, otherwise an xref stream; `/Prev` chain; encrypts strings and streams with per-object keys when the doc is encrypted (RustCrypto) | Preserved (same as in memory) | **Default Save**; render snapshot sections; journal checkpoints |
| **ID-preserving full write** | `papyrine-writer`: all live objects, one xref, no `/Prev` | Preserved | Snapshot compaction (L2), recovery checkpoints |
| **Optimized full rewrite** | qpdf `QPDFWriter` (object and xref streams, compression, linearization, encryption changes, garbage collection) | **Renumbered** | "Save optimized", Compress, Sanitize, Redact, Remove security, Save As with "optimize" |

**Staying consistent:**

- **Serializer parity.** Our serializer and qpdf's `unparseBinary` must
  agree. A test re-parses every incremental section with qpdf and compares
  each object with the in-memory handle, for the full corpus of edits.
- **Incremental save is only offered when it is safe.** If the file needed
  repair (the original xref chain is broken), appending a `/Prev` to it
  would produce a file other readers may reject. Save then does an optimized
  full rewrite and says so. The exception is a signed file: we warn that
  preserving the signature would require appending to a damaged file, and
  let the user choose.
- **Rebasing after an optimized rewrite in place.** QPDFWriter renumbers
  objects, so the saved file's IDs no longer match memory. After the atomic
  replace, the engine reopens the new file as the base `QPDF` and rebases
  undo history through the `getRenumberedObjGen` map (before-image object
  IDs are translated). An entry that cannot be mapped (an object that was
  garbage-collected) truncates history at that point, with a notice
  ("Undo history before the optimized save is no longer available").
- **Save policy:**
  - Save = incremental.
  - After many saves (appended bytes > 50% of the original, or > 20
    sections), Save shows a one-time, non-blocking suggestion to "Save
    optimized". The user can turn it off.

**Atomic replace:**
1. Write to a temp file in the same directory.
2. `fsync` it (`F_FULLFSYNC` on macOS).
3. Validate it: qpdf re-parse, plus the renderer opens it and renders
   page 1.
4. `rename` / `ReplaceFileW`, preserving permissions, extended attributes
   and ACLs.
5. `fsync` the directory.

The original is never touched on failure.

### 4.6 Rendering and snapshot compaction

**Tiles:**
- 512 px tiles, √2 zoom buckets, GPU scaling between buckets.
- Low-res preview first, then sharp tiles, then prefetch.
- A priority queue with cancellation (`FPDF_RenderPageBitmap_Start` with a
  pause callback).
- Transport over the `papyrine://` scheme as raw RGBA, decoded with
  `createImageBitmap` (ADR-009).

**Caches, sized to the memory budget:**
- L1 decoded tiles in the broker: soft cap 48 MB, trimmed to visible tiles
  plus the preview when idle for more than 5 s.
- L2 previews and thumbnails, QOI-compressed: 16 MB.
- PDFium's page cache is limited to 8 pages.
- The disk thumbnail cache is off by default.

**Compaction policy** for render snapshots (ADR-005):

| Level | Trigger | Action | Cost |
|---|---|---|---|
| L0 | Every commit | Append one section with only this commit's changed objects | O(changed objects) |
| L1 merge | `k > 16` sections, or measured re-open time > 30 ms | Replace all sections with one merged section (latest version of each dirty object) | O(dirty bytes), background on the actor |
| L2 rebase | Section bytes > max(16 MB, 25% of base), or `k > 16` after an L1 | ID-preserving full write to a temp file in the cache dir; this becomes the new render base (mmap); sections reset to 0 | O(document), background, off-actor streaming; skipped while a save is running |

Renderer re-open is debounced (~50 ms after the last commit). The broker
invalidates tiles only for `affected_pages`. Tiles of other pages stay valid
across snapshots, because a snapshot changes only what the ChangeSet says.

**Benchmark B1 (first Phase work):** 2,000-page file, 1,000-page image-heavy
file, and `typical-20p`. It measures re-open time, first-tile time after an
edit, and RSS, as functions of `k` (1–64) and section size (1 KB–64 MB).

**Targets:** edit → visible tile updated < 100 ms p95, and renderer RSS
growth < 10 MB over a 500-edit session.

**Fallback if B1 misses: command mirroring.** Frequent edits (annotations,
page rotate/move/delete, form values) are also applied directly to PDFium's
in-memory document through its edit APIs (`FPDFAnnot_*`, `FPDF_MovePages`,
`FPDFPage_SetRotation`, `FPDFAnnot_SetStringValue` for form values), so the
visible tile updates without a re-open.

**How qpdf and PDFium stay consistent under mirroring:**

1. **qpdf is the only source of truth.** The PDFium document is a
   display-only replica. Nothing is ever saved, exported, searched for
   results that feed back into edits, or journalled from PDFium. Saves,
   journal records and every command's ChangeSet come from qpdf alone.
2. **Commit order.** A command commits in the engine (qpdf) first. Only a
   successful commit produces a `Mirror { seq, op }` message for the
   renderer. A command that fails in qpdf is never mirrored.
3. **Mirror ops are declarative** and derived from the ChangeSet, not
   re-running command logic. Example: "set annotation N's /Rect, /C and
   appearance stream to these bytes". PDFium gets the same appearance-stream
   bytes qpdf stored, so the two render identically.
4. **Only whitelisted, mirror-safe command types are mirrored.** Everything
   else, and any mirror op that PDFium rejects, triggers an immediate
   snapshot re-open, which is always correct.
5. **Epochs:**
   - The renderer tags every tile with the replica epoch `(base_id,
     last_seq)`.
   - At idle, or after 50 mirrored ops or 30 s, the renderer does a **resync**:
     it re-opens from a qpdf snapshot and discards the replica.
   - Tiles rendered from the replica for pages the resync changed are
     invalidated.
   - Divergence is therefore bounded in time and never persistent.
6. **Undo** is mirrored as the inverse declarative op, derived from the
   before-images, or falls back to a re-open.
7. **Verification in CI:** for every mirror-safe command, a test renders the
   affected page (a) from the mirrored replica and (b) from a fresh qpdf
   snapshot, and requires pixel identity (or ≤ 1/255 per channel for
   anti-aliasing). A randomized property test runs 1,000-op sequences with
   the same comparison at the end.
8. **Form field typing** never happens inside PDFium's form-fill
   environment. Our UI overlay edits the value, which becomes an engine
   command and then a mirror op. PDFium never owns field state.

### 4.7 Text — `papyrine-text`

Extraction, hit testing and char boxes come from PDFium in the renderer. On
top of that `papyrine-text` does search normalization: NFKC, ligature
expansion, hyphenation joining, case and diacritic folding, and bidi logical
order. Results stream as `SearchHit` events. Layout analysis (blocks,
paragraphs, reading order) arrives with v0.4 Edit.

### 4.8 UI — `apps/desktop/src`

- **Framework:** React 18 + strict TypeScript, Vite, Zustand, Radix UI
  primitives, TanStack Virtual, i18next (ICU; RTL via logical CSS).
- **Styling:** design tokens as CSS custom properties, with light, dark and
  high-contrast themes. The icon set is original SVG.
- **Code splitting:** every non-core tool is a `React.lazy` chunk loaded on
  first use. The app shell is bundle-budgeted (§1.1).
- **Text layer:** an invisible positioned text layer per visible page, for
  selection, search highlights and screen readers.
- **IPC client:** generated TS types. Requests carry a `RequestId`; long jobs
  return a `JobId` with progress events and `cancel(jobId)`.

### 4.9 IPC — `papyrine-ipc`

- **UI ⇄ host:** Tauri commands, Tauri events, and the `papyrine://` scheme.
- **Host ⇄ children:** `ipc-channel`, `serde` + `postcard`, shared memory
  for pixels and sections, and native handle passing.
- **Cancellation:** `CancelToken` is checked at most ~50 ms apart. A
  mutating job cancelled before commit leaves the document untouched.
- **Versioning:** the protocol is versioned. Component helpers declare the
  protocol range they support.

---

## 5. Data flows

**Open:**
1. The host mmaps the file and spawns (or reuses) the renderer, which renders
   page 1 immediately. This is the < 1 s path.
2. In parallel the engine loads the file through qpdf and reports metadata,
   outline and the RepairLog.
3. If qpdf had to repair the file, the render base switches to qpdf's
   repaired ID-preserving full write, so both processes see the same objects
   (§15).

**Edit:**
1. The UI sends `Execute(cmd)` to the **host**.
2. The host's journal writer appends an `Intent` record (§7), then forwards
   the command to the engine.
3. The actor applies it: ChangeSet, history push, new snapshot section. It
   returns the after-images, and the host appends a `Commit` record.
4. The host invalidates tiles for the affected pages.
5. The renderer re-opens (debounced) and the UI re-requests only visible
   invalidated tiles.

**Save:** the writer (§4.5), atomic replace, the journal is marked clean, and
the file watcher ignores the self-caused change.

---

## 6. Threading

| Where | Threads |
|---|---|
| WebView | Main JS thread renders only; tile decode via `createImageBitmap` |
| Host | Tokio runtime (IPC, I/O, watcher, journal fsync) + blocking pool |
| Engine | One document actor per open doc (owns `QPDF`) + a shared rayon pool, created lazily, for CPU work on owned data |
| Renderer | One PDFium thread + one IPC thread |

Invariants:
- `QPDF` is only touched on its actor.
- No lock is held across IPC.
- Long work is chunked with cancellation checks.

---

## 7. Persistence: change-driven journal (ADR-012)

This replaces timer-based autosave.

**Journal per document:** `recovery/<doc-id>/`:
- `base.json`: original path, size, mtime, BLAKE3 hash of the original.
- `journal.log`: an append-only record log. Each record is length-prefixed
  and CRC32C-checksummed.

**The journal is written by the host, outside the engine, and each command's
record is written before the engine applies it** (write-ahead). The engine
has no filesystem access (§8), and an engine crash must not be able to lose
or corrupt the journal.

**Two records per command:**

| Record | Written | Contents |
|---|---|---|
| `Intent{seq}` | By the host **before** forwarding the command to the engine | `seq`, timestamp, command name, `params()`, and every **external input** the command needs (e.g. the bytes of an inserted PDF or image, stored in a side blob `blobs/<blake3>` so replay never depends on files that may have changed) |
| `Commit{seq}` | By the host when the engine returns the ChangeSet | the **after-images** of every object in the ChangeSet (serialized object + stream bytes, zstd if > 4 KB), created object IDs, and a BLAKE3 of the after-image set |

Every record is length-prefixed and CRC32C-checksummed.

**The ordering guarantee:**
1. The host's `write()` of the `Intent` completes before the engine receives
   the command.
2. After a *process* crash (host, engine or renderer), the intent is already
   in the OS page cache, so it survives.
3. `fsync` is group-committed at most once per **1 s** (configurable
   0.2–5 s), so a power loss or kernel panic loses at most ~1 s. A command
   with a large payload (> 1 MB) waits for its own `fsync` before it is
   forwarded.
4. The write is a buffered append of a few KB (µs), so the write-ahead step
   adds no user-visible latency. The `fsync` never blocks the UI or the
   actor.

**Replay rules:**
- `Intent` + `Commit`: apply the after-images directly. This is
  deterministic and never re-runs command logic.
- A trailing `Intent` without `Commit` means the process died *during*
  apply. By default recovery does **not** re-execute it, because the
  command may itself be what crashed the engine. Instead the user is told:
  "Your last action, *Rotate pages 3–5*, didn't finish. [Redo it] [Skip]".
  Redo re-runs it from `params()` and the stored inputs.
- A torn tail (bad CRC) is truncated at the last valid record.

**Checkpoints:** when the journal exceeds 32 MB or 500 records, the engine
produces, and the host writes, an ID-preserving checkpoint (`checkpoint.pdfu`, a merged incremental
section over the original) and truncates the journal.

**Recovery:**
1. On launch the host scans `recovery/`.
2. If the original still matches `base.json`'s hash, it offers
   **Restore unsaved changes**: open the original, apply the checkpoint, then
   replay journal records up to the last valid CRC.
3. Undo history is rebuilt, because before-images are derivable from the
   original plus earlier after-images.
4. If the original changed on disk, it offers **Open recovered copy**
   instead.

**Engine crash (not app crash):** the supervisor restarts the engine and
the host replays the journal into it. The user sees "Engine restarted; no
changes lost." If the in-flight command crashed the engine, it is skipped
and reported as above. A command that crashes the engine twice is
quarantined for that document, so there is no crash loop.

**Cleanup:** the journal is deleted on a clean save or close ("Don't save"),
and recovery directories older than 30 days are pruned, with a notice.

**External changes:** a `notify` watcher plus an mtime/size check on focus.
If the file changed, a banner offers "Reload" or "Keep mine (Save As)".

**Privacy:** the journal contains document content. It lives in the per-user
app-data dir with 0600/owner-only ACLs, is never uploaded, and is excluded
from Time Machine and backup indexing where the OS allows.

---

## 8. Security

**Threat:** a malicious PDF exploits a memory-safety bug in qpdf, PDFium or
a codec (C/C++) to read or write the user's files or to exfiltrate data.

| Control | Implementation |
|---|---|
| Least privilege | Engine, renderer and component helpers run sandboxed; only the host has filesystem, dialog and network access |
| macOS | Seatbelt profile via `sandbox_init` at child start: deny by default; allow reading the app bundle, component dir and a per-child temp dir; no network; no writes outside temp |
| Linux | `landlock` fs allowlist + `seccomp-bpf` syscall allowlist (no `socket`/`connect`/`execve`) + `PR_SET_NO_NEW_PRIVS`; user/net namespaces when available |
| Windows | Restricted token + low integrity level + Job object (no child processes, memory cap, UI restrictions); AppContainer evaluated later |
| File access | Brokered: children receive already-open read-only handles or bytes; all writes go through the host's atomic replace using bytes the engine produced |
| PDF JavaScript | Off by default; only available through the optional Form Scripts component, running inside a sandboxed helper with CPU and memory limits and no network or filesystem API (ADR-006) |
| Actions | Launch, URI, GoToR and embedded-file-open actions go to the host, which always prompts with the target shown |
| Secrets | Passwords and keys held in `zeroize`d buffers, redacted in logs by a `Secret<T>` type, persisted only in the OS keychain on opt-in |
| Fuzzing | cargo-fuzz targets for `papyrine-content`, qpdf load via the shim, the serializer, codecs and font loaders; 10 min per PR, nightly 1 h per target; OSS-Fuzz application once the repo is public and stable |
| Supply chain | `cargo-deny` (licenses, advisories, sources), `cargo-audit`, `pnpm audit`, committed lockfiles, vendored native sources pinned by hash |

**Components add:**
- Signature verification before install and a hash check before each helper
  launch.
- Helpers run only as sandboxed child processes. They are **never loaded as
  libraries into the host.**

---

## 9. Optional components (ADR-011)

### 9.1 Components

Heavy capabilities are **not** in the base install. A component is either a
**data pack** or a **helper executable** (+ data).

| Component (initial set) | Kind | Approx. download | First needed in |
|---|---|---|---|
| `ocr-engine` (Tesseract 5 + Leptonica helper) | helper | ~5 MB | v0.6 |
| `ocr-lang-<code>` (tessdata_fast; `-best` variants optional) | data | 1–15 MB each | v0.6 |
| `form-scripts` (form JavaScript runtime, ADR-006) | helper | ~1 MB (QuickJS) or ~12 MB (PDFium-V8 variant) | v0.5 |
| `mrc` (MRC scan compression) | helper | ~3 MB | v0.6 |
| `fonts-cjk`, `fonts-indic`, `fonts-arabic-hebrew`, `fonts-extra` | data | 5–40 MB | v0.5 (v0.1 uses installed system fonts via fontdb for non-Latin text) |
| `icc-print` (ECI/FOGRA/GRACoL profiles, where redistributable) | data | ~5 MB | v0.7 |
| `spell-<lang>` (Hunspell dictionaries, Linux only; macOS and Windows use OS spell check) | data | ~1 MB each | v0.4 |

**Package format:** a `.papyrine-component` file (zstd tar) containing:
- `component.toml`:
  - `id`, `version`, `kind`, `platforms` (os/arch);
  - `min_app` / `max_app` and the IPC protocol range;
  - `files[]` with SHA-256;
  - `size`, `license`, `notices`, `description`;
- a detached **Ed25519 signature** over the manifest bytes;
- the payload.

**Catalog:**
- A signed `components.json` published as a GitHub Release asset on the
  project repo. That is static hosting, no server.
- It is signed by an online **component signing key** that the embedded
  root keys authorize (§9.3).
- The catalog is fetched **only** on user action, or if the user enabled the
  update check (§9.2). There is no background network by default.

**Install flow:**
1. On first use of a feature, a prompt appears: "Recognize Text needs the OCR
   engine (5 MB) and English (4 MB). Download?"
2. Download to a temp dir, verify the signature and hashes, unpack, and
   atomically rename into
   `app-data/components/<id>/<version>/`.
3. The previous version is kept until the new one launches successfully.

**Offline and enterprise installs:**
- "Install from file…" accepts `.papyrine-component` files.
- A mirror URL or directory can be set in settings or in a policy file
  (`/etc`, the registry, or a macOS profile).
- The CLI has `papyrine components list|install|remove|verify`.

**Runtime:**
- The component manager resolves the helper binary.
- The supervisor launches it sandboxed with the same IPC protocol, and it
  exits after 60 s idle.
- Data packs are opened read-only by the process that needs them.

**Preferences → Components** lists what is installed, disk use, and
update/remove actions. Each component's notices are merged into About →
Licenses.

### 9.2 Opt-in security update check

- **Off by default.** First run asks once, with no pre-selected answer:
  "Check for security updates once a day? Papyrine sends no identifiers:
  it only downloads a small public file." The answer is stored and can be
  changed in Preferences → Privacy.
- **What it fetches:** one static file, `updates.json`, from the latest
  GitHub Release of `manjunathsharma10/papyrine`, plus `components.json`
  when component updates are enabled. It is an HTTPS GET with no cookies,
  no query string and no custom headers beyond a generic User-Agent. No
  version, OS or install ID is sent; comparison happens locally.
- **What it contains:** per-channel latest version, a severity flag
  (`security` / `normal`), a short advisory text, affected version ranges,
  download URLs with SHA-256, and an Ed25519 signature (§9.3). An unsigned
  or badly signed file is ignored and logged.
- **Behaviour:**
  - For a `security` release affecting the installed version, a
    non-modal banner appears: "Security update available (v0.1.3): fixes a
    crash when opening crafted PDFs. [Download] [Details] [Later]".
  - Normal releases show only in About.
  - Download verifies the SHA-256 and the signature, then opens the
    installer, or uses the Tauri updater once v1.0 auto-update exists. There
    is **no silent install.**
- **Offline and enterprise:** a policy setting disables the check entirely
  or points it at a mirror.
- The CLI has `papyrine update check`; it is never automatic.

### 9.3 Signing keys: storage and rotation

Four kinds of key, all Ed25519 unless noted:

| Key | Signs | Storage | Rotation |
|---|---|---|---|
| **Root keys** (2: primary + backup) | Only the `keyring.json` that authorizes the online keys | **Offline.** Generated on an air-gapped machine; the private keys live on two hardware security keys (YubiKey, OpenPGP/PIV Ed25519) kept in different physical places, with an encrypted paper backup of each seed in a safe. Never on a networked machine or in CI | Root public keys are embedded in the app. A root is replaced by an app release that embeds the new root, signed by the other root. Either root alone can sign a keyring, so losing one is survivable |
| **Release signing key** (updates.json, installer hashes) | `updates.json` | GitHub Actions **environment secret** in a protected `release` environment (required reviewer = owner; main-branch-only; no fork PRs) | Rotated every 12 months or on suspicion. The new public key is added to `keyring.json` (signed by a root) with validity dates; the old key stays valid for verification until its expiry |
| **Component signing key** | `components.json` and each component manifest | Same protected environment, separate secret | Same as the release key |
| **Tauri updater key** (v1.0) | Updater artifacts (minisign) | Same protected environment | Tauri embeds one updater public key per build, so rotation ships the new key in a release signed with the old key |

- **Keyring:** `keyring.json` lists the online keys with their purpose,
  `not_before` and `not_after`, and revocations. It is signed by a root, and
  fetched and cached alongside `updates.json` and `components.json`.
- **Compromise of an online key:** a root signs a keyring that revokes it.
  Clients that fetch it stop trusting the revoked key immediately. Offline
  clients learn on their next check, and every app release also bundles the
  latest keyring.
- **Documentation:** the key ceremony, custody and incident steps go in
  `docs/KEYS.md`; the public threat model goes in `SECURITY.md`.
- **Dev builds** use a separate dev root that release builds reject.
- **Timing:** keys are created before the first public release (end of
  v0.1). Until then nothing signed is published.

### 9.4 Code signing and notarization of downloadable helpers

Component helpers are executables that the app downloads, so the OS must
trust them like the app itself.

- **macOS:**
  - Each helper is packaged as a minimal **app bundle**
    (`PapyrineOCR.app/Contents/MacOS/papyrine-ocr`), because a bare Mach-O
    can't carry a stapled ticket.
  - It is signed with the same Developer ID Application identity as the app,
    with hardened runtime and minimal entitlements (none for JIT; QuickJS
    needs none).
  - It is notarized with `notarytool` and **stapled**, so offline launches
    pass Gatekeeper.
  - On install, the host checks
    `SecStaticCodeCheckValidity` against the requirement
    `anchor apple generic and certificate leaf[subject.OU] = "<TEAMID>"` in
    addition to our Ed25519 signature.
  - Downloads are written without a quarantine flag, because our own
    signature check replaces Gatekeeper's download prompt. Notarization
    still covers the case of users copying packs manually.
- **Windows:**
  - Authenticode-signed with the app's certificate plus an RFC 3161
    timestamp.
  - On install, the host checks `WinVerifyTrust` and that the signer
    matches the app's own signer.
  - EV or Azure Trusted Signing avoids SmartScreen warnings.
- **Linux:** the Ed25519 signature only; Flatpak builds ship components as
  Flatpak extensions where possible.
- **CI:** the release workflow signs, notarizes and staples helpers in the
  same job as the app, then builds the `.papyrine-component` and signs its
  manifest.
- **Until certificates exist:** helper builds are marked `unsigned`. Release
  builds refuse unsigned components unless Preferences → Advanced →
  "Allow unsigned components (developers)" is on, which shows a warning.

### 9.5 Lazy loading in the base install

To protect the startup budget, code that is part of the base install but
not core is initialized on first use, not at launch:
- codecs, the rayon pool, the fontdb system font scan, lcms2 transforms,
  the component catalog, and the search index;
- enforced by the startup-trace allowlist gate (§1.1).

---

## 10. Build, CI, release

- **Pinned toolchains:** Rust (`rust-toolchain.toml`), Node LTS, pnpm, and
  CMake + a C++17 compiler.
- **PDFium:** `bblanchon/pdfium-binaries`, no V8/XFA, pinned version + SHA-256.
- **CI matrix:** macOS arm64/x64, Windows x64/arm64, Ubuntu x64.
  - Checks: fmt, clippy `-D warnings`, `tsc`, ESLint, Prettier, and tests.
  - Gates: licenses and notices (§12); the corpus-not-committed check
    (ADR-017); budgets (§1.1); benchmarks (§13).
  - Nightly fuzzing.
- **E2E:** `tauri-driver` WebDriver on Windows and Linux. On macOS,
  Playwright against the real engine via a dev-only bridge.
- **Release:** per-arch `.dmg`, `.msi`/NSIS, `.deb`/`.rpm`/Flatpak/AppImage,
  plus components as signed `.papyrine-component` assets. Code signing is wired
  but skipped until certificates exist.

---

## 11. Platform integrations

### 11.1 Fonts, spelling, speech
- **Fonts:** fontdb system scan (lazy) + the bundled base fonts + font
  components.
- **Spell check:** OS spell checkers (NSSpellChecker, Windows
  ISpellChecker); spellbook + dictionary components on Linux.
- **Read Aloud:** the `tts` crate over OS engines.

### 11.2 Printing (native APIs, not webview print)

**Basic print scope:**
- the OS print dialog;
- all / current / range of pages;
- fit, shrink oversized, or actual size, with auto-rotate and centering;
- print comments on or off (per the annotation `/Print` flag);
- form values included;
- progress and cancel.

Copies, duplex and paper come from the OS dialog.

**Shared pipeline.** The engine produces a **print PDF**: the page subset,
annotations filtered by print flag, form appearances generated, from the
qpdf-normalized document, so damaged files print the way they display.
Printing is never done by the webview.

| Platform | Approach | Estimate |
|---|---|---|
| Shared | Print-PDF generation, scaling/rotation math with unit tests, the print options UI, progress/cancel | 4–5 days |
| **macOS** | `NSPrintOperation` via objc2, using PDFKit's `PDFDocument.printOperation(for:scalingMode:autoRotate:)` on the print PDF. This is vector output with the native dialog and preview. A "Print as image" option renders pages with PDFium at printer DPI instead, for files Quartz mishandles | 3–4 days |
| **Windows** | There is no OS API to "print a PDF", so this is Chromium's approach. Win32 `PrintDlgEx` provides the printer DC and DEVMODE (paper, orientation, duplex, copies). The **sandboxed renderer** renders each page with PDFium into an **EMF** (`FPDF_SetPrintMode`, `FPDF_RenderPage` with `FPDF_PRINTING`) and sends it to the host, which plays it into the printer DC (`PlayEnhMetaFile`) on a background thread with `StartDoc/StartPage/EndPage`. That way PDF parsing never runs in the unsandboxed host. The work includes printable-area offsets, per-page orientation, cancel via `AbortDoc`, and a banded "print as image" fallback for drivers that mishandle EMF transparency | 8–10 days |
| **Linux** | `GtkPrintUnixDialog` + `GtkPrintJob` with `gtk_print_job_set_source_file(print.pdf)`, which sends the PDF straight to CUPS (vector). Inside Flatpak, GTK uses the `org.freedesktop.portal.Print` portal automatically. Any conversion CUPS does uses the *system's* filters, which we don't ship or link | 4–5 days |
| Test infrastructure | Virtual printers in CI: CUPS-PDF (Linux), "Microsoft Print to PDF" with an output path (Windows), and `NSPrintInfo` job disposition "save" (macOS). Captured output is rendered and compared to expected pages | 2 days |
| **Total** | | **21–26 working days (~4–5 weeks)** |

My v2 estimate of "~1 week" was wrong. It didn't count Windows, which has
no OS PDF printing, or the test infrastructure. Advanced printing (N-up,
booklet, poster, …) stays in v0.8. The v0.1 vs v0.1.x decision is yours
(ROADMAP 1.17).

### 11.3 Scanning
WIA/TWAIN, ImageCaptureCore, and SANE (runtime-only).

### 11.4 HEIC/HEIF import (ADR-013)
- **macOS:** ImageIO (`CGImageSourceCreateWithData`), available on every
  supported macOS. No extra code or license.
- **Windows:** WIC. It needs two Microsoft Store extensions:
  - *HEIF Image Extensions* (free) for the container;
  - *HEVC Video Extensions* for the codec. This is a paid Store item, though
    many OEM Windows 11 machines have the free "from device manufacturer"
    edition preinstalled.

  Papyrine detects which one is missing, and the message is specific:
  - **HEVC missing:** WIC finds the HEIF container decoder, but frame decode
    fails with `WINCODEC_ERR_COMPONENTNOTFOUND`.
  - **HEIF missing:** there is no decoder for `GUID_ContainerFormatHeif`.

  Message (HEVC case):
  > **Can't open "IMG_2041.heic"**
  > Windows needs Microsoft's **HEVC Video Extensions** to read HEIC photos.
  > It's available from the Microsoft Store (it may cost a small fee).
  > **[Open Microsoft Store]** **[Convert another way…]** **[Cancel]**

  - "Open Microsoft Store" launches the `ms-windows-store://pdp/?ProductId=…`
    link for the right extension. The IDs are verified at implementation and
    kept in one constant.
  - "Convert another way…" explains exporting the photo as JPEG from the
    Photos app.
  - In batch imports, missing HEIC files are listed in one summary rather
    than one dialog per file.
  - The check runs once per session and is cached, and it re-runs after the
    user returns from the Store.
- **Linux:** `dlopen("libheif.so.1")` at runtime if the system has it (most
  distros package it; it is included in recent Freedesktop/GNOME Flatpak
  runtimes). It is not bundled; LGPL and HEVC patent exposure are ADR-008
  and ADR-013 concerns. Otherwise Papyrine shows a message explaining how to
  install libheif, or suggests converting with the system image tool.
- Papyrine never ships an HEVC decoder of its own on any platform.

---

## 12. Licenses, notices and the license gate (ADR-001, ADR-014, ADR-015)

**Product license:** MIT OR Apache-2.0.

**Shipped dependencies:** permissive only. Allowlist: MIT, Apache-2.0,
BSD-2/3, ISC, Zlib, MPL-2.0, Unicode-3.0, FTL, IJG, libpng, BSL-1.0, CC0,
OFL-1.1 (fonts only).

**Banned in shipped artifacts:**
- GPL and AGPL at any level.
- LGPL except runtime-`dlopen`ed, optional system libraries.
- By name: MuPDF, Ghostscript, jbig2dec, dssim, Poppler, libheif (bundled),
  and GnuTLS (LGPL) anywhere.
- **qpdf's crypto provider must be native** (§4.1). OpenSSL is also
  license-compatible (Apache-2.0), but it is not used, and the gate rejects
  it too so the configuration can't drift.

**Separate inventories:**

| Inventory | Source of truth | Gate |
|---|---|---|
| Shipped Rust | `cargo metadata` for the `papyrine-app` binary graph, **excluding dev-dependencies** (`cargo-deny` with `exclude-dev = true`, `deny.toml`) | Strict allowlist + name bans |
| Shipped JS | `pnpm licenses list --prod --json` for `apps/desktop` | Strict allowlist + name bans |
| Shipped native (vendored C/C++ and PDFium) | `third_party/native.toml`: one entry per library with version, SPDX, license and notice file paths, and the PDFium sub-licenses (FreeType, ICU, lcms, libjpeg-turbo, OpenJPEG, libpng, zlib, abseil, AGG, fast_float, simdutf, llvm-libc) | Every vendored dir must have an entry; entries must pass the allowlist |
| qpdf build configuration | The CMake cache of the vendored qpdf build + a runtime test | `DEFAULT_CRYPTO=native`, `REQUIRE_CRYPTO_GNUTLS=OFF`, `REQUIRE_CRYPTO_OPENSSL=OFF`; `getRegisteredImpls() == ["native"]` |
| Components | each `component.toml` | Same allowlist; notices required |
| Test-only oracles (Poppler, PDF.js, veraPDF, pyHanko, pikepdf, EU DSS, scikit-image) | `tools/test-oracles.toml` | Allowed to be any license, but must never appear in shipped inventories |

**Bundle inspection gate** (the last line of defence): after each
`tauri build`, `tools/inspect-bundle`:
- lists every file in the artifact;
- lists linked libraries (`otool -L`, `readelf -d`, `llvm-readobj --coff-imports`);
- fails on any library not in `native.toml` or on the system allowlist;
- fails on any banned name (`libpoppler*`, `libmupdf*`, `libgs*`,
  `libjbig2dec*`, `libheif*`, `libgnutls*`, `libssl*`, `libcrypto*`,
  …);
- scans static archives' symbol tables for GnuTLS/OpenSSL symbol prefixes
  (`gnutls_`, `SSL_`, `EVP_`), which catches static linking.

Poppler is only ever installed via apt/brew inside CI test jobs, as a test
oracle.

**Notices** (attribution obligations apply even under permissive licenses):
- `tools/gen-notices` merges `cargo-about` (Rust), a pnpm production-deps
  license collector (JS), `native.toml` (which pulls in full license texts,
  including PDFium's `licenses/*`), and fonts.
- It produces `THIRD_PARTY_LICENSES.md` (repo) and `notices.html` (shipped in
  the bundle, shown in **About → Licenses**).
- Required credit lines appear verbatim, e.g.:
  - "This software is based in part on the work of the Independent JPEG
    Group" (IJG: libjpeg-turbo, mozjpeg);
  - "Portions of this software are copyright © The FreeType Project" (FTL,
    via PDFium).
- CI regenerates and **fails if the committed file differs**.

### 12.1 Dependency list

**Base install (shipped)** — "v" is the milestone that first needs it. All
licenses are to be confirmed by the gate when each is added.

| Dependency | Purpose | License | v |
|---|---|---|---|
| Tauri 2 (+ dialog, window-state plugins) | App shell | MIT/Apache-2.0 | 0.1 |
| qpdf ≥ 12 (vendored) + zlib-ng + libjpeg-turbo | Object layer, repair, rewrite, linearize, encrypt | Apache-2.0 · Zlib · IJG/BSD-3/Zlib | 0.1 |
| cxx | Rust ⇄ C++ shim bridge | MIT/Apache-2.0 | 0.1 |
| *(own code)* AF-subset recognizer + formatter | Tokenizer and strict recognizer for form scripts (ADR-006); **no third-party JS parser** | MIT/Apache-2.0 (ours) | 0.1 |
| PDFium (prebuilt, no V8/XFA) + pdfium-render | Rendering, text, form widgets | BSD-3/Apache-2.0 + sub-licenses (§12) · MIT/Apache-2.0 | 0.1 |
| ipc-channel, serde, postcard, serde_json, ts-rs | IPC and types | MIT/Apache-2.0 · MIT | 0.1 |
| tokio, rayon, crossbeam, parking_lot | Async and parallelism | MIT/Apache-2.0 | 0.1 |
| notify, blake3, crc32c, zstd | Watching, hashing, journal | CC0/MIT/Apache · CC0/Apache · MIT/Apache · MIT/BSD | 0.1 |
| tracing, thiserror, clap | Logging/startup trace, errors, CLI | MIT · MIT/Apache · MIT/Apache | 0.1 |
| unicode-normalization, unicode-bidi, unicode-segmentation | Search normalization | MIT/Apache-2.0 | 0.1 |
| qoi | Preview cache compression | MIT/Apache-2.0 | 0.1 |
| ttf-parser, rustybuzz, fontdb, subsetter | Fonts for annotations and typing, subsetting | MIT/Apache · MIT · MIT · MIT/Apache | 0.1 |
| landlock, seccompiler, windows-rs, objc2 | Sandbox and OS APIs | MIT/Apache-2.0 | 0.1 |
| zeroize, secrecy | Secret hygiene | MIT/Apache-2.0 | 0.1 |
| ed25519-dalek, sha2, ureq + rustls | Signed update check (v0.1) and component verification and download (v0.5); user-initiated only | BSD-3 · MIT/Apache · MIT/Apache, ISC | 0.1 |
| Base fonts (Noto Sans/Serif/Mono Latin-Greek-Cyrillic subsets) | Typing and annotation text | OFL-1.1 | 0.1 |
| React, Zustand, TanStack Virtual, Radix UI, i18next | UI | MIT | 0.1 |
| mozjpeg | JPEG encode | IJG/BSD-3/Zlib | 0.2 |
| OpenJPEG | JPEG 2000 | BSD-2 | 0.2 |
| jbig2enc + Leptonica | JBIG2 generic-region (lossless) encode | Apache-2.0 · BSD-2 | 0.2 |
| fax | CCITT G4 | MIT | 0.2 |
| image, fast_image_resize, zopfli | Pixels, resampling, max Flate | MIT/Apache · MIT/Apache · Apache-2.0 | 0.2 |
| lcms2 | ICC colour | MIT | 0.2 |
| ssimulacra2 | Perceptual metric (verification) | BSD-2-Clause | 0.2 |
| allsorts | Advanced font subsetting (CFF, CID) | Apache-2.0 | 0.2 |
| RustCrypto (aes, cbc, sha2, rsa, p256, p384, cms, x509-cert, der, x509-ocsp) | Encryption, signatures | MIT/Apache-2.0 | 0.3 |
| cryptoki, security-framework | PKCS#11, macOS Keychain | Apache-2.0 · MIT/Apache | 0.3 |
| spellbook | Linux spell check | MPL-2.0 | 0.4 |
| docx-rs, rust_xlsxwriter | Export | MIT · MIT/Apache | 0.6 |
| tantivy | Folder search index | MIT | 0.8 |
| tts | Read Aloud | MIT | 0.8 |

**Components (downloaded on demand):** Tesseract 5 (Apache-2.0) + Leptonica
(BSD-2) + tessdata (Apache-2.0); QuickJS / rquickjs (MIT) or the PDFium V8
build (BSD-3 + V8 BSD-3); Noto font families (OFL-1.1); ICC profile packs
(per-profile redistribution terms; each verified before inclusion).

**Runtime-detected external tools (never bundled):**
- LibreOffice (MPL-2.0; Office → PDF);
- veraPDF (used under MPL-2.0; PDF/A and PDF/UA validation);
- libheif (LGPL; Linux HEIC);
- SANE (GPL + linking exception; Linux scanning).

**Test-only oracles (never shipped):** Poppler (GPL), PDF.js (Apache-2.0),
pyHanko (MIT), pikepdf (MPL-2.0), veraPDF, EU DSS (LGPL, CI container),
scikit-image (BSD-3).

---

## 13. Benchmarks and quality gates

- **B1** Snapshot re-open (§4.6), the first benchmark built.
- **B2** Launch, first page and idle memory (§1.1).
- **B3** Search: 1,000 pages < 3 s, first hit < 300 ms.
- **B4** Scroll frame timing: p95 ≤ 16.7 ms on the 2,000-page file.
- **B5** (v0.2) Compression benchmark, **gated per file** (ADR-016). For
  every file × preset:
  - size ≤ baseline + 2%;
  - mean page SSIM ≥ baseline − 0.005;
  - min page SSIM ≥ the preset floor (Lossless: pixel-identical render;
    Print 0.97; Balanced 0.92; Screen 0.88; Maximum 0.85).
  Aggregates are reported, not gated.
- **Image metric:**
  - in-house SSIM (Wang et al. 2004, 11×11 Gaussian σ = 1.5, luma, rendered
    at 100 dpi), validated in tests against scikit-image as a test-only
    oracle;
  - SSIMULACRA2 (`ssimulacra2` crate, BSD-2-Clause) reported as a secondary
    perceptual score.
- **Noise:** hosted-runner numbers gate on trends. Absolute targets are
  verified on the reference Mac in every milestone report.

---

## 14. Test corpus (ADR-017)

The corpus is never committed:
- `corpus/manifest.toml` lists URL, SHA-256, license and tags.
- `corpus/fetch` downloads into `corpus/cache/` (gitignored), and CI caches
  it.
- Synthetic files (typical-20p, 2,000-page, 1,000-page image-heavy,
  encryption variants, deterministic malformations) are **generated at test
  time** by `tools/gen-corpus` into the cache.
- CI check `tools/check-no-corpus` fails if any tracked file is a PDF, FDF or
  XFDF, a certificate or key file (`.p12`, `.pfx`), or an image or blob
  > 256 KB, outside an explicit allowlist that starts empty. It also fails if
  anything under `corpus/` other than `manifest.toml` and scripts is
  tracked.

---

## 15. Known hard problems

| Problem | Approach |
|---|---|
| qpdf and PDFium disagree on damaged files | Render base = qpdf's repaired ID-preserving write when repair occurred |
| Snapshot re-open cost | B1 first; compaction L1/L2; command-mirroring fallback with qpdf as sole truth (§4.6) |
| Double memory for large docs (qpdf + PDFium) | Shared mmap, no heap copies, capped caches, large-document memory gate (§1.2) |
| Form JS without V8 | ADR-006: native AF subset + Adobe-boilerplate recognizer in v0.1 (measured: 125/134 sampled JS forms, 14/23 excluding IRS); QuickJS component later |
| Paragraph text editing | v0.4; layout analysis + rustybuzz; de-risking prototypes earlier |
| Lossy JBIG2 character substitution (the Xerox incident) | Lossless generic-region by default in every preset; lossy symbol mode only as explicit opt-in (ADR-018) |
| Webview memory floor | Measured in Spike 0.1; per-OS budgets (§1.2) |
| Benchmark noise on hosted runners | Trend gates + reference-machine absolute checks |

[`cxx`]: https://cxx.rs
