# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly (web target).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Package: **@useragent-kit/wasm@0.4.50**, `--target web`
- Provenance: the official npm artifact `@useragent-kit/wasm@0.4.50`
  (`useragent_wasm_web.js` + `useragent_wasm_bg.wasm`, byte-for-byte). Includes
  PR #1552 (remote_permission single-permission wire fix +
  `storeRemotePermissionDecision` to resolve `NeedsPermissionPrompt`), #1551
  (deriveProductEntropy), #1548 (on-chain→positional statement transcoder), and
  the `onchainToSignedStatement` binding.
- License: AGPL-3.0 (see LICENSE in this directory)

To update: `npm pack @useragent-kit/wasm@<version>`, extract, and copy
`useragent_wasm_web.js` → `useragent_wasm.js` and `useragent_wasm_bg.wasm`
here; bump the version above.
