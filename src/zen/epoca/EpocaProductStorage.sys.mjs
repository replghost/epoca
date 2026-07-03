// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Profile-persisted key/value storage backing the TrUAPI local-storage
// host functions. Values are opaque bytes, namespaced per product — the
// engine does not embed the product id in storage outcomes, so scoping
// here is what keeps products from reading each other's keys.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  JSONFile: "resource://gre/modules/JSONFile.sys.mjs",
});

export const EpocaProductStorage = {
  _filePromise: null,

  _ensure() {
    if (!this._filePromise) {
      this._filePromise = (async () => {
        const file = new lazy.JSONFile({
          path: PathUtils.join(PathUtils.profileDir, "epoca", "storage.json"),
        });
        await file.load();
        return file;
      })();
    }
    return this._filePromise;
  },

  /** @returns {Promise<Uint8Array|null>} stored bytes, or null if absent. */
  async get(productId, key) {
    const file = await this._ensure();
    const encoded = file.data[productId]?.[key];
    if (encoded === undefined) {
      return null;
    }
    return new Uint8Array(
      ChromeUtils.base64URLDecode(encoded, { padding: "reject" })
    );
  },

  /** @param {Uint8Array} value */
  async set(productId, key, value) {
    const file = await this._ensure();
    file.data[productId] ??= {};
    file.data[productId][key] = ChromeUtils.base64URLEncode(value, {
      pad: false,
    });
    file.saveSoon();
  },

  async remove(productId, key) {
    const file = await this._ensure();
    if (file.data[productId]) {
      delete file.data[productId][key];
      file.saveSoon();
    }
  },
};
