// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process dotNS resolution: <name>.dot -> on-chain contenthash
// lookup -> IPFS bundle fetch -> asset map, via the vendored UserAgentKit
// engine (DotnsHandle.resolveAndFetch, which performs both the JSON-RPC
// state_call and the gateway fetch internally). Results feed
// EpocaDotAppRegistry so dotapp://<name>/ loads serve from memory.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
});

// A dotNS label as accepted by the resolver, without the .dot suffix.
export const DOT_LABEL_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;

export const EpocaDotNs = {
  _handlePromise: null,
  // name -> Promise<assets>; dedupes concurrent resolutions and caches
  // successes for the session (products are content-addressed; a stale CID
  // is refreshed on browser restart or explicit re-resolve()).
  _resolutions: new Map(),

  _handle() {
    if (!this._handlePromise) {
      this._handlePromise = lazy.EpocaHostEngine.glue().then(
        glue => new glue.DotnsHandle()
      );
    }
    return this._handlePromise;
  },

  /**
   * Resolve a dotNS name and fetch its bundle.
   *
   * @param {string} name - The label without the .dot suffix, e.g. "browse".
   * @returns {Promise<object>} asset map of path -> Uint8Array.
   */
  resolve(name) {
    if (!DOT_LABEL_RE.test(name)) {
      return Promise.reject(new Error(`invalid dot name: ${name}`));
    }
    let pending = this._resolutions.get(name);
    if (!pending) {
      pending = this._handle().then(handle =>
        handle.resolveAndFetch(`${name}.dot`)
      );
      pending.catch(() => this._resolutions.delete(name));
      this._resolutions.set(name, pending);
    }
    return pending;
  },
};
