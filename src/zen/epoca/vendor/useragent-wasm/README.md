# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly (web target).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Package: **@useragent-kit/wasm@0.4.50**, `--target web`
- Provenance: built from useragent-kit `main` @ 61f41cc07 (PR #1552:
  remote_permission single-permission wire fix + `storeRemotePermissionDecision`
  binding to resolve `NeedsPermissionPrompt`), on top of #1551 (deriveProductEntropy)
  and #1548 (on-chain→positional statement transcoder). Workspace version 0.4.50.
  NOTE: this is a local `wasm-pack build --target web` of the merged 0.4.50
  source — the official npm `@useragent-kit/wasm@0.4.50` publish is pending a
  fix to useragent-kit's tag/Swift-dist release step (unrelated to the wasm).
  Re-vendor the official artifact once 0.4.50 is on npm (should be byte-equivalent).
- License: AGPL-3.0 (see LICENSE in this directory)

To update: `npm pack @useragent-kit/wasm@<version>`, extract, and copy
`useragent_wasm_web.js` → `useragent_wasm.js` and `useragent_wasm_bg.wasm`
here; bump the version above.
