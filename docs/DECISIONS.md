# Papyrine — Architecture Decision Records

Each ADR has a status (Proposed / Accepted / Superseded), context, decision
and consequences. Once an ADR is **Accepted**, it is never edited, only
superseded. Proposed ADRs may be revised during review; revision 2 applied the owner's
first review and revision 3 the second (2026-09-30). Revision 4 (2026-10-02)
records the Step 0 results as ADR-026 to ADR-041. The owner delegated these
decisions on 2026-10-02, so they are Accepted; the items needing the owner's
hardware are listed in [STEP_0_REPORT.md](STEP_0_REPORT.md). Amendments to
earlier ADRs are made by a new ADR ("Amends ADR-0xx"), not by editing the old one.

| # | Title | Status |
|---|---|---|
| 001 | License MIT OR Apache-2.0; permissive shipped dependencies | Proposed |
| 002 | qpdf via C API + C++ shim is the only object model; lopdf rejected | Proposed |
| 003 | Tauri 2 + Rust + React/TypeScript | Accepted |
| 004 | Single multi-role executable; host + engine + **one** renderer | Proposed |
| 005 | Render snapshots + compaction, benchmark first; mirroring keeps qpdf as truth | Proposed (rev 3); confirmed and amended by ADR-029 |
| 006 | Form JavaScript: native AF subset now (measured coverage), component later | Proposed (rev 3); confirmed and amended by ADR-030 |
| 007 | Pure-Rust text shaping and fonts | Proposed |
| 008 | LGPL/GPL software only as optional runtime integrations or test oracles | Proposed |
| 009 | Tiles, √2 zoom buckets, raw RGBA transport, budget-sized caches | Proposed |
| 010 | Lightweight budgets as CI gates (incl. large-document memory) | **Accepted** |
| 011 | Optional components mechanism | Proposed |
| 012 | Write-ahead journal in the host replaces timer autosave | Proposed (rev 3) |
| 013 | HEIC via OS decoders; specific Windows HEVC message | Proposed (rev 3) |
| 014 | Generated third-party notices | Proposed |
| 015 | License gate: shipped vs test-only inventories + bundle inspection | Proposed |
| 016 | Image-quality metric and per-file compression gates | Proposed |
| 017 | Test corpora are never committed | Proposed |
| 018 | JBIG2 lossless by default; lossy only as explicit opt-in | Proposed |
| 019 | Product name: Papyrine | **Accepted** |
| 020 | MVP-first milestone plan (v0.1 → v0.1.x → … → v1.0) | **Accepted** |
| 021 | qpdf is built with native crypto only | Proposed |
| 022 | Opt-in security update check | Proposed |
| 023 | Signing-key custody and rotation | Proposed |
| 024 | Code signing and notarization of downloadable helpers | Proposed |
| 025 | Printing via native platform APIs | Proposed; timing decided by ADR-028 |
| 026 | Linux memory accounting counts the app's own memory; DMABUF renderer off | Accepted (owner, 2026-10-02) |
| 027 | AppImage dropped; Linux ships .deb, .rpm and Flatpak | Accepted (owner, 2026-10-02) |
| 028 | Printing timing: macOS + Linux in v0.1, Windows in v0.1.x | Accepted (lead, 2026-10-02) |
| 029 | Step 0 confirms the snapshot-section design; mirroring fallback not adopted | Accepted |
| 030 | ADR-006 confirmed by Spike 0.4 coverage; boilerplate fingerprint; execute per script | Accepted |
| 031 | AF behaviour deviations from PDFium | Accepted (7 cases unverified against Acrobat) |
| 032 | Licence allowlist adds LicenseRef-AGG-2.3 (native manifest only) | Accepted |
| 033 | Native libraries: pinned tarballs, Release-profile builds, no pkg-config | Accepted |
| 034 | qpdf shim error contract | Accepted |
| 035 | Renderer: pdfium-render as loader and raw bindings only; tile format and flags; warm-up | Accepted |
| 036 | Launch measurement via LaunchServices; `drawn` and `presented` stages | Accepted |
| 037 | Windows tile URL and CSP | Accepted |
| 038 | Frontend toolchain pin | Accepted |
| 039 | Content-stream round-trip rules; `papyrine-core` has no dependencies | Accepted |
| 040 | Corpus pinning: Wayback snapshots allowed; operational "malformed" | Accepted |
| 041 | Gate conventions: native licence-file paths, startup allowlist, MiB | Accepted |

**Pending (Wave 2 of Step 0, 2026-10-02):** Spike 0.3 (qpdf vs lopdf on the
corpus; it confirms or reopens ADR-002) and the qpdf large-document memory
run (ARCHITECTURE §1.2). ADR-002 stays as written until they report.

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

## ADR-005: Render snapshots: sections + compaction, benchmark first; mirroring fallback keeps qpdf as sole truth
**Context:**
- The renderer must show engine edits.
- Appending an incremental section per edit chains over a long session, and
  every reload has a cost.

**Decision:**
- L0: one section per commit with only its changed objects.
- **L1 merge** when there are more than 16 sections or re-open exceeds
  30 ms.
- **L2 rebase** (an ID-preserving full write becomes the new base) above
  max(16 MB, 25% of the base).
- Re-open is debounced ~50 ms.
- Tiles are invalidated only for the affected pages.
- **Benchmark B1 (Spike 0.2) comes first** and fixes the thresholds.
- **Targets:** edit → updated tile < 100 ms p95; renderer RSS growth
  < 10 MB over 500 edits.

**If B1 fails: command mirroring**, with consistency rules (ARCHITECTURE
§4.6):
1. qpdf is the only source of truth. The PDFium document is a display-only
   replica, and nothing is saved, exported or journalled from it.
2. Mirroring happens only after the qpdf commit succeeds.
3. Mirror ops are declarative, derived from the ChangeSet: the same
   appearance-stream bytes and the same values.
4. Only whitelisted command types are mirrored. Anything else, or any
   rejected op, triggers an immediate re-open.
5. Epoch-tagged tiles plus a periodic **resync** from a qpdf snapshot (at
   idle, every 50 ops, or every 30 s) bound divergence in time.
6. Undo is mirrored as the inverse op, or falls back to a re-open.
7. CI requires pixel identity between replica and snapshot renders for every
   mirrored command, plus 1,000-op randomized sequences.
8. Form typing never runs in PDFium's form-fill environment.

**Consequences:**
- Every edit exercises the incremental writer.
- Under mirroring, the worst case is a short-lived display difference. It
  never affects saved data.

## ADR-006: Form JavaScript: options compared; native AF subset now, component later
**Context:**
- PDFium's form scripting layer (`fxjs`: app, doc, field, event and util
  objects plus the `AF*` built-ins) is written against V8's API, so another
  engine means reimplementing that object model.
- Measured PDFium mac-arm64 (chromium/8076):
  - without V8: **6.9 MB** on disk, 3 MB compressed;
  - with V8 (+XFA): **41.6 MB** on disk, 12 MB compressed.

| | (a) PDFium built with V8 | (b) QuickJS + our Acrobat JS API | (c) Defer form JS entirely |
|---|---|---|---|
| Size | +35 MB on disk, +9 MB download; the whole renderer library swaps | +~1 MB | 0 |
| Effort | Low integration (weeks). But form interaction then lives inside PDFium, so field values must be synced back to the engine as commands, against "engine is the source of truth" | High: event model, Field/Doc/App/util objects, ~60 `AF*` functions, calculation order, quirks; est. 8–12 weeks plus a compatibility tail | None |
| Fidelity | Highest | Good for real-world forms; lower for exotic scripts | Forms fill, but formatting, calculations and validation don't run |
| Security | Large engine (JIT; run `--jitless`), in the sandboxed renderer | Small interpreter, easy caps, no JIT, in a sandboxed helper | Nothing to attack |
| Optional component? | Yes: download the V8 variant, restart the renderer | Yes: a small helper | n/a |

**Parser for the native subset: in-house, no third-party JS parser.**
- A hand-written tokenizer covers JS string literals (with escapes), numbers,
  identifiers, punctuation and comments.
- A strict recognizer accepts only:
  - a sequence of `AFname(arg, …);` statements;
  - where `AFname` is on the allowlist;
  - and each argument is a string, number, boolean, `new Array("…", …)` or
    `["…", …]` literal.
- Estimated at ~500–700 lines of Rust with **no dependencies**, adding
  < 100 KB to the binary. It is licensed MIT/Apache-2.0 as our own code, and
  fuzzed.
- Anything outside the grammar is rejected, never partially executed.
- A full parser (oxc_parser MIT, swc Apache-2.0, boa MIT/Unlicense) was
  considered and rejected. The subset needs no general grammar, and a smaller
  surface is safer and lighter.
- **Adobe boilerplate:** the known Adobe viewer-version and XFA-check
  document scripts are recognized by **normalized-token fingerprint** (not
  substring match) and treated as no-ops. They only prompt users to upgrade
  Acrobat.

**Preliminary measurement (2026-09-30).**

Sample: 136 public government forms downloaded for analysis and not
committed: 112 IRS, 16 OPM, 7 New York State, 1 USCIS. SSA and VA downloads
were blocked. All 136 are AcroForm (113 are XFA hybrids, filled via their
AcroForm side), and 134 contain JavaScript.

| Measure | Result |
|---|---|
| Non-empty scripts that are pure allowlisted AF calls | 1,335 / 2,334 (57%); nearly all keystroke and format triggers |
| JS forms fully covered, strict subset only | 11 / 134 (8%). Blocked by the Adobe boilerplate document scripts in 111 IRS forms |
| **JS forms fully covered, subset + boilerplate recognizer** | **125 / 134 (93%)** |
| …by source | IRS 111/111 · New York 7/7 · OPM 7/15 · USCIS 0/1 |
| **…excluding IRS** (to reduce sample bias) | **14 / 23 (61%)** |
| Main remaining blockers | OPM: checkbox mutual-exclusion button scripts (`this.getField(…).value = …` in `if` blocks, ~630 scripts over 8 forms). USCIS I-9: custom date validation (`util.scand`, RegExp) |

**Caveats:**
- The sample is dominated by IRS forms, which share one pattern.
- "Covered" means the recognizer accepts every script. Behavioural parity
  with Acrobat is tested separately (≥ 200 reference cases, ROADMAP 1.13).
- XFA-internal scripts are not counted because they are not used.

Spike 0.4 repeats this on the full corpus (≥ 150 JS forms from more sources)
and publishes the breakdown.

**Decision:**
1. **v0.1:** (c) + the native AF subset + the boilerplate recognizer. No JS
   engine. Other scripts show a banner.
2. **v0.5:** optional **`form-scripts` component** using **(b) QuickJS**,
   reusing the Rust `AF*` implementations. The measured blockers
   (field-value assignment, show/hide, `util.scand`/`printd`, RegExp) define
   its first compatibility targets.
3. **(a) is the fallback** if (b) matches Acrobat on < 95% of the JS-forms
   corpus after a time-boxed effort. I'd come back to the owner first.

**Consequences:**
- The base install has no JS engine.
- On the sample, 93% of forms work fully with no JS engine (61% outside IRS).
- Form JS is always opt-in.

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
- The `papyrine://` scheme serves raw RGBA, decoded with `createImageBitmap`.
- Caches are sized to the memory budget:
  - tile L1 soft cap 48 MB, trimmed when idle;
  - previews 16 MB (QOI);
  - PDFium page cache 8 pages;
  - disk thumbnails off by default.

## ADR-010: Lightweight budgets as CI gates
**Status:** Accepted (owner, 2026-09-30).

**Decision:** these budgets are CI gates (ARCHITECTURE §1.1):
- Installer ≤ 50 MB for dmg, msi/NSIS, deb, rpm and Flatpak; AppImage
  ≤ 100 MB.
- Cold launch to first page < 1 s on the reference Mac.
- Idle memory 150 / 175 / 200 MB (macOS / Linux / Windows).
- **Large-document memory:** peak ≤ 400 MB, settled ≤ 250 MB, and each of the
  engine and renderer ≤ 120 MB settled, on four large generated files. This
  catches qpdf and PDFium each holding a copy.
- No pre-first-paint work outside a startup allowlist.
- Initial JS ≤ 200 KB gzipped.

The Windows offline installer (bundling WebView2) is exempt and labelled.
Spike 0.1 validates the shell baseline, and 0.2 the large-document baseline.

**Consequences:**
- Per-arch macOS builds.
- A shared mmap with no heap file copies in the engine or renderer.
- Capped caches.
- Code-split UI.
- Lazy subsystems.
- Heavy features become components.

## ADR-011: Optional components mechanism
**Decision:**
- Components are **signed data packs or sandboxed helper executables**,
  never libraries loaded into the host.
- Format: a `.papyrine-component` file (zstd tar) with `component.toml` (SHA-256
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

## ADR-012: Write-ahead journal in the host replaces timer autosave
**Decision:**
- The **host** (outside the sandboxed engine) writes the journal.
- For every command, the host appends an **`Intent`** record before the
  engine receives the command. The record holds params plus all external
  inputs as content-addressed blobs.
- After the engine applies the command, the host appends a **`Commit`**
  record with the after-images.
- Records are CRC32C-checked.
- `fsync` is group-committed ≤ 1 s. Commands with large payloads wait for
  their own fsync.
- Checkpoints every 32 MB or 500 records.

**Replay:**
- `Commit` records apply their after-images deterministically.
- A trailing `Intent` without `Commit` is **not** re-executed automatically.
  The user is asked to Redo or Skip, because the command may have crashed
  the engine. A command that crashes the engine twice is quarantined.

**Consequences:**
- A process crash loses nothing that was requested.
- A power loss loses ≤ ~1 s.
- An engine crash can never corrupt the journal.
- The write-ahead step is a µs-scale buffered append, so there is no
  user-visible latency.
- The journal holds document content, so it has owner-only permissions and
  is excluded from backups where possible.

## ADR-013: HEIC via OS decoders
**Decision:**
- macOS: ImageIO.
- Windows: WIC with Microsoft's *HEIF Image Extensions* and *HEVC Video
  Extensions*. Papyrine detects **which** is missing: no HEIF container
  decoder, vs. a container decoder whose frame decode fails with
  `WINCODEC_ERR_COMPONENTNOTFOUND`. It then shows a specific message: "Can't
  open 'IMG_2041.heic'. Windows needs Microsoft's HEVC Video Extensions to
  read HEIC photos… [Open Microsoft Store] [Convert another way…]". Batch
  imports get one summary rather than a dialog per file.
- Linux: `dlopen` libheif if the system provides it. Otherwise Papyrine shows
  how to install it.
- Papyrine never bundles an HEVC decoder or libheif.

**Consequences:**
- No LGPL or HEVC patent exposure in shipped artifacts.
- Availability differs by platform, and this is documented.

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

## ADR-019: Product name: Papyrine
**Status:** Accepted (owner, 2026-09-30).

**Context:** the working name "Folio" conflicted on several registries,
checked 2026-09-30:
- the Mac App Store "Folio PDF Reader & Editor";
- the iOS "Folio" PDF apps;
- Flathub's GNOME "Folio" notes app;
- crates.io `folio` / `folio-pdf` (a PDF library);
- npm `folio`.

Also rejected: Quire, Sheaf, Octavo, Colophon and Quarto, for PDF-app or
publishing conflicts.

**Decision:** **Papyrine**. It was clear on crates.io, npm, Flathub and the
Apple stores when checked.
- Crates: `papyrine-*`.
- CLI: `papyrine`.
- Bundle ID: `io.github.manjunathsharma10.papyrine` (to move to a custom
  domain if the owner registers one).
- Repo: renamed to `github.com/manjunathsharma10/papyrine`; GitHub redirects
  the old URL.

**Consequences:** a formal trademark search (USPTO, EUIPO, WIPO) is still
recommended before v1.0. Registry checks are not legal clearance.

## ADR-020: MVP-first milestone plan overrides the spec's phase order
**Status:** Accepted (owner approved the direction, 2026-09-30).

**Decision:**
- Step 0 spikes come first.
- Then **v0.1 MVP**: view (single + continuous), search (current document),
  annotate, fill forms, organise pages, and save safely, plus the opt-in
  security update check.
- Then **v0.1.x**: CLI, multi-document search, facing mode, stamps,
  split-every-N, and printing if the owner chooses.
- Then v0.2 Compress, v0.3 Sign & Protect, v0.4 Edit, v0.5 Forms/Comments
  Pro + components, v0.6 Scan/OCR/Create/Export, v0.7 Standards &
  Accessibility, v0.8 Compare/Automation/Power, v1.0 Release.
- Milestone reports replace phase reports.

**Consequences:** the first usable release comes much sooner, and the
flagship compression follows directly.

## ADR-021: qpdf is built with native crypto only
**Decision:**
- Vendored qpdf is built with `DEFAULT_CRYPTO=native` and
  `REQUIRE_CRYPTO_NATIVE=ON`, with OpenSSL and GnuTLS disabled.
- GnuTLS (LGPL) is banned outright. OpenSSL (Apache-2.0) would be
  license-compatible, but it is not used, to avoid ~5 MB and a
  per-platform build.
- qpdf only uses crypto for PDF encryption of local files. New cryptography
  (encrypting incremental sections; signatures in v0.3) uses RustCrypto.
- **The gate:**
  - a CMake-cache check;
  - a runtime test that `QPDFCryptoProvider::getRegisteredImpls()` returns
    exactly `["native"]`;
  - bundle inspection for `libgnutls*`, `libssl*` and `libcrypto*`, plus a
    symbol scan for static linking.

**Consequences:** there are two crypto implementations: qpdf's native one
for its own reading and writing, and RustCrypto for ours. Both are covered by
the encryption interop tests.

## ADR-022: Opt-in security update check
**Decision:**
- Off by default. First run asks once, with no pre-selected answer.
- When enabled, it makes one daily HTTPS GET of a static, Ed25519-signed
  `updates.json` from GitHub Releases. It sends no identifiers, version,
  query string or cookies, and compares versions locally.
- A security release affecting the installed version shows a non-modal
  banner. Download is user-initiated and verified. There is **no silent
  install**; auto-update waits for v1.0 and is opt-in too.
- Policy can disable the check or point it at a mirror. The CLI has
  `papyrine update check`.

**Consequences:** security fixes reach users who opted in, without
telemetry. Users who didn't opt in rely on release announcements.

## ADR-023: Signing-key custody and rotation (updates and component packs)
**Decision:**
- **Two offline root keys** (Ed25519, on hardware security keys stored in
  separate locations, with encrypted paper seed backups). They sign only
  `keyring.json`, and either root alone suffices.
- **Online keys** (release, component and, from v1.0, Tauri updater) live in
  a protected GitHub Actions environment: required reviewer = owner,
  main-branch only, no fork PRs. Each is listed in the root-signed keyring
  with `not_before`/`not_after`.
- **Rotation:** online keys yearly or on suspicion. A root is replaced by an
  app release signed by the other root.
- **Revocation:** a root-signed keyring update. Every app release also
  bundles the latest keyring.
- Dev builds use a separate dev root that release builds reject.
- The ceremony, custody and incident steps are documented in `docs/KEYS.md`.
- Keys are created before the first public release.

**Consequences:**
- Compromise of the CI secrets can be recovered from without shipping a new
  app, by revoking through the keyring.
- Losing both roots would require a manual reinstall. That is mitigated by
  two separate locations plus paper backups.

## ADR-024: Code signing and notarization of downloadable helpers
**Decision:**
- **macOS:** each helper is a minimal app bundle, so it can be stapled. It is
  signed with the app's Developer ID, uses hardened runtime with minimal
  entitlements, and is notarized and **stapled**. On install, the host checks
  `SecStaticCodeCheckValidity` with a Team-ID requirement, in addition to our
  Ed25519 signature.
- **Windows:** Authenticode with an RFC 3161 timestamp. On install,
  `WinVerifyTrust` must pass and the signer must match the app's.
- **Linux:** Ed25519 only; Flatpak extensions where possible.
- The release CI signs, notarizes and staples helpers in the same job as the
  app.
- Until certificates exist, helpers are marked unsigned. Release builds
  refuse them unless a developer setting is enabled, with a warning.

**Consequences:**
- It needs an Apple Developer ID and a Windows signing certificate (EV or
  Azure Trusted Signing) before components ship publicly (v0.5).
- Stapling allows offline first launch.

## ADR-025: Printing via native platform APIs
**Status:** Proposed; timing decided by ADR-028 (macOS + Linux in v0.1, Windows in v0.1.x).

**Decision:** printing never uses the webview.
- The engine produces a normalized "print PDF", with the page subset, print
  flags and appearances resolved.
- **macOS:** PDFKit `printOperation` (vector); PDFium raster as "print as
  image".
- **Windows:** Win32 `PrintDlgEx`. The **sandboxed renderer** produces EMF
  via PDFium, and the host plays it into the printer DC, so no PDF parsing
  happens in the host. Banded raster fallback.
- **Linux:** the GTK print dialog; the PDF goes to CUPS via `GtkPrintJob`,
  with the portal inside Flatpak.
- CI virtual printers on all three.

**Estimate:** 21–26 working days in total: macOS 3–4, Windows 8–10,
Linux 4–5, shared 4–5, test infrastructure 2.

**Consequences:**
- Windows is the costliest platform, because it has no OS PDF printing.
- The quality of print output on macOS and Linux depends partly on Apple's
  and CUPS's PDF handling. The "print as image" option is the escape hatch.

---

## Revision 4 ADRs (Step 0 results, 2026-10-02)

Evidence: [STEP_0_REPORT.md](STEP_0_REPORT.md) and `docs/spikes/`.

## ADR-026: Linux memory accounting counts the app's own memory; DMABUF renderer off
**Status:** Accepted (owner, 2026-10-02). Amends ADR-010 (idle-memory
accounting on Linux only).

**Context:** Spike 0.1 found the bare shell at **260 MB PSS** on the Linux CI
runner against the 175 MB budget. The runner is headless (Xvfb, Mesa llvmpipe,
no GPU). PSS charges the app the shared WebKitGTK, GTK and Mesa library pages
in full when no other application on the machine maps them (about 144 MB of
the 260 MB). Those pages belong to the system, not to Papyrine, and on a
normal desktop they are shared with other programs.

| Linux CI runner, bare shell | PSS (all processes) | Anonymous | File-backed |
|---|---|---|---|
| Default | 260 MB | about 111 MB | about 144 MB |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | 185 MB | about 59 MB | about 125 MB |

**Decision:**
- The Linux idle-memory and large-document budgets count **the app's own
  memory only**: anonymous plus private dirty pages, summed over all app
  processes, excluding shared file-backed library pages. The **175 MB budget
  stays**. macOS (`phys_footprint`) and Windows (private working set) are
  unchanged.
- PSS is still recorded and reported, but is informational.
- The app sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` at startup on Linux, before
  the webview starts, **only if the variable is unset** (so a user or packager
  can override it).
- The measured figure on the headless runner is 59 to 111 MB anonymous, so the
  bare shell passes with room for the app.
- `tools/check-budgets` reads `idle_anon_mb` on Linux; `measure-shell.mjs`
  emits it as the gated value (`max(Pss_Anon, Private_Dirty)` per process).

**Consequences:**
- The Linux budget now measures what Papyrine controls and is stable across
  distributions.
- It is not a claim about total system footprint. A real-GPU Linux desktop run
  (X11 and Wayland) is still wanted to confirm the flag does not hurt
  rendering and that the numbers hold; it needs the owner's hardware.
- The flag trades GPU-composited WebKit rendering for lower memory; the tile
  path is a canvas, so this is acceptable. Revisit if scrolling is slow on real
  hardware.

## ADR-027: AppImage dropped; Linux ships .deb, .rpm and Flatpak
**Status:** Accepted (owner, 2026-10-02). Amends ADR-010 (removes the AppImage
budget) and applies ADR-015.

**Context:** Tauri's AppImage bundles the WebKitGTK stack, including
`libgnutls`, `libnettle`/`libhogweed`, OpenSSL `libcrypto` and `libcups`, all
banned by ADR-015, and the CI bundle job measured it at **261 MB** against the
100 MB budget. (The shell spike's own build measured 77 MB; the two builds
differ in environment and the discrepancy is not reconciled, because the
licence failure alone decides the matter.) It fails the bundle-inspection gate
and, in CI, the size budget.

**Decision:**
- AppImage is **not** a release format. The Linux formats are **.deb, .rpm and
  Flatpak**. Flatpak uses the runtime's WebKitGTK, which is outside our bundle.
- `tauri.conf.json` bundle targets exclude `appimage`; `tools/check-budgets`
  rejects any `.AppImage` input; the 100 MB AppImage budget is removed.
- .deb and .rpm depend on the distribution's `libwebkit2gtk`, so they stay
  about 2 MB.

**Consequences:**
- Users on distributions without .deb/.rpm use the Flatpak.
- Flatpak packaging (a flatpak-builder manifest, the portal printing path,
  size measurement) is v0.1 work; no size is claimed yet.
- If an AppImage is wanted later, it needs a stripped, licence-clean library
  set and a new ADR superseding this one.

## ADR-028: Printing timing: macOS + Linux in v0.1, Windows in v0.1.x
**Status:** Accepted (lead, 2026-10-02). Decides the timing left open in
ADR-025 and ROADMAP 1.17; option (c).

**Context:** the estimate is 21 to 26 working days for all three platforms.
Windows (EMF from the sandboxed renderer played into the printer DC) is the
costliest part at 8 to 10 days, and it cannot be verified by hand on the
reference Mac.

**Decision:**
- **v0.1:** printing on macOS (PDFKit, 3 to 4 days) and Linux (GTK dialog and
  CUPS, 4 to 5 days), plus the shared pipeline (4 to 5 days) and the CI
  virtual printers for those two.
- **v0.1.x:** Windows printing, with CI virtual-printer coverage (for example
  Microsoft Print to PDF) because it cannot be hand-verified on the reference
  Mac.
- ADR-025 is otherwise unchanged. Printing never uses the webview.

**Consequences:**
- About 12 to 14 working days of the estimate land in v0.1 instead of 21 to 26.
- Windows users have no print command in v0.1; the release notes say so.

## ADR-029: Step 0 confirms the snapshot-section design; mirroring fallback not adopted
**Status:** Accepted. Amends ADR-005 and confirms ARCHITECTURE §4.6.

**Context:** Benchmark B1 (Spike 0.2, renderer half, macOS M4, release build)
measured PDFium re-opening a document through a custom file reader over the
shared mapping plus appended incremental sections.

| Measure | Result | ADR-005 target |
|---|---|---|
| Re-open, 20 pages, k = 1..64 sections | 0.01 to 0.17 ms | none |
| Re-open, 2,000 pages (50.8 MB) | **0.4 to 0.6 ms**, about 2.5 us per section, independent of section size | none |
| Edit to updated tile, 500 edits, 20 pages | p95 0.8 ms | < 100 ms |
| Edit to updated tile, 500 edits, 2,000 pages | p95 **7.7 to 11.9 ms** (about 12 ms) | < 100 ms |
| Renderer footprint growth over 500 edits | 0.1 MB (20 pp), **at most 1.0 MB** (2,000 pp) | < 10 MB |
| First tile in a fresh process | 21 to 26 ms (123 ms cold file cache) | none |
| Cancel at first poll | about 1.2 ms | none |

**Decision:**
- The snapshot-section design (L0 per commit, L1 merge, L2 rebase, debounced
  re-open) is **adopted**. The command-mirroring fallback is **not adopted for
  now**; its rules stay documented in ARCHITECTURE §4.6 as the fallback should a
  later measurement fail.
- Thresholds from ADR-005 stay (L1 merge above 16 sections). B1 shows they are
  conservative; they may be relaxed after the qpdf-side numbers exist.

**Caveats (so this is not over-read):**
- The numbers are **renderer-only and macOS-only**. They exclude the qpdf
  commit, IPC and transport.
- The test pages are synthetic and cheap to draw, and the section filler is
  unreferenced, so the matrix mostly shows xref-chain cost.
- The 1,000-page image-heavy file has not been run through the bench.
- Windows and Linux are unmeasured. CI jobs for B1 on both are follow-ups.

**Consequences:** every edit exercises the incremental writer; consistency
between qpdf and PDFium is by construction (same bytes), not by mirrored ops.

## ADR-030: ADR-006 confirmed by Spike 0.4 coverage; boilerplate fingerprint; execute per script
**Status:** Accepted. Confirms and amends ADR-006.

**Context:** Spike 0.4 ran the recognizer over 1,108 downloaded PDFs from 20
sources; 415 contain JavaScript (391 real-world, 24 from pdf.js), 21,589
non-empty scripts. Details: [spikes/0.4-form-scripts.md](spikes/0.4-form-scripts.md).

| Measure | ADR-006 sample (136 forms) | Spike 0.4, real-world (391 forms) |
|---|---|---|
| Scripts that are pure allowlisted AF calls | 57% | **72%** (15,518 / 21,589 over all 415) |
| Forms covered, subset + boilerplate | 93% (125 / 134) | **76%** (298 / 391) |
| Forms covered, excluding IRS | 61% (14 / 23) | **62%** (151 / 244) |
| IRS-type forms (IRS, NY, CRA, USCIS, DOL, FDA) | 100% | **99.6%** (249 / 250) |

By source: IRS 147/147, NY 35/35, USCIS 56/57, OPM 19/38, US states 54/88,
gov.uk 6/27, Home Affairs 0/8, CMS 0/5. Without the boilerplate recognizer
IRS coverage is 0%. Format and keystroke scripts are covered at 99.6% and
96.5%. The headline fell from 93% to 76% only because the sample is no longer
80% IRS; the like-for-like figure is unchanged (61% to 62%). The corpus skews
to US government forms, so commercial and non-US forms are likely lower.

**Decision (v0.1 plan confirmed):**
1. v0.1 ships the native AF subset plus the boilerplate recognizer and no JS
   engine, as ADR-006 decided. The published coverage figures are **99.6%
   for IRS-type forms and 62% for everything else**.
2. **Boilerplate fingerprint definition** (amends ADR-006): strings and
   numbers are normalised, runs of `ADBE.<name> = "<string>";` collapse,
   FNV-1a over the token stream, **exact match on the whole script**. There are
   nine known shapes (eight Adobe version/XFA-check shapes plus the inert
   California `onOpen()` that sets `nocache`/`noautocomplete`). New shapes are
   added by reading the script, never by loosening the match.
3. **Execute per script, banner per form:** every accepted script runs;
   rejected ones stay inert and the banner lists what did not run. Scripts are
   not disabled for the whole form when one is unsupported. 81% of forms have
   at least 90% of their scripts covered, and 90% have at least 50%.
4. **Calculation semantics:** each calculate script runs once per change in
   `/CO` order; fields outside `/CO` are not recalculated; a throwing script
   leaves its field unchanged. The host decides when to trigger.
5. **Before committing to QuickJS (v0.4/v0.5), add the exact-shape pattern
   tier** (checkbox exclusion and six similar shapes): it lifts real-world
   coverage to 82% (72% excluding IRS) for a few hundred lines, so the engine
   is needed for about 18% of forms rather than 24%. This is an upper-bound
   estimate from regexes, not an implementation. Re-measure first. The
   field-access and regex/string-method blockers define the component's first
   targets.
6. The `form-scripts` component (QuickJS, v0.5) and fallback (a) stay as in
   ADR-006.

**Consequences:** `papyrine-forms` has no runtime dependencies; the tokenizer,
recognizer, matcher and classifier add about 50 KB to the binary (about 150 KB
for the whole crate, over ADR-006's estimate of < 100 KB for the parser alone;
accepted). "Covered" means the recognizer accepts every script; it says
nothing about matching Acrobat (ADR-031).

## ADR-031: AF behaviour deviations from PDFium
**Status:** Accepted; the unverified rows are re-checked against Acrobat
before 1.0.

**Context:** the AF reference suite has 501 cases (280 from PDFium's BSD tests,
89 derived from PDFium source, 132 derived by hand from Adobe's API reference;
**none captured from a running Acrobat**, which was not available). ROADMAP 1.13
required at least 200. In seven cases PDFium and Papyrine differ on purpose.

**Decision:** Papyrine deliberately differs from PDFium in these cases:

| Case | PDFium | Papyrine |
|---|---|---|
| `AFNumber_Format` sepStyle 4 | falls back to style 0 | `1'234'567.89` (Acrobat documents style 4) |
| `AFNumber_Format` negStyle 1 | no sign, red | `-1,234.50`, red |
| `AFDate_FormatEx` on non-date `x` | prints today | keep text, invalid-date alert |
| `AFDate_FormatEx` on `20122015` | falls back to today | keep text, alert |
| `02/29/2023` | rolls over to 03/01/2023 | rejected, alert (**unverified**) |
| 12-hour clock, hour 0 | `0:05 am` | `12:05 am` |
| 12-hour clock, hour 12 | `12:00 am` | `12:00 pm` |

Also: the recognizer rejects extra trailing arguments to AF calls (Acrobat
ignores them; none occur in the corpus); `AFMergeChange`, `AFMakeNumber` and
`AFParseDateEx` are library functions only.

**Consequences:** the owner's check of `corpus/cache/generated/acrobat-check.pdf`
(made by `spikes/forms/make_acrobat_check_form.py`) in Acrobat Reader settles
the unverified rows, also leap-day rejection, `m/d/yy` fed a four-digit year,
and half-even rounding of exact ties (`1234.5` with 0 decimals). Until then
the table above is the specification.

## ADR-032: Licence allowlist adds LicenseRef-AGG-2.3 (native manifest only)
**Status:** Accepted. Amends the allowlist in ADR-001 / ARCHITECTURE §12.

**Context:** PDFium bundles Anti-Grain Geometry 2.3, a permissive licence with
no SPDX identifier. The gate rejected it (found independently by the renderer
and CI engineers).

**Decision:**
- `LicenseRef-AGG-2.3` is allowed **in the native manifest
  (`third_party/native.toml`) only**, not for Rust or JS dependencies. The
  AGG notice is carried in `THIRD_PARTY_LICENSES.md` (ADR-014).
- The gate implements this as `ALLOWED_NATIVE_ONLY`.

**Consequences:** ARCHITECTURE §12 lists it. No other non-SPDX licence is
allowed without a new ADR.

## ADR-033: Native libraries: pinned tarballs, Release-profile builds, no pkg-config
**Status:** Accepted. Amends ARCHITECTURE §2/§12 ("vendored" sources).

**Decision:**
- Native sources are **not vendored into the repository**. `third_party/fetch`
  downloads pinned tarballs (qpdf 12.4.2, zlib 1.3.2, libjpeg-turbo 3.2.0) over
  https, verifies their SHA-256 against `third_party/native.toml`, and extracts
  to `third_party/cache/src/`. `build.rs` panics with a "run
  third_party/fetch" message if sources are missing or stale and never uses the
  network. PDFium comes as pinned prebuilt binaries via `tools/fetch-pdfium`
  (chromium/8076 = PDFium 156.0.8076.0, SHA-256 pinned per platform).
- Third-party C++ libraries are **always built with the CMake Release profile**,
  also in debug builds, because a debug qpdf makes tests too slow.
- Dependencies are resolved **without pkg-config** (it breaks on paths with
  spaces and could pick up system copies) and pinned through
  `CMAKE_PREFIX_PATH`; Homebrew prefixes are ignored.
- libjpeg-turbo is built with `WITH_SIMD=OFF` for now.

**Consequences:** CI needs `third_party/fetch` and `tools/fetch-pdfium` steps
before any cargo build of these crates. A cold build of zlib, libjpeg-turbo and
qpdf takes about 46 s on the M4. The qpdf library adds about 1.3 MB to a
release binary (grows with API use). Windows MSVC (`/EHsc`, SIMD off) and
Linux builds of `qpdf-sys` are verified by CI, not by hand.

## ADR-034: qpdf shim error contract
**Status:** Accepted. Detail for ADR-002.

**Decision:** the C++ shim catches every exception, including `catch (...)`,
at the boundary and returns `Err("<code>|<message>")`.
- `<code>` is the `qpdf_error_code_e` value for qpdf errors, **100/101/102**
  for non-qpdf exceptions, and **110/111** for shim type and range errors.
- `std::range_error`, `out_of_range` and `length_error` map to **`Damaged`**,
  since they come from out-of-range numbers read from a damaged file.
- Warnings are collected per document (`setSuppressWarnings(true)` plus
  `getWarnings()`), giving message, object id/generation and offset. Warnings
  from handles with no owning document go to qpdf's global logger, which the
  shim silences once.
- Passwords are truncated at an embedded NUL (qpdf takes `char const*`).
- Writer stream-data modes are `Uncompress`/`Preserve`/`Compress`;
  `DecodeLevel::None` is omitted because qpdf throws for it on filtered streams.

**Consequences:** the Rust side never sees a C++ exception. A lazy
`StreamDataProvider`, per-object keys and the AcroForm/outline/page-label
helpers are follow-ups (`stream_replace` copies for now).

## ADR-035: Renderer: pdfium-render as loader and raw bindings only; tile format and flags; warm-up
**Status:** Accepted. Amends ADR-009 and ARCHITECTURE §4.

**Decision:**
1. **`pdfium-render` 0.9.4 (default features off) is used only as the dynamic
   loader and FFI layer.** Its high-level wrappers hide the progressive-render
   pause callback and the custom file access that we need
   (`FPDF_LoadCustomDocument` over the shared mapping, `FPDF_RenderPageBitmap_Start`
   with a pause callback for cancellation).
2. **Tile transport is opaque RGBA composited on white** (so premultiplied
   equals straight alpha), 512 x 512 = 1 MiB per tile. Render flags are
   `FPDF_ANNOT | FPDF_REVERSE_BYTE_ORDER | FPDF_RENDER_LIMITEDIMAGECACHE`.
   Form widgets need the form-fill draw pass (`FPDF_FFLDraw`), a follow-up.
3. **A background render warm-up runs at startup (after first paint)** to
   absorb PDFium's one-time font initialisation (the first tile in a fresh
   process takes 21 to 26 ms, 123 ms on a cold file cache). It is a startup
   allowlist entry only after first paint.
4. Cancellation is cooperative and returns `Error::Cancelled` in about 1.2 ms.

**Consequences:** the 8-page cache is MRU and a failed re-open keeps the old
document. On Windows `FPDF_FILEACCESS` uses `c_ulong`, so documents over 4 GB
fail with `TooLarge` there until a wrapper is added.

## ADR-036: Launch measurement via LaunchServices; `drawn` and `presented` stages
**Status:** Accepted. Detail for ADR-010 launch gate.

**Decision:**
- Launch time is the wall clock from just before the launch call to the host
  receiving the UI's `first_tile_painted` event.
- The event has two stages: `drawn` (right after `drawImage`) and `presented`
  (two animation frames later). **The gate uses `presented`**; `drawn` is the
  fallback when no frame arrives (occluded window).
- On macOS the `.app` is launched through LaunchServices (`open -n`), like a
  Finder launch, and WebKit helpers are attributed with the responsibility API
  (CI falls back to "helpers new since launch").
- Linux runs are wrapped in `dbus-run-session` (without a session bus every
  launch took 25 s).
- "Cold" is approximated by the first launch after a build where
  `sudo purge` is unavailable; the reference-machine check is repeated each
  milestone.

**Consequences:** reference Mac measured median 351 ms, p95 383 ms (20 runs)
under load average 4.3 to 4.5; under load 8 to 13, 0.76 s. Timing runs must
not share the machine with other heavy jobs.

## ADR-037: Windows tile URL and CSP
**Status:** Accepted. Amends ADR-009.

**Decision:** on Windows WebView2 maps custom schemes to
`http://papyrine.localhost/...`; the client builds the tile URL per platform and
the CSP allows both `papyrine:` and `http://papyrine.localhost` (plus `ipc:` and
`http://ipc.localhost`).

**Consequences:** the tile protocol handler is one code path; only the URL
prefix differs.

## ADR-038: Frontend toolchain pin
**Status:** Accepted.

**Decision:** React 18.3, Vite 7, TypeScript 5.9 (strict), `@vitejs/plugin-react`
5, pnpm 10 via `packageManager`. Tauri features limited to `wry`,
`compression` and `common-controls-v6`; no plugins, devtools or tray in the
shell. Runtime JS dependencies stay at `react`, `react-dom` and
`@tauri-apps/api`. `bundle.licenseFile` is left unset (it makes `create-dmg`
fail on its EULA step).

**Consequences:** Vite 8 and TypeScript 7 exist and are deliberately not used
yet; moving is a dependency PR with the bundle gate as the check. Initial JS
measured 45.4 KB gzipped against the 200 KB budget.

## ADR-039: Content-stream round-trip rules; `papyrine-core` has no dependencies
**Status:** Accepted.

**Decision (content streams, `papyrine-content`):**
- The guarantee `parse(serialize(parse(x))) == parse(x)` holds with
  `SerializeOptions::lossless()` (shortest `f64` text). The default of 6
  decimals is exact only for inputs within that precision.
- Integral numbers always parse as `Int` (`4.`, `4.0`, `4` are identical);
  comments are dropped on parse.
- Unterminated inline images are dropped with an error. Operator tokens are
  printable ASCII only; other bytes become error tokens. Names keep raw high
  bytes.
- The inline-image `EI` heuristic looks 6 bytes ahead; a known-length image
  accepts at most one whitespace byte before `EI`. The serializer emits a NUL
  separator before `EI` when the last 8 bytes of image data contain "EI".
- The parser never panics: problems become `ParseError { offset, kind }`.

**Decision (`papyrine-core`):** it has **no dependencies**; errors are
hand-written and the startup trace is a small std-only recorder. `tracing`
integration lives in the host or a feature.

**Consequences:** the roundtrip fuzz target found five inline-image bugs, now
fixed (1.96 M executions clean). Parse throughput is 167 MB/s (lex 362,
serialize 151) on the M4; per-op `Vec` allocations are the obvious cost if it
ever needs to be faster.

## ADR-040: Corpus pinning: Wayback snapshots allowed; operational "malformed"
**Status:** Accepted. Detail for ADR-017.

**Decision:**
- Every corpus URL is pinned to a commit SHA or, for documents with no
  versioned home (IRS forms), a **Wayback Machine `id_` raw snapshot**; the
  SHA-256 is recorded in the manifest. CI must cache `corpus/cache/files`
  (keyed on the manifest hashes) and `corpus/cache/generated` (keyed on the
  `gen-corpus` source hash).
- "Malformed" is defined operationally as "qpdf reports a warning or error"
  until Papyrine's parser supplies conformance tags.
- Top-level `corpus/*.toml` manifests are exempt from the 256 KiB rule of
  `check-no-corpus`; the type and location rules still apply.
- Government "latest" links go stale: a republished form fails `corpus/fetch`
  on hash and the manifest is regenerated.

**Consequences:** 2,971 files / 179 MB in `manifest.toml` plus 415 JS forms.
Wayback is the weak link (106 of 3,386 entries failed with connection refused
during a cold fetch). Mirroring them needs public hosting, which is an owner
decision (open item).

## ADR-041: Gate conventions: native licence-file paths, startup allowlist, MiB
**Status:** Accepted. Detail for ADR-014/015.

**Decision:**
- Native manifest `license_files` resolve against
  `third_party/cache/src/<name>-<version>/`; `--require-fetched` makes missing
  files fatal.
- Startup allowlist semantics follow `papyrine-core`: only `subsystem.*` spans
  count, and entries may end in `.*`. The initial entries are provisional.
- Sizes are binary units (1 MB = 1,048,576 bytes).
- Rust licences use `cargo-deny`, and notices a purpose-built generator
  (`tools/gen-notices`), instead of `cargo-about`. The PDFium licence file's
  `//` comment markers are stripped so notices are byte-identical across
  platforms.

**Consequences:** `THIRD_PARTY_LICENSES.md` is 247 KB against the 256 KB
no-corpus limit; it needs a waiver in `tools/check-no-corpus.allow` or a split
before v0.2 dependencies land (open item, with the `site/package-lock.json`
waiver).
