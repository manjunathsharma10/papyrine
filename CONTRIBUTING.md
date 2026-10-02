# Contributing to Papyrine

Papyrine is dual-licensed under MIT OR Apache-2.0. By contributing you agree that your
contribution is licensed the same way. Read `docs/ARCHITECTURE.md` (especially sections 1.1, 10 and
12) and `docs/DECISIONS.md` before larger changes.

## Prerequisites

| Tool | Version | Notes |
|---|---|---|
| Rust | the version pinned in `rust-toolchain.toml` | `rustup` installs it automatically; includes `rustfmt` and `clippy` |
| Node.js + pnpm | Node 24, pnpm 10 | `corepack enable` then `pnpm install` at the repo root |
| CMake and a C++17 compiler | CMake 3.20+ | Xcode command line tools (macOS), Visual Studio Build Tools with the C++ workload (Windows), `build-essential` (Linux). Ninja is recommended |
| Python | 3.11+ | runs the fetch scripts and `tools/check-no-corpus` |
| Linux only | `libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev patchelf` | Tauri system libraries |

Optional, for the gates: `cargo install cargo-deny --locked`.
`qpdf` and Poppler may be installed as **test-time oracles only**. They are never linked or shipped.

## First build

```sh
git clone https://github.com/manjunathsharma10/papyrine && cd papyrine
python3 third_party/fetch                       # pinned native sources -> third_party/cache/ (SHA-256 verified)
python3 tools/fetch-pdfium --platform mac-arm64  # or mac-x64, linux-x64, win-x64, win-arm64
pnpm install
cargo build --workspace
cargo test --workspace
```

Test PDFs are never committed. Synthetic files are generated at test time by `tools/gen-corpus`;
third-party files are downloaded with `python3 corpus/fetch` into the gitignored `corpus/cache/`.

## Before you push

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
pnpm --filter papyrine-desktop typecheck
```

Then run the gates that CI runs (all are quick):

```sh
cargo deny check licenses bans sources                 # shipped Rust licences and bans
cargo run -q -p license-gate -- all --require-fetched  # shipped JS, native manifest, qpdf crypto
cargo run -q -p gen-notices -- --check                 # THIRD_PARTY_LICENSES.md is up to date
python3 tools/check-no-corpus                          # no PDFs, keys or blobs > 256 KB tracked
cargo run -q -p gates-selftest -- --require-all        # each gate rejects deliberate violations
```

### Dependencies and notices

- Shipped dependencies must be permissive: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0,
  Unicode-3.0, BSL-1.0, CC0, IJG, libpng, FTL; OFL-1.1 for fonts only. GPL, AGPL and LGPL are
  banned, as are MuPDF, Ghostscript, jbig2dec, dssim, Poppler, a bundled libheif, GnuTLS and
  OpenSSL (see section 12).
- Adding or updating a dependency changes `THIRD_PARTY_LICENSES.md`. Regenerate it with
  `cargo run -p gen-notices` (run `pnpm install` and `python3 third_party/fetch` first) and commit it.
- A vendored native library needs a `[[library]]` entry in `third_party/native.toml` with version,
  SPDX licence, licence files, URL and SHA-256 (see the schema in `tools/license-gate/src/native.rs`).

### Budgets

Installer size, launch time, idle memory, the initial JS bundle and the startup trace are gates
(ARCHITECTURE section 1.1). A pull request that grows an installer by more than 5% needs a
maintainer to add the `size-increase-approved` label. Work that must run before first paint needs an
entry in `tools/check-budgets/startup-allowlist.txt` and a reason in the pull request.

## Commits and pull requests

- Conventional commit messages (`feat(render): ...`, `fix(cos): ...`, `build: ...`, `docs: ...`).
- Keep pull requests focused; new behaviour comes with tests.
- Never commit PDFs, FDF/XFDF, certificates or keys, or files larger than 256 KB.
- Security issues: see `SECURITY.md`; do not file them as public issues.
