# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly (web target).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Package: **@useragent-kit/wasm@0.4.49** (official npm release), `--target web`
- Provenance: origin/main @ 443119156e1b  (includes deriveProductEntropy +
  encodeDeriveEntropy bindings [#1551], on-chain→positional statement
  transcoder [#1548], and NeedsResourceAllocation/NeedsRemotePermission).
- License: AGPL-3.0 (see LICENSE in this directory)

To update: `npm pack @useragent-kit/wasm@<version>`, extract, and copy
`useragent_wasm_web.js` → `useragent_wasm.js` and `useragent_wasm_bg.wasm`
here; bump the version above.
