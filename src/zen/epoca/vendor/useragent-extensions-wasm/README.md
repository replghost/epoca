# useragent-extensions-wasm (vendored)

Host extension registry (window.host.ext.* — CRDT et al.) from UserAgentKit,
compiled to WebAssembly. Loaded alongside the core engine in
useragent-wasm/ (split upstream so hosts opt in to the extension layer).

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-extensions-wasm`)
- Commit: bf6755b754dd504a1285bac61f989c60598e203e (with LocalBroadcastCrdtRuntime, uncommitted)
- License: AGPL-3.0 (see LICENSE in this directory)
- Build command (from the useragent-kit checkout root):

  ```
  wasm-pack build crates/host-extensions-wasm --target web --release
  ```

To update: rebuild at the new commit with the same command and copy
`pkg/useragent_extensions_wasm.js` + `pkg/useragent_extensions_wasm_bg.wasm`
here, then update the commit hash above.
