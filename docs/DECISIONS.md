# Folio — Architecture Decision Records

Each ADR has a status (Proposed / Accepted / Superseded), context, decision
and consequences. Once an ADR is **Accepted**, it is never edited, only
superseded. Proposed ADRs may be revised during review; revision 2
(2026-09-30) applies the owner's first review.

| # | Title | Status |
|---|---|---|
| 001 | License MIT OR Apache-2.0; permissive shipped dependencies | Proposed (rev 2) |
| 002 | qpdf via C API + C++ shim is the only object model; lopdf rejected | Proposed (rev 2) |
| 003 | Tauri 2 + Rust + React/TypeScript | Accepted |
| 004 | Single multi-role executable; host + engine + **one** renderer | Proposed (rev 2) |
| 005 | Render snapshots: sections + compaction, benchmark first | Proposed (rev 2) |
| 006 | Form JavaScript: options compared; native AF subset now, component later | Proposed (rev 2) |
| 007 | Pure-Rust text shaping and fonts | Proposed |
| 008 | LGPL/GPL software only as optional runtime integrations or test oracles | Proposed (rev 2) |
| 009 | Tiles, √2 zoom buckets, raw RGBA transport, budget-sized caches | Proposed (rev 2) |
| 010 | Lightweight budgets as CI gates | Proposed |
| 011 | Optional components mechanism | Proposed |
| 012 | Change-driven journal replaces timer autosave | Proposed |
| 013 | HEIC via OS decoders | Proposed |
| 014 | Generated third-party notices | Proposed |
| 015 | License gate: shipped vs test-only inventories + bundle inspection | Proposed |
| 016 | Image-quality metric and per-file compression gates | Proposed |
| 017 | Test corpora are never committed | Proposed |
| 018 | JBIG2 lossless by default; lossy only as explicit opt-in | Proposed |
| 019 | Product name: "Folio" conflicts; alternatives | **Needs owner decision** |
| 020 | MVP-first milestone plan overrides the spec's phase order | Proposed |

---

## ADR-001: License MIT OR Apache-2.0; permissive shipped dependencies
**Context:** the owner wants open source and asked for the best license.

**Decision:**
- Dual **MIT OR Apache-2.0**.
- Shipped dependencies must be on the allowlist in ARCHITECTURE §12.
- GPL and AGPL are banned from anything shipped. LGPL is allowed only as a
  runtime-`dlopen`ed, optional system library.
- Named bans: MuPDF, Ghostscript, jbig2dec, dssim, Poppler, and a bundled
  libheif.

**Consequences:**
- It maximizes adoption, including by businesses, and includes Apache-2.0's
  patent grant.
- JBIG2 decoding comes from PDFium, and SSIM is in-house (ADR-016).
- A permissive license still carries **attribution obligations**
  (IJG, FTL, BSD notices, …). These are met by ADR-014.

## ADR-002: qpdf via C API + C++ shim is the only object model; lopdf rejected
**Context:** we need a tolerant parser, repair, every encryption revision,
object and xref streams, and linearization. qpdf is C++, and its C API
(`qpdf-c.h`, qpdf 12) covers much less than the C++ API.

**Decision:**
- Rust talks to qpdf through the **C API where sufficient**, plus a **thin
  C++ shim bridged with `cxx`** for the rest. The exact inventory is in
  ARCHITECTURE §4.1. The C API lacks:
  - the writer's renumbering map;
  - encryption key access;
  - `StreamDataProvider` and `InputSource`;
  - `QPDFLogger` details;
  - all document and object helpers (AcroForm, annotation flattening and
    appearances, outlines, page labels, embedded files, name/number trees).
- The shim catches every C++ exception at the boundary.
- The in-memory `QPDF` instance is the **single object model**. Both our
  incremental and ID-preserving writers and qpdf's optimized rewrite
  serialize from it.
- Consistency is kept by:
  - a serializer-parity test against qpdf's own parse;
  - a shadow verifier that fingerprints all objects around every command
    (in tests);
  - rebasing undo history through `QPDFWriter::getRenumberedObjGen` after
    optimized rewrites;
  - refusing incremental appends onto files whose xref needed repair
    (unless the user chooses it to keep a signature).

**Why not lopdf** (MIT, well maintained, now writes object streams and
decrypts):
1. no linearization writer;
2. it eagerly loads every object into memory, which conflicts with the
   memory budget on large files;
3. shallower repair;
4. narrower encryption-writing and helper coverage.

Spike 0.3 measures points 2 and 3 on the corpus. The decision comes back to
the owner if lopdf proves comparable.

**Consequences:**
- C++ sits in the trust boundary, mitigated by the sandbox and fuzzing.
- A CMake build on all platforms.
- A `cxx` bridge to maintain.
- Our own incremental writer and serializer (small, well-specified).

## ADR-003: Tauri 2 + Rust + React/TypeScript
**Status:** Accepted (owner choice).

Small binaries and a native webview per OS, which maps to UIA,
NSAccessibility and AT-SPI. The cost is webview variance, so the UI is tested
on all three OSes. The webview memory floor is addressed in ADR-010.

## ADR-004: Single multi-role executable; host + engine + one renderer
**Context:** the spec requires sandboxed parsing and rendering and crash
isolation. PDFium is single-threaded. The owner asked to reconsider a render
pool against the memory budget.

**Decision:**
- **One executable** re-executed in roles (`host`, `engine`, `render`,
  `cli`), so qpdf and codecs are linked once, which is good for the size
  budget.
- **Three processes** at runtime: host, sandboxed engine, and **one**
  sandboxed renderer.
- Component helpers are extra processes, started on demand.
- The render pool code supports N workers, but the default is 1. It is
  raised only if benchmarks show tile latency misses target *and* a second
  worker fits the memory budget.

**Consequences:**
- The memory overhead is one PDFium instance.
- Rendering, thumbnails and search text share one queue, ordered by
  priority.
- Engine crashes are recoverable through journal replay (ADR-012).

## ADR-005: Render snapshots: sections + compaction, benchmark first
**Context:**
- The renderer must show engine edits.
- Appending an incremental section per edit chains over a long session.
- Every reload has a cost, and the snapshot grows.

**Decision:**
- Each commit appends one section containing only its changed objects (L0).
- **L1 merge** when there are more than 16 sections or re-open exceeds
  30 ms.
- **L2 rebase** (an ID-preserving full write becomes the new base) when
  sections exceed max(16 MB, 25% of the base).
- Re-open is debounced ~50 ms.
- Tiles are invalidated only for the affected pages.
- **Benchmark B1 (Spike 0.2) is the first benchmark built.** It fixes the
  thresholds, and nothing depends on snapshots until it passes.
- **Targets:** edit → updated tile < 100 ms p95; renderer RSS growth
  < 10 MB over 500 edits.
- **Fallback:** command mirroring into PDFium's in-memory document via its
  edit APIs.

**Consequences:**
- Every edit exercises the incremental writer.
- Compaction needs the ID-preserving full writer (which is also used for
  journal checkpoints).

## ADR-006: Form JavaScript: options compared; native AF subset now, component later
**Context:**
- PDFium's form scripting layer (`fxjs`: the app, doc, field, event and util
  objects plus the `AF*` built-ins) is written directly against V8's API.
  Swapping in another JS engine means reimplementing that object model.
- Budget: base installer ≤ 50 MB. Measured PDFium mac-arm64 (chromium/8076):
  - without V8: **6.9 MB** on disk, 3 MB compressed;
  - with V8 (+XFA): **41.6 MB** on disk, 12 MB compressed.

| | (a) PDFium built with V8 | (b) QuickJS + our Acrobat JS API | (c) Defer form JS entirely |
|---|---|---|---|
| Size | +35 MB on disk, +9 MB download; the whole renderer library swaps | +~1 MB | 0 |
| Effort | Low integration (weeks). But form interaction then lives inside PDFium in the renderer, so field values must be synced back to the engine model as commands, which cuts against the "engine is the source of truth" design | High: the event model, Field/Doc/App/util objects, ~60 `AF*` functions, calculation order, Acrobat quirks; est. 8–12 weeks plus a long compatibility tail | None |
| Fidelity | Highest: PDFium's implementation is mature | Good for real-world forms (dominated by `AF*` formatting, simple calculations, show/hide and validation); lower for exotic scripts | Forms fill, but formatting, calculations and validation don't run |
| Security | Large engine and attack surface (JIT; run `--jitless`); inside the sandboxed renderer | Small interpreter, easy CPU and memory caps, no JIT; inside a sandboxed helper | Nothing to attack |
| Fits "optional component"? | Yes: download the V8 PDFium variant and restart the renderer with it | Yes: a separate small helper | n/a |

**Decision:**
1. **v0.1:** (c) plus a **native AF subset**, with no JS engine at all. A
   strict recognizer executes field scripts only when they consist solely
   of standard `AF*` calls with literal arguments (number, percent, date,
   time and special formats and keystrokes, range validation, simple
   calculations), implemented in Rust. Any other script shows a clear
   banner. This covers a large share of real-world forms at zero size cost;
   Spike 0.4 plus the corpus give the actual share.
2. **v0.5:** an optional **`form-scripts` component**, **(b) QuickJS**,
   reusing the Rust `AF*` implementations. It is preferred for size,
   security and keeping the engine as the source of truth.
3. **(a) is the fallback.** If (b) fails to match Acrobat reference behaviour
   on ≥ 95% of the JS-forms corpus after a time-boxed effort, I'll return to
   the owner with (a) as the component.

**Consequences:** the base install has no JS engine. Form JS is always
opt-in: install the component, then enable it per document or globally.

## ADR-007: Pure-Rust text shaping and fonts
We use rustybuzz, ttf-parser, fontdb and subsetter instead of
HarfBuzz/FreeType/fontconfig, which means less C in the trust boundary and
simpler builds. PDFium keeps its internal FreeType for rendering. The fontdb
system scan is lazy (startup budget). Embedding system fonts respects the
OS/2 `fsType` embedding permissions.

## ADR-008: LGPL/GPL software only as optional runtime integrations or test oracles
- LibreOffice (MPL) and veraPDF (used under MPL) are detected subprocesses.
- libheif (Linux) and SANE are runtime `dlopen` or subprocess only, never
  bundled.
- Poppler, EU DSS and other GPL/LGPL tools run **only** as CI test oracles.

This is enforced by ADR-015.

## ADR-009: Tiles, √2 zoom buckets, raw RGBA transport, budget-sized caches
- 512 px tiles, rendered at the bucket scale and GPU-scaled.
- The `folio://` scheme serves raw RGBA, decoded with `createImageBitmap`.
- Caches are sized to the memory budget:
  - tile L1 soft cap 48 MB, trimmed when idle;
  - previews 16 MB (QOI);
  - PDFium page cache 8 pages;
  - disk thumbnails off by default.

## ADR-010: Lightweight budgets as CI gates
**Decision:** the budgets in ARCHITECTURE §1.1 are hard gates:
- installer ≤ 50 MB;
- cold launch to first page < 1 s on the reference Mac;
- idle memory 150 / 175 / 200 MB (macOS / Linux / Windows);
- no pre-first-paint work outside a startup allowlist;
- initial JS ≤ 200 KB gzipped.

**Proposed deviations** from the owner's numbers, with reasons (§1.2):
- Windows and Linux idle memory is higher because of the webview process
  floor.
- AppImage is budgeted at 100 MB because it bundles WebKitGTK.
- The Windows offline installer (which bundles the WebView2 runtime) is
  exempt.

Spike 0.1 validates these numbers before any building on them. If the bare
shell breaks a budget, I report back rather than relax it.

**Consequences:**
- Per-arch macOS builds, not universal.
- Code-split UI.
- Lazy subsystems.
- Heavy features become components (ADR-011).

## ADR-011: Optional components mechanism
**Decision:**
- Components are **signed data packs or sandboxed helper executables**,
  never libraries loaded into the host.
- Format: a `.folio-component` file (zstd tar) with `component.toml` (SHA-256
  per file, compatibility range, license, notices) and an Ed25519 signature
  over the manifest.
- A signed static catalog is served from GitHub Releases. The embedded
  public keys support rotation.
- No background network: the catalog is fetched only on user action or
  opt-in.
- Install from file, or from a mirror or policy, for offline and enterprise
  use. CLI management.
- Atomic install, with the previous version kept until the new one works.
- The initial set: OCR engine, OCR languages, form scripts, MRC, font packs,
  print ICC packs, Linux spell dictionaries (ARCHITECTURE §9).

**Consequences:**
- The base install stays small.
- Features that need a component show a working install prompt on first
  use. That is not a placeholder: the feature works once installed.
- Release engineering must sign and notarize helper executables.

## ADR-012: Change-driven journal replaces timer autosave
**Decision:**
- Every committed command appends a CRC-checked journal record holding the
  **after-images** of changed objects, so replay is deterministic.
- `fsync` is batched to ≤ 1 s, with an immediate fsync for large payloads.
- Checkpoints (an ID-preserving merged section) every 32 MB or 500 records.
- Recovery replays onto the verified original, rebuilds undo, and refuses
  (offering a recovered copy) if the original changed.
- The same journal lets an engine crash recover transparently.

**Consequences:**
- A crash loses ≤ ~1 s of work.
- The journal holds document content, so it is protected with owner-only
  permissions and excluded from backups where possible.

## ADR-013: HEIC via OS decoders
**Decision:**
- macOS: ImageIO.
- Windows: WIC with Microsoft's HEIF and HEVC extensions. If they are
  missing, Folio explains which extension to install.
- Linux: `dlopen` libheif if the system provides it. Otherwise Folio shows
  how to install it, or suggests converting first.
- Folio never bundles an HEVC decoder or libheif.

**Consequences:**
- No LGPL or HEVC patent exposure in shipped artifacts.
- HEIC availability differs by platform, and this is documented in the user
  guide.

## ADR-014: Generated third-party notices
**Decision:**
- `tools/gen-notices` merges:
  - `cargo-about` (shipped Rust graph);
  - a pnpm production-deps license collector;
  - `third_party/native.toml` (vendored C/C++ plus PDFium and all its
    bundled sub-licenses: FreeType FTL, ICU, lcms, libjpeg-turbo IJG,
    OpenJPEG, libpng, zlib, abseil, AGG, fast_float, simdutf, llvm-libc);
  - fonts;
  - components.
- It outputs `THIRD_PARTY_LICENSES.md` (repo) and `notices.html` (shipped;
  About → Licenses).
- Required credit lines are included verbatim (IJG, FreeType).
- CI regenerates the files and fails on any diff.

## ADR-015: License gate: shipped vs test-only inventories + bundle inspection
**Decision:** separate inventories.
- **Shipped Rust:** `cargo-deny`, excluding dev-dependencies.
- **Shipped JS:** production dependencies only.
- **Shipped native:** `native.toml`; every vendored directory must be
  listed.
- **Components:** their manifests.
- **Test oracles:** `tools/test-oracles.toml`. Any license is allowed, but
  an oracle must never appear in a shipped inventory.
- The **bundle inspection** gate checks the built artifact's files and
  linked libraries against the native manifest and the banned names.
- Poppler is installed only inside CI test jobs.

**Consequences:** the GPL test oracles stay usable with zero risk of
shipping them.

## ADR-016: Image-quality metric and per-file compression gates
**Decision:**
- dssim is AGPL and banned.
- **Primary metric:** in-house SSIM (Wang et al. 2004: 11×11 Gaussian,
  σ = 1.5, luma, pages rendered at 100 dpi), ~200 lines, validated against
  scikit-image (a BSD test-only oracle).
- **Secondary:** SSIMULACRA2 via the `ssimulacra2` crate (BSD-2-Clause),
  reported, not gated.
- **Gates apply per file × preset:**
  - size ≤ baseline + 2%;
  - mean page SSIM ≥ baseline − 0.005;
  - min page SSIM ≥ the preset floor (Lossless: pixel-identical; Print
    0.97; Balanced 0.92; Screen 0.88; Maximum 0.85).
- Aggregates are informational.

**Consequences:** a regression on one file can't hide behind improvements on
others.

## ADR-017: Test corpora are never committed
**Decision:**
- The repo holds only `corpus/manifest.toml` (URL, SHA-256, license, tags)
  and scripts.
- Files are downloaded or generated at test time into gitignored
  `corpus/cache/`, which CI caches.
- `tools/check-no-corpus` fails CI if any tracked file is a PDF, FDF or
  XFDF, a key or certificate file, or a binary > 256 KB (the allowlist
  starts empty), or if anything else is tracked under `corpus/`.

**Consequences:**
- No redistribution of third-party test files from a public repo.
- The first CI run on each cache miss is slower.

## ADR-018: JBIG2 lossless by default; lossy only as explicit opt-in
**Context:** JBIG2 symbol matching (pattern matching and substitution) can
replace similar glyphs, as in the Xerox scanner incident where 6 and 8 were
swapped in scanned numbers.

**Decision:**
- Every preset, including Maximum, encodes mono images as **lossless
  generic-region JBIG2** (or CCITT G4 if smaller).
- Lossy symbol mode exists only in Custom → Advanced, off by default.
- Enabling it shows a warning dialog every time and is recorded in the
  report.
- The CLI requires `--jbig2-lossy`, which prints the warning.
- A test asserts that no preset or default enables it.

**Consequences:**
- Mono output is somewhat larger than tools that use lossy symbol mode by
  default.
- Readability and legal safety come first.

## ADR-019: Product name: "Folio" conflicts; alternatives
**Status:** Needs owner decision.

**Findings (2026-09-30):**
- The Mac App Store has **"Folio PDF Reader & Editor"** (`com.folio.pdfeditor`),
  a direct conflict. iOS also has "Folio: PDF Scanner & Editor" and "Folio:
  Private PDF Tools".
- Flathub has **Folio** (`com.toolstack.Folio`), a GNOME markdown notes app.
- On crates.io, `folio` is taken (deprecated) and `folio-pdf` is taken by a
  Rust PDF library.
- On npm, `folio` is taken (a test framework).

Other names checked and rejected:

| Name | Conflict |
|---|---|
| Quire | "Quire: PDF AI Scan, Edit, Sign" |
| Sheaf | "Sheaf – PDF Scanner & Editor" |
| Octavo | A PDF booklet-imposition Mac app |
| Colophon | "PDF Colophon: Edit & Sign" |
| Quarto | Well-known publishing system |

**Candidates:**

| Name | crates.io | npm | Flathub | Apple stores | Notes |
|---|---|---|---|---|---|
| **Recto** (right-hand page) | free | taken (unrelated Bootstrap fork) | none | "Recto Notes", "Recto MD", "Recto Audiobooks": not PDF tools | Short, fits the domain; the npm name is not needed (UI is not a published package) |
| **Papyrine** | free | free | none | none | Fully clear on every registry checked |
| **Rectoverso** | free | free | none | none | Clear; longer |

**Recommendation:** **Recto** for brevity (crates `recto-*`, bundle ID
`io.github.<account>.recto` or a domain the owner controls), or **Papyrine**
if a fully clear name matters more.

A proper trademark search (USPTO, EUIPO, WIPO) is recommended before v1.0.
The registry checks above are not legal clearance.

The GitHub repo `folio` would be renamed; GitHub redirects the old URL.

## ADR-020: MVP-first milestone plan overrides the spec's phase order
**Decision:**
- Step 0 spikes, then **v0.1 MVP**: open, view, search, annotate, fill
  forms, organise pages, save safely. Basic print is proposed as an
  addition.
- Then v0.2 Compress, v0.3 Sign & Protect, v0.4 Edit, v0.5 Forms/Comments
  Pro + components, v0.6 Scan/OCR/Create/Export, v0.7 Standards &
  Accessibility, v0.8 Compare/Automation/Power, v1.0 Release.
- Items cut from old Phase 1 are listed with their new homes in ROADMAP.
- Milestone reports replace phase reports.

**Consequences:** the first usable release comes much sooner, and the
flagship compression follows immediately in v0.2.
