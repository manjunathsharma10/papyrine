# qpdf-load fuzz target

Fuzzes the whole read path of `papyrine-cos` / vendored qpdf: open (with recovery, with and
without a password), page walk, stream decode, object fingerprints, and a full rewrite.

Needs the nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
third_party/fetch                      # once: pinned qpdf/zlib/libjpeg-turbo sources

cd fuzz/qpdf-load        # all cargo-fuzz commands run from here
cargo +nightly fuzz run --fuzz-dir . load -- -max_total_time=600      # 10 min, the per-PR budget
cargo +nightly fuzz run --fuzz-dir . load -- -max_total_time=3600     # 1 h, the nightly budget
```

Seed it with small generated PDFs (never commit PDFs; `*.pdf` is gitignored):

```sh
mkdir -p corpus/load && cp /path/to/small/*.pdf corpus/load/    # corpus/ here is gitignored
```

Crashes land in `fuzz/qpdf-load/artifacts/load/`; reproduce with
`cargo +nightly fuzz run --fuzz-dir . load artifacts/load/<file>`. Build outputs are in `fuzz/qpdf-load/target/`.
