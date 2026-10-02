# Papyrine

A lightweight, fast, offline-first, open-source desktop PDF editor for Windows, macOS and
Linux. It aims for full parity with professional PDF suites, and its flagship
feature is a best-in-class PDF compression and optimization engine.

> **Status:** early development. Step 0 (de-risking spikes) is done apart from two Wave 2 items (see the [Step 0 report](docs/STEP_0_REPORT.md)); v0.1 (the MVP) is in progress. There is no usable release yet.

- [Architecture](docs/ARCHITECTURE.md)
- [Roadmap](docs/ROADMAP.md)
- [Step 0 report](docs/STEP_0_REPORT.md)
- [Decisions (ADRs)](docs/DECISIONS.md)

## Principles

- **Works on real-world files:** damaged PDFs are repaired, not rejected.
- **Offline-first:** no telemetry unless you opt in, and no cloud dependency.
- **Safe by design:** parsing and rendering run in sandboxed processes, and
  PDF JavaScript is off by default.
- **Transparent compression:** it audits before it compresses, shows a
  before/after report and a visual diff, and never writes a larger file.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
