# Spike: test corpus (ROADMAP 1.2, ADR-017)

Status: built and run on macOS 27 / Apple M4 (10 cores) on 2026-10-02.
Windows and Linux were not run here; `corpus/fetch` and `tools/check-no-corpus`
are standard-library Python and `gen-corpus` is portable Rust, so CI on all
three platforms is the place to confirm.

## What exists

| Piece | Path |
|---|---|
| Manifest schema and usage | `corpus/README.md` |
| Third-party manifest | `corpus/manifest.toml` (2,971 files) |
| Downloader | `corpus/fetch` (python3 stdlib, reads every `corpus/*.toml`) |
| Synthetic generator | `tools/gen-corpus` (Rust, one dependency: `miniz_oxide`) |
| Manifest builder (maintainer) | `tools/gen-corpus/scripts/build_manifest.py` |
| Tracked-file gate | `tools/check-no-corpus`, `tools/check-no-corpus.allow` (empty) |

## Third-party corpus (`corpus/manifest.toml`)

2,971 files, 179.0 MB (all hashes computed from real downloads). Every URL is
pinned to a commit SHA (GitHub raw) or a Wayback Machine raw snapshot (`id_`).

| Source | Files | License recorded |
|---|---:|---|
| mozilla/pdf.js `test/pdfs` (files <= 3 MB; `.link` files skipped) | 974 | Apache-2.0 |
| qpdf `qtest` and examples | 674 | Apache-2.0 |
| veraPDF corpus: all Isartor (200), TWG, ISO 32000 samples, every 14th of the rest | 464 | CC-BY-4.0 |
| PDFium `testing/resources` (chromium/pdfium mirror) | 297 | BSD-3-Clause |
| Apache PDFBox test resources | 167 | Apache-2.0 |
| IRS forms (Wayback snapshot of irs.gov, 96 forms) | 96 | public domain (US gov) |
| pdfplumber, pypdf, pikepdf, pdfminer.six, pdf-differences, lopdf | 79, 67, 60, 54, 35, 4 | MIT, BSD-3, MPL-2.0, MIT, Apache-2.0, MIT |

Tags are measured, not guessed: `qpdf --check` (exit 3 -> `malformed`, exit 2
-> `malformed` + `unreadable`), `qpdf --is-encrypted`/`--requires-password`
(`encrypted`, `password`), and a byte scan of the qpdf-normalised file for
`/AcroForm`, `/XFA`, `/JavaScript`/`/JS`, `/ByteRange`.

| Tag | Files | Tag | Files |
|---|---:|---|---:|
| malformed (qpdf warnings or errors) | 805 | js (JavaScript present) | 194 |
| unreadable (qpdf cannot open or repair) | 116 | js and forms | 172 |
| forms (AcroForm) | 500 | encrypted | 121 |
| xfa | 131 | password-protected | 67 |
| signed (ByteRange) | 114 | pdfa (veraPDF PDF/A + Isartor) | 346 |
| irs | 96 | pdfua | 30 |
| large (> 1 MB) | 38 (57.6 MB) | isartor | 200 |

The requirement of >= 50 damaged files is met 16 times over (805 with qpdf
diagnostics, 116 that qpdf cannot open at all). With the 415 files in
`corpus/js-forms.toml` (written by the form-script work) the JavaScript form
target of 150 is exceeded; `corpus/fetch` loads both manifests with the same
schema.

Licensing notes. The recorded license is what the source repository or
collection declares. Individual files inside large third-party suites (pdf.js,
qpdf, PDFBox, pdfplumber samples) can have their own provenance and were not
audited one by one; none are redistributed (ADR-017), they are only downloaded
for testing. The veraPDF corpus is CC-BY-4.0 (attribution: veraPDF Consortium
and the Isartor and BFO suites it incorporates). Nothing from MuPDF,
Ghostscript or Poppler test trees was used.

### Fetch timings (`corpus/fetch -j 16`, cold cache)

* Manifest + js-forms (3,386 entries, ~265 MB): 443 s, dominated by
  Wayback Machine, which refuses connections under load (106 of 3,386
  entries failed with connection refused: 6 IRS entries of this manifest, 100
  from `js-forms.toml`, which is also Wayback-hosted). All 2,875 GitHub-hosted
  entries of the first run downloaded in 78 s with zero failures and zero hash
  mismatches.
* `--verify` (hash all cached files, no network): 0.2 s for 158 MB.
* `fetch` caps `web.archive.org` at 2 concurrent requests with a delay and
  retries with backoff; on a cache miss CI should cache `corpus/cache/files`
  keyed by the manifest hash so Wayback is hit once. Wayback is the weakest
  link; if it stays flaky, mirror the IRS set to a release asset (needs an
  owner decision, public hosting).

## Synthetic corpus (`tools/gen-corpus`)

`cargo run -p gen-corpus --release -- --help`. Flags: `--list`, `--only
PATTERN[,..]` (globs and group names), `--skip-huge`, `--force`, `--jobs N`,
`--out DIR`, `--check`. Output: `corpus/cache/generated/` plus `INDEX.tsv`
(name, group, bytes, description) and `passwords.tsv`. Files are written to a
`.tmp` and renamed, existing files are kept unless `--force`.

Measured on the M4, all 80 files in one run: **39 s including `qpdf --check`
of everything; generation alone about 15 s** (largest: 1000p-images 7 s).

| File | Size | Gen time | Notes |
|---|---:|---:|---|
| typical-20p | 4.1 MB | 0.1 s | 20 pages, text + 1100x800 RGB image per page |
| large-2000p-text | 40.5 MB | 3.7 s | 2,000 pages, dense text, number tables, vector charts |
| large-1000p-images | 444 MB | 7 s | 1,000 pages, 2480x3508 RGB (300 dpi A4), Flate |
| large-single-page | 10.5 MB | 1.8 s | 14173 x 14173 pt (5.00 m square), 400k vector elements |
| large-objects | 127 MB | 0.5 s | /Size 1,601,701, classic xref |
| large-objects-objstm | 25 MB | 1.9 s | same object count in object streams + xref stream |
| enc-* (15) | 3-4 KB | ~0.01 s | see below |
| mal-* (58) | < 20 KB, bomb 1 MB | ~0.01 s | see below |

`--skip-huge` skips `large-1000p-images` and the classic `large-objects`.
Determinism: all non-encrypted files were generated twice with different
`--jobs` and compared by SHA-256: 62 of 62 identical.

`qpdf --check` on every generated file: typical, large-* and plain source all
exit 0 (clean). Encryption variants all exit 0 when opened with the
documented password (`passwords.tsv`).

### Encryption variants

Made by the `qpdf` CLI at generation time only (documented, never linked).
RC4-40, RC4-128, AES-128, AES-256 R5 and R6 each with a user password
("user"/"owner"), and again with an empty user password plus owner
restrictions (print, modify, extract denied); AES-128 and AES-256 R6 with
cleartext metadata; a non-ASCII AES-256 R6 password; AES-256 R6 with object
streams; RC4-128 with identical passwords. qpdf draws random salts and file
keys, so these are not byte-reproducible; parameters and passwords are fixed
(`--static-id --static-aes-iv` are used for what can be fixed).

### Malformations (58 kinds, 6 groups)

Each starts from a small valid base built by the crate and applies one defect;
edits preserve length where possible so the xref stays valid and the defect is
isolated.

* xref/trailer: shifted, zeroed or single wrong offsets; startxref beyond EOF,
  not-an-xref or missing; xref or trailer missing; no /Root; wrong subsection
  count; 19-byte entries.
* objects/streams: missing endobj or endstream; /Length short, long, negative,
  self-referencing or missing; NUL bytes in dictionaries; 50,000-deep array;
  integer overflow; bad name escapes; duplicate object definitions.
* file level: truncation at five points; 1,500 bytes of garbage before the
  header; 10 KB after EOF; missing header; bad version; fake linearization;
  bogus /Encrypt.
* page tree: circular /Kids, huge /Count, kid pointing to a missing object,
  Parent loop, /Root that is a page, missing /Type.
* filters/content: Flate corruption, bad Adler-32, truncated data, unknown
  filter, unbalanced string, inline image without EI, 1 GiB decompression bomb
  in 1 MB.
* object/xref streams and updates: wrong /N, /First, corrupt data, missing
  /Type, bad /W, short /Size, corrupt xref stream, off startxref, /Prev loop,
  /Prev into an object, plus a valid incremental update as a control.

qpdf's verdicts (`INDEX.tsv` has the descriptions; exit codes measured):
34 exit 3 (warnings, repaired), 15 exit 2 (unrecoverable or structurally
invalid), 9 exit 0. The exit-0 set is the interesting part for Papyrine:
qpdf silently tolerates short xref entries, a bad version, a missing kid
object, a Parent loop, NUL bytes, a corrupted or checksum-failing Flate
stream, and the decompression bomb (`--check` reports no problem). Papyrine's own parser and PDFium may
disagree with qpdf on exactly these.

## `tools/check-no-corpus`

Checks `git ls-files`: forbidden extensions (`.pdf .fdf .xfdf .p12 .pfx .key
.pem`, any case), content starting with `%PDF-` or `%FDF-`, blobs > 256 KiB
(except top-level `corpus/*.toml` manifests: text, 1.2 MB for `manifest.toml`), and anything under `corpus/` other than top-level `*.toml`, `fetch` and
`README.md`. Types and size can be waived by globs in
`tools/check-no-corpus.allow` (empty); the `corpus/` rule cannot.
`--self-test` builds a temporary git repository and asserts clean, violating,
waived and un-waivable cases (4 scenarios, passing).

**Finding on the current tree:** the check fails on `site/package-lock.json`
(289,928 bytes > 262,144). That file belongs to the site session. Options:
add `site/package-lock.json` to the allowlist (a reviewed exception for a
lockfile) or exclude `package-lock.json`/`pnpm-lock.yaml`/`Cargo.lock` by
rule. The allowlist was left empty as specified.

## Proposed ADRs

* ADR-017 amendment: Wayback Machine `id_` snapshots are acceptable pins for
  documents that have no versioned home (IRS forms); CI must cache them.
* Corpus `malformed` is defined operationally as "qpdf reports a warning or
  error"; Papyrine's own conformance tags will supersede it once the parser
  exists.

## Follow-ups

* CI: cache `corpus/cache/files` keyed on the manifest hashes and
  `corpus/cache/generated` keyed on the gen-corpus source hash; add a job that
  runs `tools/check-no-corpus` and `--self-test`; run `corpus/fetch` once on
  Windows and Linux runners.
* The huge files need disk (about 600 MB generated) and a size-gated CI job.
* Retry the failed Wayback entries when archive.org recovers, or mirror them.
* pdf.js `.link` entries (large files hosted elsewhere) were skipped; adding
  them needs per-file license review.
