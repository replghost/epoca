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

// Extension bridge: the core window.host surface (host.on/off, __hostCall,
// __hostResolve, __hostPush) plus the window.host.ext.crdt namespace.
// The core surface mirrors useragent-kit's reference ProductView script and
// the crdt methods mirror HOST_EXT_CRDT_SCRIPT (crates/host-ext-crdt) — keep
// both in sync with upstream. Calls travel page -> __epocaExtCall -> parent
// -> shared extension registry; results and push events come back through
// the dispatcher function this script returns to the actor.
const EXT_BOOTSTRAP = `
  (function () {
    "use strict";
    var pending = new Map();
    var nextCallId = 1;
    var listeners = new Map();

    if (!window.host) { window.host = {}; }
    if (!window.host.ext) { window.host.ext = {}; }

    window.host.on = function (eventName, callback) {
      if (!listeners.has(eventName)) { listeners.set(eventName, new Set()); }
      listeners.get(eventName).add(callback);
    };
    window.host.off = function (eventName, callback) {
      if (!listeners.has(eventName)) { return; }
      listeners.get(eventName).delete(callback);
      if (listeners.get(eventName).size === 0) { listeners.delete(eventName); }
    };
    window.__hostCall = function (channel, method, params) {
      return new Promise(function (resolve, reject) {
        var callId = nextCallId++;
        pending.set(callId, { resolve: resolve, reject: reject });
        try {
          __epocaExtCall(JSON.stringify({
            callId: callId,
            channel: channel,
            method: method,
            params: params || {}
          }));
        } catch (e) {
          pending.delete(callId);
          reject(e);
        }
      });
    };
    window.__hostResolve = function (callId, ok, value) {
      var entry = pending.get(callId);
      if (!entry) { return; }
      pending.delete(callId);
      if (ok) { entry.resolve(value); } else { entry.reject(value); }
    };
    window.__hostPush = function (eventName, payload) {
      var callbacks = listeners.get(eventName);
      if (!callbacks) { return; }
      callbacks.forEach(function (callback) {
        try { callback(payload); } catch (e) {}
      });
    };

    window.host.ext.crdt = Object.freeze({
      join: function (roomId, opts) {
        return window.__hostCall('hostBridge', 'crdtJoin', {
          roomId: roomId,
          transport: (opts && opts.transport) || 'relay'
        });
      },
      applyUpdate: function (roomId, dataBase64) {
        return window.__hostCall('hostBridge', 'crdtApplyUpdate', {
          roomId: roomId,
          dataBase64: dataBase64
        });
      },
      getStateVector: function (roomId) {
        return window.__hostCall('hostBridge', 'crdtGetStateVector', {
          roomId: roomId
        });
      },
      getFullState: function (roomId) {
        return window.__hostCall('hostBridge', 'crdtGetFullState', {
          roomId: roomId
        });
      },
      setAwareness: function (roomId, state) {
        return window.__hostCall('hostBridge', 'crdtSetAwareness', {
          roomId: roomId,
          state: JSON.stringify(state)
        });
      },
      destroy: function (roomId) {
        return window.__hostCall('hostBridge', 'crdtDestroy', {
          roomId: roomId
        });
      }
    });

    // Chrome can call a returned content *function* directly (same mechanism
    // as the frame deliverer above) but not methods on a returned object, so
    // the bridge entry point is a single dispatcher function.
    return function (kind, a, b, c) {
      if (kind === 'resolve') {
        var value = null;
        if (typeof c === 'string') {
          try { value = JSON.parse(c); } catch (e) { value = c; }
        }
        window.__hostResolve(a, b, value);
      } else if (kind === 'push') {
        var payload = b;
        if (typeof b === 'string') {
          try { payload = JSON.parse(b); } catch (e) {}
        }
        window.__hostPush(a, payload);
      }
    };
  })();
`;

export class EpocaProductChild extends JSWindowActorChild {
  #sandbox = null;
  // Content-side function that posts a host frame to the page's port.
  #deliverToProduct = null;
  // Content-side dispatcher function for the extension bridge.
  #extBridge = null;

  // dot:// products always get the host bridge — it is the whole point of the
  // scheme, and product host-api clients (e.g. host-api-wrapper) refuse to run
  // until they see the injected __HOST_WEBVIEW_MARK__/__HOST_API_PORT__. http(s)
  // pages only get it while epoca.useragent.enabled is flipped, for PoC/testing.
  // (This gate lives here because the actor's `matches` cannot express dot URIs;
  // see EpocaUserAgent.sys.mjs.)
  handleEvent(event) {
    if (event.type !== "DOMDocElementInserted") {
      return;
    }
    const scheme = this.document?.documentURIObject?.scheme;
    if (scheme === "dot") {
      this.#installBridge();
    } else if (
      (scheme === "https" || scheme === "http") &&
      Services.prefs.getBoolPref("epoca.useragent.enabled", false)
    ) {
      this.#installBridge();
    }
  }

  didDestroy() {
    if (this.#sandbox) {
      Cu.nukeSandbox(this.#sandbox);
      this.#sandbox = null;
      this.#deliverToProduct = null;
      this.#extBridge = null;
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
    Cu.exportFunction(json => this.#onExtensionCall(json), sandbox, {
      defineAs: "__epocaExtCall",
    });

    this.#deliverToProduct = Cu.evalInSandbox(BOOTSTRAP, sandbox);
    this.#extBridge = Cu.evalInSandbox(EXT_BOOTSTRAP, sandbox);
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

  #onExtensionCall(json) {
    if (typeof json !== "string") {
      return;
    }
    try {
      this.sendAsyncMessage("EpocaProduct:ExtCall", json);
    } catch (e) {
      // Actor torn down.
    }
  }

  receiveMessage(message) {
    if (message.name === "EpocaProduct:HostFrame" && this.#deliverToProduct) {
      this.#deliverToProduct(Cu.cloneInto(message.data, this.#sandbox));
    } else if (
      message.name === "EpocaProduct:ExtResolve" &&
      this.#extBridge
    ) {
      const { callId, ok, valueJson } = message.data;
      this.#extBridge("resolve", callId, !!ok, valueJson ?? null);
    } else if (message.name === "EpocaProduct:ExtPush" && this.#extBridge) {
      const { event, payloadJson } = message.data;
      this.#extBridge("push", event, payloadJson);
    }
  }
}
