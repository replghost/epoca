// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process singleton wrapping the vendored UserAgentKit TrUAPI host
// engine (WASM). Lazily instantiates the engine on first use and exposes
// frame-level dispatch for the EpocaProduct actor.

const GLUE_URL = "resource:///modules/epoca/useragent_wasm.js";
const WASM_URL = "resource:///modules/epoca/useragent_wasm_bg.wasm";

async function readBinaryResource(url) {
  if (typeof fetch === "function") {
    const response = await fetch(url);
    return response.arrayBuffer();
  }
  // Fallback for contexts without fetch: resolve the resource substitution
  // to a file and read it directly (works in unpackaged local builds).
  const resHandler = Services.io
    .getProtocolHandler("resource")
    .QueryInterface(Ci.nsIResProtocolHandler);
  const fileUrl = resHandler.resolveURI(Services.io.newURI(url));
  const path = Services.io
    .newURI(fileUrl)
    .QueryInterface(Ci.nsIFileURL).file.path;
  const bytes = await IOUtils.read(path);
  return bytes.buffer;
}

export const EpocaHostEngine = {
  _enginePromise: null,
  _gluePromise: null,

  /**
   * The initialized wasm-bindgen glue module (HostApiHandle, WalletHandle,
   * free functions). Shared with EpocaWallet so the wasm instantiates once.
   */
  glue() {
    if (!this._gluePromise) {
      this._gluePromise = (async () => {
        // The engine's time source reads globalThis.performance (for the
        // wallet auto-lock clock); the system-module global doesn't provide
        // it, so shim a monotonic-enough clock before instantiating.
        if (typeof globalThis.performance === "undefined") {
          globalThis.performance = { now: () => Date.now() };
        }
        const glue = ChromeUtils.importESModule(GLUE_URL, {
          global: "current",
        });
        const wasmBytes = await readBinaryResource(WASM_URL);
        await glue.default({ module_or_path: wasmBytes });
        return glue;
      })();
    }
    return this._gluePromise;
  },

  _ensure() {
    if (!this._enginePromise) {
      this._enginePromise = this._create();
    }
    return this._enginePromise;
  },

  async _create() {
    const glue = await this.glue();
    const api = new glue.HostApiHandle();
    // Pass no legacy accounts: exposing the soft-derivation root identity to
    // products is a known cross-product-correlation hazard (DER-001).
    api.setAccounts("[]");
    return api;
  },

  /**
   * Dispatch one TrUAPI frame from a product to the engine.
   *
   * @param {Uint8Array} frame - SCALE-encoded request frame.
   * @param {string} productId - Identifier of the originating product.
   * @returns {Promise<object>} the engine outcome (discriminated on .type).
   */
  async handleMessage(frame, productId) {
    const engine = await this._ensure();
    return engine.handleMessage(frame, productId);
  },

  /**
   * Invoke one of the engine's encode*Response methods, e.g.
   * encodeStorageReadResponse(request_id, value?).
   *
   * @returns {Promise<Uint8Array>} the SCALE response frame.
   */
  async encodeResponse(method, ...args) {
    const engine = await this._ensure();
    return engine[method](...args);
  },
};
