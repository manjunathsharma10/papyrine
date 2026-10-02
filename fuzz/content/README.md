# Content-stream fuzzing

Target `roundtrip`: lex + parse + serialize on arbitrary bytes. Asserts no panics,
in-bounds spans, `parse(serialize(parse(x))) == parse(x)` and a stable fixed point for
the default rounded number format.

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
cd fuzz/content
cargo +nightly fuzz run roundtrip -- -max_total_time=300      # 5 minutes
cargo +nightly fuzz run roundtrip -- -dict=dict.txt           # with the token dictionary
```

Corpus and artifacts land in `fuzz/content/corpus` and `fuzz/content/artifacts`
(gitignored). A crash artifact reproduces with
`cargo +nightly fuzz run roundtrip artifacts/roundtrip/<file>`.
