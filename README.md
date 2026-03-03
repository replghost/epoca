# Epoca

A programmable, privacy-first browser built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) and [PolkaVM](https://github.com/paritytech/polkavm).

> **Early development.** Expect rough edges.

## What is Epoca?

Epoca is a browser workbench for power users who want to own their browsing experience:

- **Programmable tabs** — write small, auditable tab-replacement apps in ZML (a declarative UI language) that run in a PolkaVM sandbox. A custom Reddit reader, a stripped-down Gmail, a focused Notion view — all as first-class browser tabs with no extension marketplace to trust.
- **Privacy by default** — content blocking runs before the network stack via WKContentRuleList, not in a JavaScript extension that pages can detect and disable.
- **Native performance** — GPUI on macOS/Linux/Windows, wgpu on Android. No Electron.

## Building from source

### Prerequisites

- [Rust](https://rustup.rs) (stable, latest)
- macOS 13+ (Ventura or later) for the macOS build
- Xcode Command Line Tools: `xcode-select --install`

### Build

```bash
git clone https://github.com/replghost/epoca.git
cd epoca
cargo build -p epoca --release
```

The binary is at `target/release/epoca`. Run it directly:

```bash
./target/release/epoca
```

### Running the sample ZML apps

The `examples/` directory contains pre-built guest app binaries:

```bash
# The browser will load ZML apps from the sidebar
ls examples/
```

### Building a guest app

Guest apps are written in Rust and compiled to PolkaVM:

```bash
cd guest/counter
cargo build --release
```

## Architecture

```
crates/
  epoca/           — binary entry point
  epoca-core/      — workbench shell, tab types, GPUI views
  epoca-sandbox/   — PolkaVM runtime (guest app execution)
  epoca-protocol/  — ViewTree serialization (host ↔ guest)
  epoca-broker/    — capability/permission broker
  epoca-dsl/       — ZML parser and evaluator
  epoca-shield/    — content blocking (in progress)
  epoca-android/   — Android renderer (wgpu + Vulkan/GL ES)
guest/
  counter/         — sample PolkaVM guest app
examples/          — pre-built .polkavm binaries + sample manifests
```

## Status

Epoca is pre-alpha. The core browsing shell works on macOS. Many features listed in [`docs/backlog.md`](docs/backlog.md) are in progress.

Pre-built binaries are not distributed yet — please build from source. macOS code signing and notarization are planned before any binary releases.

## License

[AGPL-3.0](LICENSE)
