# Papyrine test corpus

Nothing in this directory is a test file. Per ADR-017 the repository holds only
manifests, the fetch script and this README. Files are downloaded into
`corpus/cache/` (gitignored) and synthetic files are generated into
`corpus/cache/generated/` by `tools/gen-corpus`.

## Layout

| Path | Tracked | Purpose |
|---|---|---|
| `corpus/manifest.toml` | yes | Third-party files (pdf.js, qpdf, PDFium, veraPDF, ...) |
| `corpus/js-forms.toml` | yes | Form-script corpus (same schema) |
| `corpus/*.toml` | yes | Any further manifest; `fetch` reads every one |
| `corpus/fetch` | yes | Downloader (python3 stdlib) |
| `corpus/cache/files/<id>` | no | Downloaded files |
| `corpus/cache/generated/` | no | Output of `tools/gen-corpus` |

## Manifest schema

Every `corpus/*.toml` uses the same schema, one `[[file]]` table per file:

```toml
[[file]]
id      = "pdfjs/test/pdfs/alphatrans.pdf"   # unique; relative path, cache location
url     = "https://raw.githubusercontent.com/mozilla/pdf.js/<commit>/test/pdfs/alphatrans.pdf"
sha256  = "<64 hex chars>"
license = "Apache-2.0"                        # SPDX id (or short text) as declared by the source
source  = "mozilla/pdf.js@<commit>"          # repository/collection and pin
tags    = ["pdfjs", "forms", "malformed"]    # free-form, lowercase
size    = 12345                                # optional, bytes (used for progress and --max-size)
note    = "..."                                # optional free text
```

Rules:

* `id` is a relative path with `/` separators, no `..`, no leading `/`, and is
  unique across all manifests. The file lands at `corpus/cache/files/<id>`.
* URLs should be pinned to a commit or tag, or to a Wayback Machine `id_` raw
  snapshot, so that hashes stay stable. A hash mismatch is a hard failure.
* `license` records what the source repository or collection declares. The
  corpus is only downloaded for testing and never redistributed (ADR-017);
  per-file provenance inside large third-party suites is not audited.
* Tags used by tooling: `malformed` (qpdf reports errors or warnings),
  `unreadable` (qpdf cannot open it), `encrypted`, `forms` (has /AcroForm),
  `xfa`, `js` (contains JavaScript), `signed`, `pdfa`, `pdfua`, `large` (> 1 MB).
  Source tags: `pdfjs`, `qpdf`, `pdfium`, `pdfbox`, `pypdf`, `pdfminer`,
  `pdfplumber`, `pikepdf`, `lopdf`, `pdfdiff`, `verapdf`, `isartor`, `irs`.

## Fetching

```
corpus/fetch                    # everything
corpus/fetch --tag malformed    # only files carrying the tag (repeat for OR)
corpus/fetch --exclude-tag large
corpus/fetch --jobs 16 --max-size 5000000
corpus/fetch --verify           # hash what is already cached, no network
corpus/fetch --list             # print selected ids
```

Downloads are parallel and resumable (already-present files with a matching
hash are skipped; partial downloads use a `.part` file and HTTP Range). Exit
status is 1 on any hash mismatch or download failure, 0 otherwise.

## Synthetic files

`cargo run -p gen-corpus --release -- --help`. Output goes to
`corpus/cache/generated/`. Encryption variants need the `qpdf` CLI at
generation time (a test-time tool only, never linked into shipped code).

## Regenerating the manifest

`tools/gen-corpus/scripts/build_manifest.py` (needs `gh`, `qpdf` and network)
re-enumerates the pinned upstream trees, downloads, hashes and classifies the
files and rewrites `corpus/manifest.toml`.

## Adding a file

Add a `[[file]]` block with a pinned URL and the real SHA-256
(`curl -sL URL | shasum -a 256`), run `corpus/fetch`, and run
`tools/check-no-corpus` before committing.
