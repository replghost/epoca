# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly (web target).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Package: **@useragent-kit/wasm@0.4.51** (official npm artifact), web-target glue.
- Provenance: the published npm package's `useragent_wasm_web.js` (web target,
  renamed here to `useragent_wasm.js`) + `useragent_wasm_bg.wasm`, byte-for-byte.
  0.4.51 is built from useragent-kit `main` (commit `3bffe108`) and ships the
  **substrate `//wallet` derivation fix** (PR #1558, entropy mini-secret +
  substrate hard junction), so personhood usernames (`Resources.Consumers`),
  registration, and statement-store signing use the SAME account as the
  reference personhood apps (polkadot-app-ios/android). This supersedes the
  earlier local build of branch `fix/wallet-substrate-derivation` — same source
  commit, now the released artifact.
- Also includes (carried from 0.4.50): PR #1552 (remote_permission wire fix +
  `storeRemotePermissionDecision`), #1551 (deriveProductEntropy), #1548
  (on-chain→positional statement transcoder), and `onchainToSignedStatement`.
- License: AGPL-3.0 (see LICENSE in this directory)

To re-vendor a newer release: `npm pack @useragent-kit/wasm@<version>`, then
copy the tarball's `useragent_wasm_web.js` to `useragent_wasm.js` here and
`useragent_wasm_bg.wasm` as-is. epoca loads the web-target glue via
`ChromeUtils.importESModule` and instantiates with `glue.default({ module_or_path })`.
