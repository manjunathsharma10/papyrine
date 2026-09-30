# Folio — Roadmap

Status: **Draft v2 for approval** · Last updated: 2026-09-30

**Revision 2 changes:**
- Restructured around an **MVP (v0.1)** as the first real milestone
  (ADR-020). The spec's nine phases are replaced by milestones v0.1 → v1.0.
- A **Step 0 of de-risking spikes** comes first, including benchmark B1
  (render snapshots).
- Old Phase 1 items not needed for the MVP are deferred and listed
  explicitly.

**Every milestone ends with:**
- an installable build (macOS verified on the reference machine; Windows and
  Linux in CI);
- all budget gates green (ARCHITECTURE §1.1);
- `docs/MILESTONE_vX.Y_REPORT.md`, which replaces the spec's
  PHASE_N_REPORT, with measured budgets and deviations.

**Legend:**
- **AC** = acceptance criteria.
- **Interop check** = passes `qpdf --check`, renders without errors in
  PDFium, PDF.js and Poppler (test-only), and re-opens in Folio.
- **Round trip** = open → command → save → reopen → verify, plus undo
  restores objects byte-identically, plus the shadow verifier finds no
  unrecorded change.

| Spec phase | Now lives in |
|---|---|
| 1 Foundation | Step 0 + v0.1 (trimmed; deferrals below) |
| 2 Organize + Compress | v0.1 (core organize) + v0.2 (compress, advanced organize) |
| 3 Comment + Fill | v0.1 (core annotate, fill) + v0.5 (the rest) |
| 4 Edit | v0.4 |
| 5 Forms + Protect | v0.3 (protect) + v0.5 (form authoring, form scripts) |
| 6 Scan/OCR/Create/Export | v0.6 |
| 7 Sign + Compare | v0.3 (sign) + v0.8 (compare) |
| 8 Standards + Accessibility | v0.7 |
| 9 Automation + Polish | v0.8 + v1.0 |

---

## Step 0 — Decisions and de-risking spikes

These are throwaway-quality code in `spikes/`, not shipped. Findings go into
ADRs.

| Task | What | Exit criteria |
|---|---|---|
| 0.0 | **Product name** decided (ADR-019) | Name chosen by the owner. Crates, bundle ID and GitHub repo named accordingly. **No crates or bundle ID are created before this** |
| 0.1 | Bare Tauri 2 window + static tile on macOS, Windows and Linux | Measured installer size, cold/warm launch, and idle memory of the bare shell per OS. If the bare shell alone breaks a budget in ARCHITECTURE §1.1, **stop and report to the owner** |
| 0.2 | qpdf C++ shim (via `cxx`) + incremental section writer prototype + PDFium custom reader. **Benchmark B1** (ARCHITECTURE §4.6) | B1 report: re-open time and RSS vs number of sections `k` and section size, on the 2,000-page, 1,000-page image-heavy and typical-20p files. Decide the compaction thresholds, or switch to command mirroring. **Nothing else builds on snapshots until this is done** |
| 0.3 | qpdf vs lopdf on the corpus | Open and repair success rates, peak RSS on the 2,000-page file, time to first object. Confirms or reopens ADR-002 |
| 0.4 | PDFium form-widget + native AF-subset feasibility | Fill a text field with `AFNumber_Format` in PDFium's form-fill environment without JS, with our formatter producing the appearance. Confirms the v0.1 forms plan (ADR-006) |

---

## v0.1 — MVP: open, view, search, annotate, fill forms, organise pages, save safely

**Definition:** a person can use Folio as their everyday PDF app for reading,
marking up, filling forms and rearranging pages, and **never lose work or
corrupt a file**. Only v0.1 features appear in the UI.

### 1.1 Repository, tooling, CI
- Cargo and pnpm workspaces; strict lint and format; LICENSE-MIT,
  LICENSE-APACHE, README, SECURITY.md, CONTRIBUTING.md.
- CI matrix: macOS arm64/x64, Windows x64/arm64, Ubuntu x64.
- **Gates live from day one:**
  - license (shipped Rust / shipped JS / native manifest, separate from test
    oracles);
  - notices freshness;
  - bundle inspection;
  - no-corpus-in-repo;
  - budgets (size, launch, idle memory, startup-trace allowlist, JS bundle).
- **AC:**
  - A clean clone builds and tests with the documented prerequisites.
  - Deliberate violations fail CI on a scratch branch:
    - a GPL crate;
    - a banned native library in the bundle;
    - a committed PDF;
    - a stale notices file;
    - a 60 MB installer;
    - a subsystem initialized before first paint.

### 1.2 Test corpus (download and generate at test time only)
- `corpus/manifest.toml` + `corpus/fetch` (verified by SHA-256) + a
  `tools/gen-corpus` synthetic generator. The cache is gitignored.
- **AC:** ≥ 500 files including ≥ 50 malformed. The license of each file is
  recorded. The CI cache works on all OSes.

### 1.3 `qpdf-sys` (C API + C++ shim) and `folio-cos`
- Vendored qpdf ≥ 12 (native crypto).
- The shim functions listed in ARCHITECTURE §4.1.
- The safe Rust API and the RepairLog.
- **AC:**
  - Every corpus file either opens or fails with a typed error, with no
    panic, crash or hang (30 s).
  - Every synthetic malformed file opens and logs repairs.
  - Every encryption revision opens with the right password and is refused
    with the wrong one.
  - Fuzz target: 10 min per PR, nightly 1 h.

### 1.4 `folio-content` (lexer, parser, serializer)
- **AC:**
  - Parse → serialize → parse is identical across every corpus content
    stream.
  - The fuzz target is clean.

### 1.5 `folio-model` (MVP read side)
- Page tree, boxes, rotation, page labels (display), outlines, AcroForm
  fields, annotations, Info, the encryption and permissions summary,
  signature presence (for the save policy), the linearized flag and the
  version.
- **AC:** values match expectations for 30 curated files. No panics on the
  corpus.

### 1.6 `folio-ops` + change-driven journal
- `Command`, `EditContext`, `ChangeSet`, `History`, `CompositeCommand`, the
  shadow verifier, and journal records with group-commit fsync ≤ 1 s,
  checkpoints and replay (ARCHITECTURE §7).
- **AC:**
  - A property test of random command sequences and undo/redo restores the
    exact state.
  - Kill -9 the whole app at random points during an editing script (1,000
    iterations, in CI): recovery restores every command whose record was
    fsynced, and the loss window is ≤ 1 s of commands.

### 1.7 `folio-writer`
- Incremental sections (table or stream to match the original; encrypted
  docs; hybrid files).
- ID-preserving full write.
- qpdf optimized rewrite with history rebasing.
- Atomic replace with validation.
- The save policy (ARCHITECTURE §4.5).
- **AC:**
  - Incremental save of signed corpus files leaves the original bytes as an
    exact prefix, and pyHanko (test-only) reports the signatures intact.
  - Interop check for all three outputs on 100 corpus files.
  - Fault injection at every atomic-replace step never corrupts or loses
    the target.

### 1.8 Renderer, tiles, snapshots
- One render worker, the tile pipeline, the `folio://` transport, and caches
  sized to budget.
- Snapshots with compaction using the thresholds from B1.
- Crash restart.
- **AC:**
  - Golden-image match on 50 pages.
  - First page < 500 ms for the 20-page file and < 2 s for the 2,000-page
    file on the reference Mac.
  - Edit → updated tile < 100 ms p95.
  - Renderer RSS growth < 10 MB over a 500-edit session.
  - Killing the renderer causes only a brief re-render.

### 1.9 Process split and sandbox
- Single multi-role executable. Engine and renderer sandboxed on macOS
  (Seatbelt), Linux (landlock + seccomp) and Windows (restricted token + Job
  object).
- **AC:** inside each sandbox these attempts are all denied: reading `~`,
  writing outside temp, opening a socket, spawning a process. Normal
  operation is unaffected.

### 1.10 Viewer UI (MVP scope)
- App shell: design tokens; light, dark and high-contrast themes; an
  original icon subset; toolbar with **core tools only**; **All Tools**
  panel; command palette; left pane; right tool pane; status bar; i18n
  scaffolding (English, plus a pseudo-locale and an RTL layout test).
- Documents open in tabs in one window, via the open dialog, drag and drop,
  recent files (thumbnails, pinned) and OS file associations.
- View modes: single page, continuous, two-page facing (with or without
  cover), and simple full screen.
- Zoom: fit page, fit width, actual size, presets, 10%–6400%, Ctrl/⌘-wheel
  and trackpad pinch.
- Panes: thumbnails and bookmarks (navigate).
- Navigation: go to page, back/forward history, full keyboard navigation.
- Document Properties: view everything in the v0.1 model; edit title,
  author, subject and keywords.
- Banners: repaired file, file changed on disk; prompts for URI and launch
  actions.
- **AC:**
  - E2E: open, view modes, zoom, tabs, properties edit + save + reopen, and
    keyboard-only use of every v0.1 control.
  - B4 scroll p95 ≤ 16.7 ms.
  - axe-core reports no serious or critical issues.
  - VoiceOver can read the page text layer.
  - All strings are in the catalog.

### 1.11 Search (MVP scope)
- Find bar (⌘/Ctrl-F): whole word, case-sensitive, diacritic-insensitive,
  include comments. Searches the current document or all open documents,
  with a results list and snippets, streaming and cancellable. Covers
  annotation contents and form field values. Handles ligatures, hyphenation,
  RTL and CJK.
- **AC:**
  - B3: 1,000 pages < 3 s, first hit < 300 ms.
  - Golden queries pass for Arabic, Hebrew, Japanese, Chinese, ligatures and
    hyphenation.

### 1.12 Annotate (MVP scope)
- Create, edit, move and delete: highlight, underline, strikeout, squiggly,
  sticky note, text box (free text), pen (ink) + eraser, rectangle, oval,
  line, arrow, and static stamps (Approved, Draft, Confidential, Final, Not
  Approved).
- Properties: colour, opacity, stroke width, fill, author, subject, dates.
- Threaded replies.
- Comments pane: list, jump to, filter by type/author/page, delete. Show or
  hide all.
- Appearance streams are generated per spec. Text in non-Latin scripts uses
  system fonts via fontdb, subset-embedded only when the font's `fsType`
  permits embedding; otherwise the user picks another font.
- All other existing annotation types are displayed and preserved.
- **AC:**
  - Round trip for each type.
  - They render in PDF.js, Poppler and PDFium.
  - Acrobat-created samples in the corpus keep their appearance and
    properties after Folio edits other annotations.

### 1.13 Fill forms (MVP scope)
- AcroForm filling: text (single, multi-line, comb), checkbox, radio, combo,
  list box.
- Tab order; appearance generation; NeedAppearances handling; reset form.
- **Native AF subset, no JS engine** (ADR-006). A strict recognizer runs a
  field script only when the script consists *solely* of standard calls with
  literal arguments:
  - `AFNumber_Format/Keystroke`, `AFPercent_*`, `AFDate_*`, `AFTime_*`,
    `AFSpecial_*`, `AFRange_Validate`;
  - `AFSimple_Calculate` (SUM, PRD, AVG, MIN, MAX) with calculation order.
- Forms with any other script show a banner: "This form uses scripts that
  Folio doesn't run yet; calculations may not update."
- XFA: static XFA with an AcroForm fallback is shown and fillable. Dynamic
  XFA shows a read-only notice.
- **Fill on flat forms:** click to type text, and add ✓ ✗ • marks.
- **AC:**
  - Fill + save round trip on 30 corpus forms.
  - Values display in PDF.js, Poppler and PDFium.
  - The AF formatting outputs match Acrobat reference strings for a table
    of ≥ 200 cases.

### 1.14 Organise pages (MVP scope)
- Page grid with resizable thumbnails; drag to reorder; multi-select.
- Rotate (per page, or a range with odd/even); delete; duplicate.
- Insert pages from a PDF or a blank page; extract (as one file or one file
  per page).
- **Merge:** combine files, reorder, keep bookmarks, resolve form-field name
  collisions.
- **Split:** by ranges or every N pages, with naming templates.
- **AC:**
  - Round trip + interop for every command.
  - Undo is exact.
  - A merge of 100 corpus files keeps outlines and fields working.

### 1.15 Save safely and recover
- Save (incremental by default), Save As, and the "Save optimized"
  suggestion.
- Signature-aware policy: never invalidate existing signatures without an
  explicit warning.
- Recovery dialog.
- External-change detection.
- Engine-crash replay ("no changes lost").
- **AC:** the E2E crash test covers edit → kill → relaunch → restore →
  save → verify. Recovery is refused, with an explanation, when the original
  changed.

### 1.16 CLI (MVP scope)
- `folio info [--json]`, `folio validate --structure`, `folio merge`,
  `folio split`.
- Exit codes, `--json`, progress on stderr.
- **AC:** snapshot tests on 20 corpus files.

### 1.17 Basic printing — **proposed addition, needs owner approval**
- The native OS print dialog, page ranges, fit or actual size, and
  print-with-comments.
- It isn't in the owner's MVP list, but a PDF app without print is hard to
  use daily. Cost: ~1 week. If declined, printing moves to v0.8.

### 1.18 Release
- Unsigned installers for every target within budgets.
- `docs/MILESTONE_v0.1_REPORT.md`.

### Deferred from the old Phase 1, and where each item went
| Item | Milestone |
|---|---|
| Linearized (Fast Web View) output option | v0.2 |
| Content-stream interpreter | v0.2 |
| Multiple windows, dragging tabs between windows | v0.8 |
| Split view; side-by-side synchronized view | v0.8 |
| Presentation mode with transitions; reading mode | v0.8 |
| Marquee zoom, loupe, rotate view | v0.8 |
| Attachments pane | v0.4 |
| Layers pane | v0.4 |
| Signatures pane | v0.3 |
| Tags pane, content order | v0.7 |
| Destinations pane | v0.4 |
| Rulers, grids, guides | v0.4 |
| Invert page colours, custom page background, Read Aloud | v0.7 |
| XMP editor, custom metadata, initial-view settings editor | v0.7 |
| Advanced Search (folders, regex, attachments, catalogue index) | v0.8 |
| CLI beyond the four MVP commands | v0.2 onwards |

---

## v0.2 — Compress (flagship) and Organise+
- 2.1 `folio-content` interpreter (effective DPI of every image usage,
  taking the max over usages).
- 2.2 `folio-codecs`:
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
- 8.3 Measure tools, portfolios, rich media, 3D poster fallback.
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
- Commit and push at the end of every task, once the owner has confirmed the
  GitHub account.
