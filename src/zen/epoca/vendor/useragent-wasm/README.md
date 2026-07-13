# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly (web target).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Package: **@useragent-kit/wasm** (local build), `--target web`
- Provenance: local `wasm-pack build crates/host-wasm --target web` of
  useragent-kit branch `fix/wallet-substrate-derivation` (on top of 0.4.50).
  This branch fixes the `//wallet` account derivation to the substrate /
  subkey / polkadot-js scheme (entropy mini-secret + substrate hard junction),
  so personhood usernames (`Resources.Consumers`), registration, and
  statement-store signing use the SAME account as the reference personhood
  apps. The prior 0.4.50 derived `//wallet` from the BIP-39 seed, producing a
  different account and breaking cross-app username resolution. Re-vendor the
  official artifact once the derivation fix is released.
- Also includes (from 0.4.50): PR #1552 (remote_permission wire fix +
  `storeRemotePermissionDecision`), #1551 (deriveProductEntropy), #1548
  (on-chain→positional statement transcoder), and `onchainToSignedStatement`.
- License: AGPL-3.0 (see LICENSE in this directory)

To rebuild: in a useragent-kit checkout, `CC_wasm32_unknown_unknown=$(brew
--prefix llvm)/bin/clang wasm-pack build crates/host-wasm --target web
--out-name useragent_wasm`, then copy `useragent_wasm.js` +
`useragent_wasm_bg.wasm` here. (Node 22; Homebrew LLVM clang for the wasm
target.) Re-vendor the official npm artifact once the fix lands upstream.
