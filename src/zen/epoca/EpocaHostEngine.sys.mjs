// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process singleton wrapping the vendored UserAgentKit TrUAPI host
// engine (WASM). Lazily instantiates the engine on first use and exposes
// frame-level dispatch for the EpocaProduct actor.

const GLUE_URL = "resource:///modules/epoca/useragent_wasm.js";
const WASM_URL = "resource:///modules/epoca/useragent_wasm_bg.wasm";
const EXT_GLUE_URL =
  "resource:///modules/epoca/useragent_extensions_wasm.js";
const EXT_WASM_URL =
  "resource:///modules/epoca/useragent_extensions_wasm_bg.wasm";

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaChainService: "resource:///modules/EpocaChainService.sys.mjs",
  EpocaCrdtRelay: "resource:///modules/EpocaCrdtRelay.sys.mjs",
});

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
  _extRegistryPromise: null,
  _extSubscribers: new Set(),

  // The RelayCrdtRuntime handle (sans-IO): epoca drives its I/O — statement
  // subscribe/fetch/sign/submit — through EpocaCrdtRelay. Set alongside the
  // registry. A per-process random sender id lets the runtime drop its own
  // statement echoes on ingest.
  _relay: null,
  _relayGlue: null,

  /**
   * The extension registry (window.host.ext.* — CRDT et al.), from the
   * vendored useragent-extensions-wasm bundle. One registry per browser
   * process. The CRDT extension runs the RelayCrdtRuntime: updates still fan
   * out to this device's tabs via push events, and also relay across devices
   * through the statement store (driven by EpocaCrdtRelay).
   */
  extensionRegistry() {
    if (!this._extRegistryPromise) {
      this._extRegistryPromise = (async () => {
        if (typeof globalThis.performance === "undefined") {
          globalThis.performance = { now: () => Date.now() };
        }
        const glue = ChromeUtils.importESModule(EXT_GLUE_URL, {
          global: "current",
        });
        const wasmBytes = await readBinaryResource(EXT_WASM_URL);
        await glue.default({ module_or_path: wasmBytes });
        const registry = new glue.ExtensionRegistryHandle();
        const senderBytes = new Uint8Array(8);
        crypto.getRandomValues(senderBytes);
        const senderId = Array.from(senderBytes, b =>
          b.toString(16).padStart(2, "0")
        ).join("");
        this._relay = new glue.RelayCrdtHandle(senderId);
        this._relayGlue = glue;
        registry.registerBuiltinsWithRelayCrdt(this._relay);
        lazy.EpocaCrdtRelay.start();
        return registry;
      })();
      this._extRegistryPromise.catch(() => {
        this._extRegistryPromise = null;
      });
    }
    return this._extRegistryPromise;
  },

  /**
   * Subscribe to extension push events. `callback` receives an array of
   * `{event, payloadJson}` objects. Returns an unsubscribe function.
   */
  subscribeExtensionEvents(callback) {
    this._extSubscribers.add(callback);
    return () => this._extSubscribers.delete(callback);
  },

  /**
   * Dispatch an extension call and fan out any push events it produced.
   * The local runtimes only queue events as a result of dispatches, so
   * draining here (no timer) delivers every event promptly.
   *
   * Returns the extension's JSON result string, or null.
   */
  async extensionDispatch(channel, method, paramsJson) {
    const registry = await this.extensionRegistry();
    const result = registry.dispatch(channel, method, paramsJson) ?? null;
    await this.fanExtensionEvents();
    // Drive the statement-store relay: reconcile subscriptions and submit any
    // outbound the dispatch produced. Fire-and-forget (it does network I/O).
    lazy.EpocaCrdtRelay.pump();
    return result;
  },

  /**
   * Drain the extension runtime's queued push events and fan them to product
   * tabs. Called after a dispatch and after the relay ingests a peer statement.
   */
  async fanExtensionEvents() {
    const registry = await this.extensionRegistry();
    let events = [];
    try {
      events = JSON.parse(registry.drainEvents());
    } catch (e) {
      console.error("EpocaHostEngine: drainEvents parse failed", e);
    }
    if (!events.length) {
      return;
    }
    for (const subscriber of this._extSubscribers) {
      try {
        subscriber(events);
      } catch (e) {
        console.error("EpocaHostEngine: extension event subscriber failed", e);
      }
    }
  },

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
      // Never cache a rejection: a transient instantiation failure (e.g. a
      // resource fetch that returned early) would otherwise poison every
      // future wasm op — wallet unlock, signing, restore — until restart.
      // Drop it so the next caller re-attempts from scratch.
      this._gluePromise.catch(e => {
        console.error("EpocaHostEngine: wasm glue instantiation failed", e);
        this._gluePromise = null;
      });
    }
    return this._gluePromise;
  },

  _ensure() {
    if (!this._enginePromise) {
      this._enginePromise = this._create();
      this._enginePromise.catch(() => {
        this._enginePromise = null;
      });
    }
    return this._enginePromise;
  },

  async _create() {
    const glue = await this.glue();
    const api = new glue.HostApiHandle();
    // Pass no legacy accounts: exposing the soft-derivation root identity to
    // products is a known cross-product-correlation hazard (DER-001).
    api.setAccounts("[]");
    // Chains the host can reach; the engine answers featureSupported(Chain)
    // from this set and routes chainHead requests as NeedsChain* outcomes.
    // The chain list is derived from the vendored useragent-kit environment
    // bundle, so ensure it is loaded before querying supported genesis hashes.
    await lazy.EpocaChainService.ensureEnvironment();
    const genesisHashes = lazy.EpocaChainService.supportedGenesisHashes();
    if (genesisHashes.length) {
      api.setSupportedChains(genesisHashes);
    }
    console.debug(
      `EpocaHostEngine: engine ready, ${genesisHashes.length} supported chain(s)`
    );
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
  /**
   * A shared StatementHandle for building/assembling statement-store payloads
   * (buildSigningPayload, assembleStatement). Distinct from HostApiHandle.
   */
  async statementHandle() {
    if (!this._statementHandle) {
      const glue = await this.glue();
      this._statementHandle = new glue.StatementHandle();
    }
    return this._statementHandle;
  },

  async encodeResponse(method, ...args) {
    const engine = await this._ensure();
    return engine[method](...args);
  },

  /**
   * Record a just-in-time remote-permission decision so a replayed message
   * (a `NeedsPermissionPrompt`'s pending bytes) passes the engine's permission
   * gate. `payload` is the prompt outcome's `payload` (a RemotePermission tag).
   *
   * @param {string} productId
   * @param {Uint8Array|number[]} payload
   * @param {boolean} allow
   */
  async storePermissionDecision(productId, payload, allow) {
    const engine = await this._ensure();
    engine.storeRemotePermissionDecision(
      productId,
      Uint8Array.from(payload || []),
      allow
    );
  },
};
