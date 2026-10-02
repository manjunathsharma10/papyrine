# Step 0 report: de-risking spikes

Date: 2026-10-02 · Status: **Wave 1 done; Wave 2 (Spike 0.3, qpdf large-file
memory) pending.** Decisions: ADR-026 to ADR-041 in
[DECISIONS.md](DECISIONS.md). Per-spike detail: [spikes/](spikes/).

## Summary

| Spike | Result |
|---|---|
| 0.1 Tauri shell | **Passed, after two decisions.** All budgets pass on macOS and Windows. Linux idle memory broke the budget under PSS accounting (260 MB vs 175 MB); the owner changed the accounting to the app's own memory and dropped AppImage (ADR-026, ADR-027) |
| 0.2 qpdf shim + PDFium reader + B1 | **Passed (renderer half).** B1 beats every target; snapshot design confirmed, mirroring not adopted (ADR-029). qpdf shim and `papyrine-cos` built with 39 tests. The qpdf-side large-file memory run is pending |
| 0.3 qpdf vs lopdf | **Pending (Wave 2).** ADR-002 stands. The corpus tooling it needs is done |
| 0.4 Form scripts | **Passed.** v0.1 forms plan confirmed (ADR-030): 99.6% of IRS-type forms, 62% of other real-world JS forms |
| Corpus | **Done.** 2,971-file manifest, 415 JS forms, 80 generated files, no corpus committed |
| Content streams | **Done.** `papyrine-core` and `papyrine-content` built, fuzzed |
| CI and gates | **Done, partly red.** All gates live; main CI was red on three issues outside the gate code (see Open items) |

## Measured budgets (ARCHITECTURE §1.1)

Reference Mac = Apple Silicon, macOS 27. CI numbers are GitHub-hosted runners
(run 37003221829, commit ad641f9) and are not the reference machine.

| Budget | macOS arm64 (reference) | Windows x64 (CI) | Linux x64 (CI, Xvfb, no GPU) | Limit |
|---|---|---|---|---|
| Installer | dmg 2.01 MB (.app 5.06 MB) | msi 2.55 MB, NSIS 1.72 MB | deb 2.05 MB, rpm 2.05 MB; Flatpak not built | 50 MB each |
| Cold launch to first tile | median 351 ms, p95 383 ms (20 runs) | median 624 ms | median 389 ms | < 1.0 s (reference); CI < 2.0 s |
| Idle memory, bare shell | 77.4 MB (host 32.2, WebContent 26.5, GPU 15.7, Networking 5.8) | 67.8 MB | **59 to 111 MB own memory (gated)**; 185 to 260 MB PSS (informational) | 150 / 200 / 175 MB |
| Initial JS | 45.4 KB gzipped | same | same | 200 KB |

Linux detail (ADR-026):

| Configuration | PSS | Anonymous | File-backed shared |
|---|---|---|---|
| Default | 260 MB | about 111 MB (host 24.6, networking 7.5, web 78.7) | about 144 MB |
| `WEBKIT_DISABLE_DMABUF_RENDERER=1` | 185 MB | about 59 MB | about 125 MB |

AppImage (dropped, ADR-027): 261 MB in the CI bundle job (the shell spike build
measured 77 MB), and it bundles GnuTLS, nettle/hogweed, OpenSSL and libcups.

Other measurements:

| Item | Value |
|---|---|
| PDFium library | `libpdfium.dylib` 6.94 MB (3.49 MB compressed); about 0.5 MB footprint on load |
| qpdf added to a release binary | about 1.28 MB (thin LTO, strip); `libqpdf.a` 7.0 MB |
| qpdf cold build (zlib, libjpeg-turbo, qpdf, shim) | 46 s, M4, 4 jobs |
| B1 re-open, 2,000 pages (50.8 MB), k = 1..64 | 0.4 to 0.6 ms |
| B1 edit to tile p95 (500 edits), 2,000 pages | 7.7 to 11.9 ms (target < 100 ms) |
| B1 footprint growth over 500 edits | at most 1.0 MB (target < 10 MB) |
| First tile in a fresh process | 21 to 26 ms; 123 ms on a cold file cache |
| Cancel at first poll | about 1.2 ms |
| Content-stream parse / serialize / lex | 167 / 151 / 362 MB/s (10.49 MB stream, 496,716 ops) |
| `papyrine-forms` size | about 150 KB whole crate (about 50 KB recognizer path) |

## What passed and what failed

**Passed**
- Every macOS and Windows budget; Linux installer, launch and JS budgets.
- B1: all three targets with large margin (renderer-only, macOS).
- qpdf: all encryption revisions (R2 to R6) open with user and owner
  password; wrong, missing and empty passwords give `InvalidPassword`; 58
  malformations and 400 + 100 byte-mutation cases never panic or hang; the
  crypto gate shows native-only (`getRegisteredImpls() == ["native"]`, no
  GnuTLS or OpenSSL symbols). Fuzz: 119,903 executions in 30 s, no crash.
- Content streams: roundtrip fuzz found five inline-image bugs, all fixed;
  1.96 M clean executions.
- Forms: 32 tests, 501 AF reference cases pass, 6 robustness tests with more
  than 120,000 random scripts and no panic.
- Corpus: 3,280 of 3,386 manifest entries downloaded with 0 hash mismatches;
  all 80 generated files produced in about 15 s, deterministic for the 62
  non-encrypted files checked.
- Gates: `tools/gates-selftest` runs 59 fixtures (every gate, including the
  deliberate violations in ROADMAP 1.1) and all pass locally.

**Failed or found broken**
- Linux idle memory under PSS (resolved by decision, ADR-026).
- AppImage fails the licence/bundle gate and the size budget (resolved by
  dropping it, ADR-027).
- Windows tests failed in CI on `qpdf-sys` verbatim `\\?\` paths given to
  CMake; fixed in commit 2ea7ba9 (confirmation in CI pending).
- Main CI red on `site/package-lock.json` (289,928 bytes, over the 256 KiB
  limit of `check-no-corpus`).
- 106 Wayback entries failed with connection refused during a cold fetch
  (6 IRS entries of the main manifest, 100 of `js-forms.toml`).
- 9 of 58 malformations are accepted by `qpdf --check` (exit 0); Papyrine's
  parser and PDFium may disagree with qpdf on them.

## Deviations from the documents

- Native sources are fetched as pinned tarballs, not vendored (ADR-033).
- `pdfium-render` is used as loader and raw bindings only (ADR-035).
- Form widgets are not drawn by the renderer yet (`FPDF_FFLDraw` follow-up).
- The lazy `StreamDataProvider`, per-object keys, and a `memmap2` dependency
  are not in `papyrine-cos` yet; `QPDFLogger` is replaced by per-document
  warnings (ADR-034).
- `papyrine-core` uses a std-only startup recorder and hand-written errors, not
  `tracing`/`thiserror` (ADR-039).
- `papyrine-forms` is about 150 KB, over ADR-006's under-100 KB estimate for
  the parser alone (accepted, ADR-030).
- The AF reference is built from PDFium tests and Adobe documentation, not
  captured from Acrobat; ROADMAP 1.13 wanted Acrobat-captured outputs or public
  tables (public tables were used).
- Frontend: Vite 7 and TypeScript 5.9, not the current Vite 8 and TypeScript 7
  (ADR-038).
- Rust licences use `cargo-deny` and an in-house notices generator, not
  `cargo-about` (ADR-041); `wildcards = "warn"`, `unmaintained = "workspace"`.
- The "cold" launch figure is not after `sudo purge` (needs a password); the
  first launch after a build is used as a lower bound.
- `Cargo.lock` was left out of several engineers' commits and `--locked` was
  dropped from CI until the lockfile is stable.

## Open items

**Pending work (Wave 2)**
- Spike 0.3: qpdf vs lopdf on the corpus (success, peak RSS, time to first
  object). Confirms or reopens ADR-002.
- qpdf-side large-document memory run on the four large files (ARCHITECTURE
  §1.2); engine and renderer must each stay at or below 120 MB settled.

**Needs the owner's hardware or decision**
- A real-GPU Linux desktop run (X11 and Wayland) to confirm the DMABUF flag
  does not hurt rendering and that the own-memory figure holds (ADR-026).
- Acrobat Reader check of the 7 AF deviations: open
  `corpus/cache/generated/acrobat-check.pdf` (made by
  `python3 spikes/forms/make_acrobat_check_form.py`) and compare each field
  with the printed columns; also leap-day rejection, `m/d/yy` with a
  four-digit year, and rounding of exact ties (ADR-031).
- Agencies blocked from download here: VA, SSA, DoD, ATF, NRC, Utah (403) and
  IRCC. They need CI or the owner's machine; Canada is only 7 CRA forms so far.
- Whether to mirror the Wayback-hosted corpus entries publicly (needs public
  hosting) or retry once archive.org recovers (ADR-040).
- Enable GitHub private vulnerability reporting (`SECURITY.md` relies on it)
  and triage the Dependabot PRs.
- A real Flatpak build (flatpak-builder manifest) to measure its size.
- Reference-machine cold launch after `sudo purge`, with the stray `pkill -x
  papyrine` loop stopped and the machine idle.

**Engineering follow-ups**
- Waive or split: `site/package-lock.json` (289,928 bytes) and
  `THIRD_PARTY_LICENSES.md` (247 KB) against the 256 KiB `check-no-corpus`
  limit; decide between an allowlist entry per path or a lockfile rule.
- CI: add `third_party/fetch` and `tools/fetch-pdfium` steps, qpdf-sys builds
  on Linux and Windows (MSVC `/EHsc`, SIMD off), B1 on Windows and Linux,
  corpus caches, the fuzz jobs, MSI inspection, and the memory producers for
  the launch, idle-memory and large-document JSON inputs of `check-budgets`
  (Linux must emit `idle_anon_mb`; `measure-shell.mjs` writes
  `memoryGate` for this). Re-add `--locked` once `Cargo.lock` is stable.
- Run the 1,000-page image-heavy file through B1; add `FPDF_FFLDraw`.
- Wire `papyrine-forms` into the engine (`FormValues`, a clock for `DateEnv`,
  UTF-16 selection offsets) and add its fuzz target.
- Add the exact-shape pattern tier before the QuickJS decision (ADR-030).
- Wire `StartupTrace::violations` into the budget gate with the real
  allowlist (the current entries are a guess).
- Untested platforms: Windows arm64, macOS x64 and Linux arm64 for the shell;
  Windows and Linux for B1, qpdf runtime and the corpus tools.
