// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Content-side half of the product bridge. At document creation (before any
// page script runs) it publishes the two globals every TrUAPI product looks
// for — `window.__HOST_WEBVIEW_MARK__` and `window.__HOST_API_PORT__` (a
// MessagePort) — and relays raw byte frames between that port and the
// parent-process host engine.
//
// The MessageChannel and its listeners live in a same-principal sandbox in
// the content compartment (the same technique WebExtension content scripts
// use), so the page receives a genuine MessagePort. Only two functions cross
// the privilege boundary: an exported page->chrome frame callback and the
// chrome->page deliver function returned by the bootstrap script.

const BOOTSTRAP = `
  (function () {
    "use strict";
    const channel = new MessageChannel();
    channel.port1.onmessage = event => __epocaFrameToHost(event.data);
    window.__HOST_API_PORT__ = channel.port2;
    window.__HOST_WEBVIEW_MARK__ = true;
    return bytes => channel.port1.postMessage(bytes);
  })();
`;

// Product origins, plus any http(s) page for PoC/bridge testing while the
// epoca.useragent.enabled pref is flipped. This gate lives here because the
// actor's `matches` cannot express dotapp URIs (see EpocaUserAgent.sys.mjs).
const BRIDGE_SCHEMES = new Set(["dotapp", "https", "http"]);

export class EpocaProductChild extends JSWindowActorChild {
  #sandbox = null;
  // Content-side function that posts a host frame to the page's port.
  #deliverToProduct = null;

  handleEvent(event) {
    if (
      event.type === "DOMDocElementInserted" &&
      BRIDGE_SCHEMES.has(this.document?.documentURIObject?.scheme)
    ) {
      this.#installBridge();
    }
  }

  didDestroy() {
    if (this.#sandbox) {
      Cu.nukeSandbox(this.#sandbox);
      this.#sandbox = null;
      this.#deliverToProduct = null;
    }
  }

  #installBridge() {
    const win = this.contentWindow;
    if (!win || this.#sandbox) {
      return;
    }

    const sandbox = Cu.Sandbox(win, {
      sandboxName: "epoca-product-bridge",
      sandboxPrototype: win,
      sameZoneAs: win,
      // Without this, assignments to window.* through the sandbox become
      // Xray expandos that the page never sees.
      wantXrays: false,
    });
    Cu.exportFunction(data => this.#onProductFrame(data), sandbox, {
      defineAs: "__epocaFrameToHost",
    });

    this.#deliverToProduct = Cu.evalInSandbox(BOOTSTRAP, sandbox);
    this.#sandbox = sandbox;
  }

  #onProductFrame(data) {
    try {
      // Structured clone operates below the compartment wrappers, so the
      // content-side Uint8Array can cross to the parent directly. Never
      // touch the bytes JS-side here: TypedArray access over Xrays throws.
      this.sendAsyncMessage("EpocaProduct:Frame", data);
    } catch (e) {
      // Products must send structured-cloneable frames; drop anything else.
    }
  }

  receiveMessage(message) {
    if (message.name === "EpocaProduct:HostFrame" && this.#deliverToProduct) {
      this.#deliverToProduct(Cu.cloneInto(message.data, this.#sandbox));
    }
  }
}
