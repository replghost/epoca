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
});

export class EpocaProductParent extends JSWindowActorParent {
  async receiveMessage(message) {
    if (message.name !== "EpocaProduct:Frame") {
      return;
    }

    try {
      const outcome = await lazy.EpocaHostEngine.handleMessage(
        new Uint8Array(message.data),
        this.#productId()
      );
      this.#handleOutcome(outcome);
    } catch (e) {
      console.error("EpocaProduct: engine dispatch failed", e);
    }
  }

  #handleOutcome(outcome) {
    switch (outcome?.type) {
      case "Response":
        // The engine returns response bytes as a plain number[]; convert
        // before shipping across processes.
        this.sendAsyncMessage(
          "EpocaProduct:HostFrame",
          Uint8Array.from(outcome.data)
        );
        break;
      case "Silent":
        break;
      default:
        console.warn(
          `EpocaProduct: unhandled engine outcome '${outcome?.type}'`
        );
    }
  }

  #productId() {
    // Placeholder identity until dotapp:// origins land: key the engine's
    // per-product state on the document's host.
    return this.manager?.documentPrincipal?.host || "unknown-product";
  }
}
