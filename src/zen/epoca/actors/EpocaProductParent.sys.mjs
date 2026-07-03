// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process half of the product bridge. Receives TrUAPI frames from
// the content actor and dispatches them to the UserAgentKit host engine.
//
// Only self-contained outcomes (Response/Silent) are handled so far; the
// Needs* outcomes (signing, chain access, storage, permissions...) arrive
// with wallet/chain integration in later phases.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
  EpocaProductStorage: "resource:///modules/EpocaProductStorage.sys.mjs",
});

export class EpocaProductParent extends JSWindowActorParent {
  async receiveMessage(message) {
    if (message.name !== "EpocaProduct:Frame") {
      return;
    }

    try {
      const productId = this.#productId();
      const outcome = await lazy.EpocaHostEngine.handleMessage(
        new Uint8Array(message.data),
        productId
      );
      await this.#handleOutcome(outcome, productId);
    } catch (e) {
      console.error("EpocaProduct: engine dispatch failed", e);
    }
  }

  async #handleOutcome(outcome, productId) {
    switch (outcome?.type) {
      case "Response":
        // The engine returns response bytes as a plain number[]; convert
        // before shipping across processes.
        this.#sendFrame(Uint8Array.from(outcome.data));
        break;
      case "Silent":
        break;
      case "NeedsStorageRead": {
        const value = await lazy.EpocaProductStorage.get(
          productId,
          outcome.key
        );
        this.#sendFrame(
          await lazy.EpocaHostEngine.encodeResponse(
            "encodeStorageReadResponse",
            outcome.request_id,
            value
          )
        );
        break;
      }
      case "NeedsStorageWrite":
        await lazy.EpocaProductStorage.set(
          productId,
          outcome.key,
          Uint8Array.from(outcome.value)
        );
        this.#sendFrame(
          await lazy.EpocaHostEngine.encodeResponse(
            "encodeStorageWriteResponse",
            outcome.request_id
          )
        );
        break;
      case "NeedsStorageClear":
        await lazy.EpocaProductStorage.remove(productId, outcome.key);
        this.#sendFrame(
          await lazy.EpocaHostEngine.encodeResponse(
            "encodeStorageClearResponse",
            outcome.request_id
          )
        );
        break;
      default:
        console.warn(
          `EpocaProduct: unhandled engine outcome '${outcome?.type}'`
        );
    }
  }

  #sendFrame(bytes) {
    this.sendAsyncMessage("EpocaProduct:HostFrame", bytes);
  }

  #productId() {
    // Placeholder identity until dotapp:// origins land: key the engine's
    // per-product state on the document's host.
    return this.manager?.documentPrincipal?.host || "unknown-product";
  }
}
