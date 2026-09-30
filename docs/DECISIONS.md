# Papyrine — Architecture Decision Records

Each ADR has a status (Proposed / Accepted / Superseded), context, decision
and consequences. Once an ADR is **Accepted**, it is never edited, only
superseded. Proposed ADRs may be revised during review; revision 2 applied the owner's
first review and revision 3 the second (2026-09-30).

| # | Title | Status |
|---|---|---|
| 001 | License MIT OR Apache-2.0; permissive shipped dependencies | Proposed |
| 002 | qpdf via C API + C++ shim is the only object model; lopdf rejected | Proposed |
| 003 | Tauri 2 + Rust + React/TypeScript | Accepted |
| 004 | Single multi-role executable; host + engine + **one** renderer | Proposed |
| 005 | Render snapshots + compaction, benchmark first; mirroring keeps qpdf as truth | Proposed (rev 3) |
| 006 | Form JavaScript: native AF subset now (measured coverage), component later | Proposed (rev 3) |
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
| 025 | Printing via native platform APIs | Proposed (timing: owner) |

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
**Status:** Proposed; **timing is the owner's decision** (ROADMAP 1.17).

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
