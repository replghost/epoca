// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process half of the product bridge. Receives TrUAPI frames from
// the content actor and dispatches them to the UserAgentKit host engine,
// mediating the outcomes that require host resources: storage, account
// access, and signing.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
  EpocaProductStorage: "resource:///modules/EpocaProductStorage.sys.mjs",
  EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
});

const AUTO_APPROVE_PREF = "epoca.useragent.auto-approve";

// Cross-product, cross-window state: one wallet, one user.
// Account grants are cached per product once approved; a signature request
// while another is pending is rejected outright (no queuing, BRG-011).
const gAccountGrants = new Set();
let gSignInFlight = false;

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
        await this.#reply("encodeStorageReadResponse", outcome.request_id, value);
        break;
      }

      case "NeedsStorageWrite":
        await lazy.EpocaProductStorage.set(
          productId,
          outcome.key,
          Uint8Array.from(outcome.value)
        );
        await this.#reply("encodeStorageWriteResponse", outcome.request_id);
        break;

      case "NeedsStorageClear":
        await lazy.EpocaProductStorage.remove(productId, outcome.key);
        await this.#reply("encodeStorageClearResponse", outcome.request_id);
        break;

      case "NeedsAccountGet":
        await this.#handleAccountGet(outcome, productId);
        break;

      case "NeedsSign":
        await this.#handleSign(outcome, productId);
        break;

      case "NeedsCreateTransaction":
        // No chain backend yet, so we can't build a signed extrinsic.
        await this.#reply(
          "encodeCreateTransactionError",
          outcome.request_id,
          "not_supported"
        );
        break;

      case "NeedsCreateTransactionLegacyAccount":
        await this.#reply(
          "encodeCreateTxNonProductError",
          outcome.request_id,
          "not_supported"
        );
        break;

      default:
        console.warn(
          `EpocaProduct: unhandled engine outcome '${outcome?.type}'`
        );
    }
  }

  async #handleAccountGet(outcome, productId) {
    const granted =
      gAccountGrants.has(productId) ||
      (await this.#confirm(
        "Account access",
        `${productId} wants to see its account address.`
      ));
    if (!granted) {
      await this.#reply(
        "encodeAccountGetError",
        outcome.request_id,
        "Rejected"
      );
      return;
    }
    gAccountGrants.add(productId);
    const publicKey = await lazy.EpocaWallet.appPublicKey(
      outcome.account.dotns_id,
      outcome.account.derivation_index
    );
    await this.#reply(
      "encodeAccountGetResponse",
      outcome.request_id,
      publicKey
    );
  }

  async #handleSign(outcome, productId) {
    // BRG-011: never queue a second signature request.
    if (gSignInFlight) {
      await this.#reply(
        "encodeSignError",
        outcome.request_id,
        outcome.request_tag
      );
      return;
    }
    gSignInFlight = true;
    try {
      // BRG-010: signing always requires explicit, per-request approval.
      const approved = await this.#confirm(
        "Signature request",
        `${productId} wants to sign a message with your account.`
      );
      if (!approved) {
        await this.#reply(
          "encodeSignError",
          outcome.request_id,
          outcome.request_tag
        );
        return;
      }
      const signature = await lazy.EpocaWallet.sign(
        outcome.account.dotns_id,
        outcome.account.derivation_index,
        Uint8Array.from(outcome.payload)
      );
      await this.#reply(
        "encodeSignResponse",
        outcome.request_id,
        outcome.request_tag,
        signature
      );
    } finally {
      gSignInFlight = false;
    }
  }

  async #reply(method, ...args) {
    this.#sendFrame(await lazy.EpocaHostEngine.encodeResponse(method, ...args));
  }

  async #confirm(title, message) {
    if (Services.prefs.getBoolPref(AUTO_APPROVE_PREF, false)) {
      return true;
    }
    const flags =
      Services.prompt.BUTTON_TITLE_IS_STRING * Services.prompt.BUTTON_POS_0 +
      Services.prompt.BUTTON_TITLE_IS_STRING * Services.prompt.BUTTON_POS_1;
    const result = await Services.prompt.asyncConfirmEx(
      this.browsingContext,
      Services.prompt.MODAL_TYPE_TAB,
      title,
      message,
      flags,
      "Allow",
      "Deny",
      null,
      null,
      false
    );
    return result.getProperty("buttonNumClicked") === 0;
  }

  #sendFrame(bytes) {
    this.sendAsyncMessage("EpocaProduct:HostFrame", bytes);
  }

  #productId() {
    // Placeholder identity until dotapp:// origins land: key host state on
    // the document's host.
    return this.manager?.documentPrincipal?.host || "unknown-product";
  }
}
