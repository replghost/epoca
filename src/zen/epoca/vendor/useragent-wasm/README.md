# useragent-wasm (vendored)

TrUAPI host engine from UserAgentKit, compiled to WebAssembly.

- Upstream: https://github.com/paritytech/useragent-kit (crate `crates/host-wasm`)
- Commit: bf6755b754dd504a1285bac61f989c60598e203e
- License: AGPL-3.0 (see LICENSE in this directory)
- Build command (from the useragent-kit checkout root):

  ```
  CC_wasm32_unknown_unknown=/opt/homebrew/opt/llvm/bin/clang \
    wasm-pack build crates/host-wasm --target web --release --out-dir pkg-web
  ```

  `wasm-opt` is intentionally disabled by the crate (bulk-memory
  incompatibility), so the .wasm is the final artifact.

To update: rebuild at the new commit with the same command and copy
`pkg-web/useragent_wasm.js` + `pkg-web/useragent_wasm_bg.wasm` here, then
update the commit hash above.
