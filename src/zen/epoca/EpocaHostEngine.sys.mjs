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

  _ensure() {
    if (!this._enginePromise) {
      this._enginePromise = this._create();
    }
    return this._enginePromise;
  },

  async _create() {
    const glue = ChromeUtils.importESModule(GLUE_URL, { global: "current" });
    const wasmBytes = await readBinaryResource(WASM_URL);
    await glue.default({ module_or_path: wasmBytes });
    return new glue.HostApiHandle();
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
};
