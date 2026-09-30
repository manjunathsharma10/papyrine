# Folio — Architecture Decision Records

Format: each ADR has a status (Proposed / Accepted / Superseded), context,
decision and consequences. New ADRs are appended; old ones are never
rewritten, only superseded.

---

## ADR-001: Product license MIT OR Apache-2.0; permissive-only dependencies
**Status:** Proposed · 2026-09-30

**Context:**
- The owner wants Folio to be open source and asked for the best license.
- The candidates were permissive (MIT/Apache) and AGPL. AGPL would unlock
  MuPDF and Ghostscript.

**Decision:**
- Dual-license **MIT OR Apache-2.0** (the Rust ecosystem convention).
- Allowed dependency licenses: MIT, Apache-2.0, BSD-2/3, ISC, Zlib, MPL-2.0,
  Unicode, FTL, IJG, BSL-1.0, CC0.
- Banned: GPL and AGPL at any level. LGPL is allowed only when loaded at
  runtime and optional (ADR-008).
- Named bans: MuPDF, Ghostscript, jbig2dec, dssim, and linking Poppler.
- Enforced by `cargo-deny` and a JS license check in CI.

**Consequences:**
- It is the widest-adoption license and includes Apache-2.0's patent grant.
  Businesses (a key user group) can embed and redistribute Folio.
- PDFium + qpdf cover everything MuPDF would. We give up MuPDF's
  structured-text and reflow helpers and write our own layout analysis
  (needed anyway for editing).
- JBIG2 *decoding* must come from PDFium (BSD) instead of jbig2dec.
- SSIM is implemented in-house instead of using dssim.

## ADR-002: qpdf is the primary object layer; lopdf is not used
**Status:** Proposed · 2026-09-30

**Context:** we need a tolerant parser, repair, every encryption revision, a
writer with object and xref streams, and linearization. The candidates were
qpdf (C++, Apache-2.0) and lopdf (Rust, MIT).

**Decision:** use qpdf via its C API, vendored and statically linked.

**Consequences:**
- It is the most battle-tested repair and writing path available under a
  permissive license, and it provides linearization, which lopdf lacks
  entirely.
- lopdf's repair and encryption coverage is materially weaker.
- Cost: C++ in the trust boundary, which is mitigated by the sandbox and
  fuzzing (ADR-004), plus a CMake build on every platform.
- qpdf has no incremental writer, so we build one (`folio-writer`).
  Everything above COS is our own Rust.

## ADR-003: Tauri 2 + Rust + React/TypeScript
**Status:** Accepted (owner choice) · 2026-09-30

Small binaries, a Rust engine in process with the host, and a native webview
per OS. A webview accessibility tree maps to UIA, NSAccessibility and AT-SPI.
Cost: webview differences across WebView2, WKWebView and WebKitGTK must be
tested in CI on all three.

## ADR-004: Broker + sandboxed engine + render-worker pool (multi-process)
**Status:** Proposed · 2026-09-30

**Context:**
- PDFium is not thread-safe; `pdfium-render`'s thread-safe mode serializes
  all calls globally.
- The spec requires parsing and rendering to happen in a sandboxed,
  lower-privilege process whose crash does not take down the UI.

**Decision:**
- The Tauri host is the broker.
- One engine process (qpdf, model, commands, writer, optimizer).
- N render worker processes, each with its own PDFium.
- `ipc-channel` for messages, shared memory for pixels, brokered file
  handles.

**Consequences:**
- Real parallel rendering, crash isolation and privilege separation.
- Costs:
  - IPC complexity;
  - per-process memory overhead (~30–60 MB each), which is counted inside
    the 500 MB budget;
  - a document is loaded once per render worker, which is cheap because the
    file mapping is shared and PDFium loads lazily.

## ADR-005: Render snapshots are original bytes + in-memory incremental delta
**Status:** Proposed · 2026-09-30

**Context:** render workers must show edits made in the engine's qpdf graph.

**Decision:**
- After each commit, the engine serializes the dirty objects as an
  incremental update into shared memory.
- Workers reopen `original ‖ delta` through a custom PDFium reader.

**Consequences:**
- It reuses the incremental writer, so every edit exercises the save path.
  This is also a continuous test of the writer.
- Cost: an O(xref) reopen per commit, debounced. To be measured in Phase 1,
  with the fallback described in ARCHITECTURE §12.

## ADR-006: PDFium prebuilt binaries without V8/XFA; form JS via QuickJS
**Status:** Proposed · 2026-09-30

**Context:** PDFium's V8 build is large (+~20 MB) and puts a full JS engine
with a large attack surface inside the renderer. The spec wants JS off by
default and tightly sandboxed.

**Decision:**
- Use the non-V8 `bblanchon/pdfium-binaries` build, pinned by hash.
- Form JavaScript runs in QuickJS in the engine sandbox, implementing only
  the Acrobat form API subset (Phase 5).
- Dynamic XFA is read-only fallback, per the spec's non-goals.

**Consequences:**
- A smaller, safer renderer.
- Cost: we implement the `AF*` function family and the event model
  ourselves.

## ADR-007: Text shaping and fonts in pure Rust (rustybuzz, ttf-parser, fontdb) instead of FreeType/HarfBuzz/fontconfig
**Status:** Proposed · 2026-09-30

HarfBuzz is ported to Rust as rustybuzz. ttf-parser and fontdb give parsing
and system font discovery on all OSes without fontconfig. This means less C
in the trust boundary and simpler cross-platform builds. PDFium keeps its own
internal FreeType for rendering.

## ADR-008: LGPL/GPL tools only as optional runtime integrations
**Status:** Proposed · 2026-09-30

- HEIC: OS codecs (ImageIO, WIC). libheif is `dlopen`ed on Linux only if the
  user has it installed.
- SANE: runtime only.
- LibreOffice (MPL) and veraPDF (used under MPL): subprocesses, detected at
  runtime, never bundled by default.
- GPL test tools (Poppler, EU DSS) run in CI only and are never shipped.

## ADR-009: Tile-based rendering with √2 zoom buckets and raw RGBA transport
**Status:** Proposed · 2026-09-30

- 512 px tiles; render at the bucket scale and let the GPU scale to the
  exact zoom.
- A `folio://` custom scheme returns raw RGBA, decoded off-main via
  `createImageBitmap`.

This avoids PNG encode/decode and base64 on the hot path, and snapshot ids in
URLs make cache invalidation exact. Cost: a large tile L1 cache in the
broker, which is bounded by bytes.
