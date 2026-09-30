# Papyrine — Roadmap

Status: **Draft v3** (direction approved; revision 3 applies the second
review) · Last updated: 2026-09-30

**Revision 3:**
- v0.1 shrunk to the owner's list. The CLI, multi-document search, facing
  mode, stamps and split-every-N move to **v0.1.x**.
- Added:
  - the large-document memory gate;
  - the opt-in security update check (v0.1);
  - the write-ahead journal;
  - per-platform print estimates (the decision is pending);
  - the measured form-script coverage.
- Name decided: Papyrine.

**Revision 2:** MVP-first restructure (ADR-020), Step 0 spikes, and the
deferral table.

**Every milestone ends with:**
- an installable build (macOS verified on the reference machine; Windows and
  Linux in CI);
- all budget gates green (ARCHITECTURE §1.1);
- `docs/MILESTONE_vX.Y_REPORT.md`, which replaces the spec's
  PHASE_N_REPORT, with measured budgets and deviations.

**Legend:**
- **AC** = acceptance criteria.
- **Interop check** = passes `qpdf --check`, renders without errors in
  PDFium, PDF.js and Poppler (test-only), and re-opens in Papyrine.
- **Round trip** = open → command → save → reopen → verify, plus undo
  restores objects byte-identically, plus the shadow verifier finds no
  unrecorded change.

| Spec phase | Now lives in |
|---|---|
| 1 Foundation | Step 0 + v0.1 (trimmed) + v0.1.x |
| 2 Organize + Compress | v0.1 (core organize) + v0.1.x + v0.2 (compress, advanced organize) |
| 3 Comment + Fill | v0.1 (core annotate, fill) + v0.1.x (stamps) + v0.5 (the rest) |
| 4 Edit | v0.4 |
| 5 Forms + Protect | v0.3 (protect) + v0.5 (form authoring, form scripts) |
| 6 Scan/OCR/Create/Export | v0.6 |
| 7 Sign + Compare | v0.3 (sign) + v0.8 (compare) |
| 8 Standards + Accessibility | v0.7 |
| 9 Automation + Polish | v0.8 + v1.0 |

---

## Step 0 — De-risking spikes

These are throwaway-quality code in `spikes/`, not shipped. Findings go into
ADRs, and each spike ends with a short report in `docs/spikes/`.

| Task | What | Exit criteria |
|---|---|---|
| 0.0 | Product name | **Done:** Papyrine (ADR-019). Repo renamed |
| 0.1 | Bare Tauri 2 window + static tile on macOS, Windows and Linux | Measured installer size (dmg, msi, deb, rpm, AppImage, Flatpak), cold/warm launch, and idle memory of the bare shell per OS. If the bare shell alone breaks a budget, **stop and report to the owner** |
| 0.2 | qpdf C++ shim (via `cxx`, native crypto) + incremental section writer prototype + PDFium custom reader over a shared mmap. **Benchmark B1** (ARCHITECTURE §4.6) and a **first large-document memory run** (§1.2) | B1 report: re-open time and RSS vs number of sections `k` and section size, on the large and typical files. Compaction thresholds fixed, or the command-mirroring fallback adopted (with its consistency tests, §4.6). qpdf and PDFium per-process memory on the four large files. **Nothing else builds on snapshots until this is done** |
| 0.3 | qpdf vs lopdf on the corpus | Open and repair success, peak RSS, time to first object. Confirms or reopens ADR-002 |
| 0.4 | Form scripts: the full-corpus measurement + an AF behaviour reference | Rust recognizer prototype run over the whole corpus's JS forms (not just the 134-form sample in ADR-006), with results broken down by source. Reference outputs for ≥ 200 AF formatting cases captured from Acrobat Reader (the owner runs a provided form, or public reference tables are used). Confirms the v0.1 forms plan |

---

## v0.1 — MVP

**Scope (owner-defined):**
- view (single page + continuous);
- search (current document);
- annotate;
- fill forms;
- organise pages;
- save safely.

**Definition:** a person can use Papyrine every day to read, mark up, fill
forms and rearrange pages, and **never lose work or corrupt a file**. Only
v0.1 features appear in the UI; everything else is post-MVP.

### 1.1 Repository, tooling, CI
- Cargo and pnpm workspaces (`papyrine-*` crates); strict lint and format;
  LICENSE-MIT, LICENSE-APACHE, README, SECURITY.md, CONTRIBUTING.md.
- CI matrix: macOS arm64/x64, Windows x64/arm64, Ubuntu x64.
- **Gates live from day one:**
  - license (shipped Rust / shipped JS / native manifest / qpdf crypto
    config, separate from test oracles);
  - notices freshness;
  - bundle inspection;
  - no corpus in the repo;
  - budgets (size, launch, idle memory, **large-document memory**,
    startup-trace allowlist, JS bundle).
- **AC:**
  - A clean clone builds and tests with the documented prerequisites.
  - Deliberate violations fail CI on a scratch branch:
    - a GPL crate;
    - a banned native library;
    - qpdf built with GnuTLS or OpenSSL;
    - a committed PDF;
    - stale notices;
    - an oversized installer;
    - a subsystem initialized before first paint.

### 1.2 Test corpus (download and generate at test time only)
- `corpus/manifest.toml` + `corpus/fetch` (SHA-256 verified) +
  `tools/gen-corpus`, which also generates the large-document files.
- **AC:** ≥ 500 files including ≥ 50 malformed and ≥ 150 JS forms. The
  license of each file is recorded. The cache works on all OSes.

### 1.3 `qpdf-sys` (C API + C++ shim) and `papyrine-cos`
- Vendored qpdf ≥ 12 with **native crypto only**; the shim functions in
  ARCHITECTURE §4.1; the safe API; the RepairLog; a custom `InputSource`
  over the shared mmap.
- **AC:**
  - Every corpus file opens or fails with a typed error, with no panic,
    crash or hang (30 s).
  - Synthetic malformed files open with logged repairs.
  - Every encryption revision opens with the right password and is refused
    with the wrong one.
  - `getRegisteredImpls() == ["native"]`.
  - Fuzzing: 10 min per PR, 1 h nightly.

### 1.4 `papyrine-content` (lexer, parser, serializer)
- **AC:** parse → serialize → parse is identical on every corpus content
  stream, and fuzzing is clean.

### 1.5 `papyrine-model` (MVP read side)
- Page tree, boxes, rotation, outlines, AcroForm fields, annotations, Info,
  the encryption and permissions summary, signature presence (for the save
  policy), and the version.
- **AC:** values match expectations for 30 curated files. No panics on the
  corpus.

### 1.6 `papyrine-ops` + write-ahead journal (host)
- `Command`, `EditContext`, `ChangeSet`, `History`, `CompositeCommand`, and
  the shadow verifier.
- The journal is written by the **host**: an `Intent` before the engine
  applies, then a `Commit` with after-images; external inputs go into blobs;
  group fsync ≤ 1 s; checkpoints (ARCHITECTURE §7).
- **AC:**
  - A property test of random command and undo/redo sequences restores the
    exact state.
  - A test asserts that the engine receives no command whose `Intent` hasn't
    been written.
  - **Kill -9 of the host, engine or renderer** at random points (1,000
    iterations, in CI): every command whose `Intent` was written is either
    recovered (with `Commit`) or reported as unfinished (without `Commit`).
    Nothing is silently lost.
  - A simulated power loss (dropping un-fsynced data with a fault-injecting
    filesystem layer) loses ≤ 1 s of commands.
  - A poison command that crashes the engine is quarantined after two tries.

### 1.7 `papyrine-writer`
- Incremental sections, the ID-preserving full write, the qpdf optimized
  rewrite with history rebasing, atomic replace with validation, and the save
  policy (ARCHITECTURE §4.5).
- **AC:**
  - Incremental save of signed files leaves the original bytes as an exact
    prefix, and pyHanko (test-only) reports the signatures intact.
  - Interop check for all three outputs on 100 corpus files.
  - Fault injection at every atomic-replace step never corrupts or loses
    the target.

### 1.8 Renderer, tiles, snapshots
- One render worker, the tile pipeline, `papyrine://` transport, and caches
  sized to budget.
- Snapshots with compaction per B1, or command mirroring with its
  consistency suite.
- Crash restart.
- **AC:**
  - Golden-image match on 50 pages.
  - First page < 500 ms (20 pages) and < 2 s (2,000 pages) on the reference
    Mac.
  - Edit → updated tile < 100 ms p95.
  - Renderer RSS growth < 10 MB over 500 edits.
  - The **large-document memory gate** passes on all four large files.

### 1.9 Process split and sandbox
- Single multi-role executable. Engine and renderer sandboxed on macOS,
  Linux and Windows.
- **AC:** inside each sandbox these attempts are denied: reading `~`,
  writing outside temp, opening a socket, spawning a process. Normal
  operation is unaffected.

### 1.10 Viewer UI (MVP scope)
- App shell: design tokens; light, dark and high-contrast themes; an
  original icon subset; a toolbar with **core tools only**; the All Tools
  panel; the command palette; left pane; right tool pane; status bar; i18n
  scaffolding (English, a pseudo-locale, an RTL layout test).
- Tabs in one window. Open via the dialog, drag and drop, recent files, and
  OS file associations.
- **View modes: single page and continuous.**
- Zoom: fit page, fit width, actual size, presets, 10%–6400%, Ctrl/⌘-wheel
  and pinch.
- Panes: thumbnails and bookmarks (navigate).
- Navigation: go to page, back/forward, full keyboard navigation.
- Document Properties: view; edit title, author, subject and keywords.
- Banners: repaired file, file changed on disk; prompts for URI and launch
  actions.
- **AC:**
  - E2E coverage of the above, keyboard-only.
  - B4 scroll p95 ≤ 16.7 ms.
  - axe-core reports no serious or critical issues.
  - VoiceOver reads the text layer.
  - All strings are in the catalog.

### 1.11 Search (current document)
- Find bar (⌘/Ctrl-F): whole word, case-sensitive, diacritic-insensitive,
  include comments and form values. A results list with snippets, streaming
  and cancellable. Handles ligatures, hyphenation, RTL and CJK.
- **AC:**
  - B3: 1,000 pages < 3 s, first hit < 300 ms.
  - Golden queries pass for Arabic, Hebrew, Japanese, Chinese, ligatures and
    hyphenation.

### 1.12 Annotate (MVP scope, no stamps)
- Highlight, underline, strikeout, squiggly, sticky note, text box, pen +
  eraser, rectangle, oval, line, arrow.
- Properties: colour, opacity, width, fill, author, subject, dates.
- Replies. The comments pane (list, jump, filter by type/author/page,
  delete). Show or hide all.
- Appearance streams per spec. Non-Latin text uses system fonts via fontdb,
  embedded only if `fsType` allows it.
- All other annotation types are displayed and preserved.
- **AC:**
  - Round trip for each type.
  - They render in PDF.js, Poppler and PDFium.
  - Acrobat-created annotations are preserved.

### 1.13 Fill forms
- AcroForm filling: text (single, multi-line, comb), checkbox, radio,
  combo, list box. Tab order, appearance generation, NeedAppearances, reset.
- **Native AF subset (ADR-006):** an in-house tokenizer and strict
  recognizer (no third-party JS parser) plus Rust implementations of the
  allowlisted `AF*` functions.
- **Adobe boilerplate recognizer:** Adobe's known viewer-version and XFA
  check document scripts are matched by normalized-token fingerprint and
  treated as no-ops.
- Forms with other scripts show a banner.
- XFA: static XFA with an AcroForm fallback is fillable; dynamic XFA is
  read-only with a notice.
- Flat forms: click to type, plus ✓ ✗ • marks.
- **AC:**
  - Fill + save round trip on 30 corpus forms.
  - Values display in PDF.js, Poppler and PDFium.
  - AF formatting matches the Acrobat reference for ≥ 200 cases.
  - Coverage on the full corpus is reported. The target is ≥ 90% of IRS-type
    forms and a published figure for the rest (the preliminary result is in
    ADR-006).

### 1.14 Organise pages (MVP scope)
- Page grid with resizable thumbnails, drag reorder, multi-select.
- Rotate (per page, range, odd/even), delete, duplicate.
- Insert from a PDF or a blank page. Extract (one file, or one per page).
- Merge (reorder, keep bookmarks, resolve field-name collisions).
- **Split by ranges.**
- **AC:**
  - Round trip + interop for every command.
  - Exact undo.
  - A 100-file merge keeps outlines and fields working.

### 1.15 Save safely and recover
- Save (incremental), Save As, and the "Save optimized" suggestion.
- The signature-aware policy.
- The recovery dialog, including the "last action didn't finish" prompt.
- External-change detection. Engine-crash replay.
- **AC:** the E2E crash test covers edit → kill → relaunch → restore →
  save → verify. Recovery is refused, with an explanation, when the original
  changed.

### 1.16 Opt-in security update check
- First-run choice (no default), Preferences → Privacy, a signed
  `updates.json` from GitHub Releases, the security banner, a verified
  download (no silent install), and a policy override (ARCHITECTURE §9.2).
- Key ceremony: the root keys generated offline on hardware keys and the
  release key in a protected GitHub environment, documented in
  `docs/KEYS.md` (§9.3).
- **AC:**
  - Nothing is fetched unless opted in (a test with a network-deny proxy).
  - Tampered or unsigned `updates.json` is ignored.
  - A revoked key in `keyring.json` is rejected.
  - The request carries no identifiers (checked in the proxy log).

### 1.17 Printing: **owner decision pending** (estimate in ARCHITECTURE §11.2)
- Basic print on all three platforms is **21–26 working days (~4–5
  weeks)**:
  - macOS 3–4 days (PDFKit, vector);
  - Windows 8–10 days (Win32 dialog; the sandboxed renderer produces EMF and
    the host plays it into the printer DC);
  - Linux 4–5 days (GTK print dialog; PDF straight to CUPS, with the portal
    in Flatpak);
  - shared pipeline 4–5 days;
  - CI virtual printers 2 days.
- The options:
  - (a) all in v0.1;
  - (b) all in v0.1.x;
  - (c) macOS + Linux in v0.1 and Windows in v0.1.x.

### 1.18 Release
- Unsigned installers within budgets.
- The first signed `updates.json`.
- `docs/MILESTONE_v0.1_REPORT.md`.

---

## v0.1.x — MVP follow-ups (small releases after v0.1)
- **CLI:** `papyrine info [--json]`, `validate --structure`, `merge`,
  `split`, `update check`. Exit codes, `--json`, progress on stderr.
  Snapshot tests on 20 files.
- **Multi-document search:** across all open documents, with grouped
  results.
- **Two-page facing mode**, with or without a cover page, and continuous
  facing.
- **Stamps:** static built-ins (Approved, Draft, Confidential, Final, Not
  Approved).
- **Split every N pages**, with naming templates.
- **Printing,** if the owner picks option (b) or (c) in 1.17.
- Each item ships with the same AC standard (round trip, interop, E2E,
  budgets).

### Deferred from the old Phase 1, and where each item went
| Item | Milestone |
|---|---|
| CLI, multi-document search, facing mode, stamps, split-every-N | v0.1.x |
| Linearized (Fast Web View) output; content-stream interpreter | v0.2 |
| Signatures pane | v0.3 |
| Attachments, layers and destinations panes; rulers, grids, guides | v0.4 |
| Tags pane, content order; invert colours, custom page background, Read Aloud; XMP/custom metadata/initial-view editors | v0.7 |
| Multiple windows + tab dragging; split and side-by-side views; presentation and reading modes; marquee zoom, loupe, rotate view; Advanced Search (folders, regex, catalogue) | v0.8 |

---

## v0.2 — Compress (flagship) and Organise+
- 2.1 `papyrine-content` interpreter (effective DPI of every image usage,
  taking the max over usages).
- 2.2 `papyrine-codecs`:
  - mozjpeg (trellis, optimized Huffman, 4:4:4/4:2:0);
  - JPEG quality estimation from quantization tables;
  - OpenJPEG (lossy and lossless);
  - JBIG2 **generic-region lossless** (default everywhere) and the
    symbol/text-region **lossy mode as explicit opt-in only** (ADR-018);
  - CCITT G4;
  - Flate with predictor selection;
  - resampling (bicubic, bilinear, area, Lanczos);
  - grey and bi-tonal detection;
  - lcms2 conversions;
  - decoding of every PDF image filter (PDFium for JBIG2/JPX/CCITT decode).
- 2.3 Space audit (every spec §10.1 category), top-20 objects with
  click-to-locate, and the per-image table.
- 2.4 The optimization pipeline: every spec §10.3 technique, each toggleable
  in Custom. "Never bigger" per object. Generational-loss guard. Linearize.
- 2.5 Presets. Mono images use **lossless JBIG2 in every preset, including
  Maximum.** The pre-run size estimate is shown.

  | Preset | Colour/grey | Mono |
  |---|---|---|
  | Lossless | no resampling, lossless re-encode | JBIG2 generic lossless or CCITT G4, whichever is smaller |
  | Print | 300 dpi if > 450, JPEG q≈90 | 1200 dpi, JBIG2 lossless |
  | Balanced (default) | 150 dpi if > 225, JPEG q≈75–80 | 300 dpi, JBIG2 lossless |
  | Screen | 96–110 dpi, JPEG q≈60–65 | 200 dpi, JBIG2 lossless |
  | Maximum | 72 dpi, JPEG q≈45–50 or JPEG 2000 | 150 dpi, JBIG2 lossless |
  | Custom | everything exposed; **"JBIG2 lossy (symbol matching)"** under Advanced, off by default | |

  Enabling lossy JBIG2 shows a warning dialog every time: "It can silently
  replace characters with similar-looking ones, e.g. 6 ↔ 8. Don't use it for
  numbers, legal or medical documents." The report records it, and the CLI
  needs `--jbig2-lossy`, which prints the same warning to stderr.
- 2.6 Target-size mode (a search over DPI × quality, largest images first,
  with a readability floor and an "impossible" report).
- 2.7 Safety:
  - new file by default;
  - the before/after report;
  - discard if larger;
  - PDF/A, PDF/UA and signature detection, with restricted techniques or
    warnings;
  - verification (re-parse, render every page, SSIM + SSIMULACRA2, optional
    veraPDF).
- 2.8 Visual diff preview (synchronized zoom, blink, difference overlay).
- 2.9 Batch compression with CSV report. CLI `compress` and `audit`.
- 2.10 Organise+:
  - crop tool, auto-remove margins, numeric page boxes, change page size;
  - page labels editor;
  - split by size, top-level bookmarks or blank separators;
  - reverse, interleave;
  - insert from images or clipboard;
  - drag pages between documents;
  - N-up / booklet export.
- 2.11 Benchmark B5 **gated per file** (ARCHITECTURE §13, ADR-016);
  `docs/COMPRESSION.md`.
- **AC:**
  - B5 green for every file × preset.
  - 100-page 300 dpi colour scan in < 60 s on the reference Mac.
  - The comparison vs qpdf-only, pikepdf-only and (if supplied) Acrobat
    outputs is published in COMPRESSION.md.
  - The JBIG2 lossy opt-in is covered by a test asserting it is never
    enabled by any preset or default.

## v0.3 — Sign and Protect
- 3.1 Electronic signatures (type, draw, image; profiles in the OS
  keychain). The toolbar's "Fill" becomes "Fill & Sign".
- 3.2 Digital IDs: PKCS#12, Windows store, macOS Keychain, PKCS#11;
  self-signed IDs.
- 3.3 Signing: adbe.pkcs7.detached and ETSI.CAdES.detached; PAdES B-B, B-T,
  B-LT, B-LTA; appearances; certification (MDP); incremental only.
- 3.4 Validation (byte range, chain building, OCSP/CRL, timestamps,
  post-signing modification analysis, view the signed revision); the
  signatures pane.
- 3.5 Encryption (AES-256 default, AES-128, RC4-128 with a warning; scopes;
  permissions; certificate encryption; remove security).
- 3.6 True redaction (marking, search-and-redact with patterns and word
  lists, real content removal across text, images, paths, form values,
  annotations and hidden OCR text; overlay appearance and codes; new file by
  default).
- 3.7 Sanitize (checklist with counts; removal of previous revisions).
- **AC:**
  - pyHanko + EU DSS (test-only) validate every PAdES level.
  - The redaction and sanitize leakage suites find zero leaks.
  - Encryption interop passes.

## v0.4 — Edit
- 4.1 Layout analysis (blocks, paragraphs, columns) with a golden-set
  evaluation.
- 4.2 Paragraph text editing with font preservation, the fallback chain
  with a user notice, formatting, lists, reflow, vertical text, and all font
  types.
- 4.3 New text boxes; spell check; find and replace as content edits with a
  preview.
- 4.4 Image and object editing; the object inspector (Advanced tier).
- 4.5 Headers and footers, watermarks, backgrounds, Bates, links, a
  bookmarks editor, layers pane + editor, attachments pane + editor, the
  destinations pane, rulers/grids/guides.
- **AC:**
  - Text-edit round trips on 50 real documents with correct extraction
    afterwards.
  - Visual regression outside edited regions ≤ threshold.
  - Font substitution is always reported.

## v0.5 — Forms Pro, Comments Pro, first components
- 5.1 **Component mechanism** (ARCHITECTURE §9): catalog, signatures,
  install from network or file, manager UI, CLI `components`, helper
  launch.
- 5.2 **`form-scripts` component** (ADR-006 decision after Spike 0.4 and a
  compatibility run on the corpus of JS forms).
- 5.3 Prepare Form: auto field detection, every field type and property,
  formats, validation, calculations, actions, the tab-order editor,
  duplicate/grid/align.
- 5.4 Form data: FDF, XFDF, XML, CSV import/export; merge data files into a
  spreadsheet; flatten.
- 5.5 The remaining annotation features: callout, polygon, polyline, cloud,
  insert/replace-text markups, file attachment, sound, dynamic and custom
  stamps with a library, status and check marks, bulk edits, FDF/XFDF
  comments, import from another version, comment summary, flatten,
  pressure-sensitive ink.
- **AC:**
  - A component installs offline from a file.
  - A tampered component is rejected.
  - Form-scripts behaviour matches Acrobat reference results on the JS-forms
    corpus at the rate set in ADR-006.
  - The sandbox-escape suite passes.

## v0.6 — Scan, OCR, Create, Export
- 6.1 `ocr-engine` and `ocr-lang-*` components; searchable and editable
  modes; suspect-word review; skip pages that already have text.
- 6.2 Scan enhancement (deskew, rotate, whitening, borders, despeckle,
  contrast, perspective).
- 6.3 The **`mrc` component**; MRC presets appear once it is installed.
- 6.4 Scanner support.
- 6.5 Create from images (HEIC per ADR-013), Office (LibreOffice), HTML
  (platform webview), clipboard.
- 6.6 Export to DOCX, XLSX, PPTX, RTF, TXT, HTML, XML, Markdown and images;
  export all images.
- **AC:**
  - OCR word accuracy ≥ 95% on clean scans.
  - MRC ≥ 5× smaller than Balanced on colour scans, with OCR text intact.
  - The DOCX evaluation set runs, and its limitations are documented.

## v0.7 — Standards and Accessibility
- 7.1 PDF/A (1b through 4) and PDF/X (1a, 3, 4) conversion and validation;
  preflight; output preview; colour conversion; printer's marks.
- 7.2 Accessibility checker, autotag, Reading Order tool, tags pane and
  editor, guided alt text.
- 7.3 Read Aloud, invert colours and custom page background; XMP editor,
  custom metadata, initial-view editor.
- 7.4 The app's own accessibility, audited with VoiceOver, NVDA/Narrator and
  Orca.
- **AC:**
  - PDF/A output passes veraPDF.
  - Our validator agrees with veraPDF on ≥ 99% of corpus verdicts.
  - Autotagged Matterhorn samples pass the PDF/UA machine checks.

## v0.8 — Compare, Automation, Power features
- 8.1 Compare files (text, formatting, images, annotations, pages; report
  PDF; pixel mode).
- 8.2 Action Wizard, macro recording, watched folders.
- 8.3 Measure tools, portpapyrines, rich media, 3D poster fallback.
- 8.4 Advanced printing (N-up, booklet, poster, print as image, …).
- 8.5 Multi-window with tab dragging; split and side-by-side views;
  presentation and reading modes; marquee zoom and loupe; rotate view.
- 8.6 Advanced Search (folders, regex, attachments, catalogue index with
  tantivy).
- 8.7 The full CLI (every spec §13 command).

## v1.0 — Release
- Code signing and notarization (once certificates exist), signed
  auto-update, in-app help plus the user guide PDF, localization, a
  performance pass, the shortcut editor and cheat sheet, complete
  preferences.
- **AC:** the spec §18 Definition of Done, verified item by item, plus every
  budget gate green.

---

## Cross-cutting, every milestone
- Every command gets a round trip, an interop check, an undo test and the
  shadow verifier.
- Every parser or decoder gets a fuzz target.
- Every dependency goes through the license gate, the notices file, and an
  ADR if it is significant.
- No new work may run before first paint without an allowlist change
  justified in review.
- Commit and push to `github.com/manjunathsharma10/papyrine` at the end of
  every task.
