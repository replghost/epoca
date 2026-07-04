// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process half of the product bridge. Receives TrUAPI frames from
// the content actor and dispatches them to the UserAgentKit host engine,
// mediating the outcomes that require host resources: storage, account
// access, and signing.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaChainService: "resource:///modules/EpocaChainService.sys.mjs",
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
// "<productId>:<kind>" — device permissions granted this session.
const gDeviceGrants = new Set();

// Gecko permission type + consent wording per TrUAPI DevicePermissionKind.
// Kinds without a Gecko permission only gate the product-side API.
const DEVICE_PERMISSIONS = new Map([
  ["Camera", { geckoType: "camera", label: "use your camera" }],
  ["Microphone", { geckoType: "microphone", label: "use your microphone" }],
  ["Location", { geckoType: "geo", label: "see your location" }],
  ["Notifications", { geckoType: "desktop-notification", label: "show notifications" }],
  ["Clipboard", { geckoType: "clipboard-read", label: "read your clipboard" }],
  ["Bluetooth", { geckoType: null, label: "use Bluetooth" }],
  ["Nfc", { geckoType: null, label: "use NFC" }],
  ["OpenUrl", { geckoType: null, label: "open external links" }],
  ["Biometrics", { geckoType: null, label: "use biometric authentication" }],
]);

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
      console.debug(
        `EpocaProduct[${productId}]: outcome ${outcome?.type}` +
          (outcome?.json_rpc_method ? ` ${outcome.json_rpc_method}` : "") +
          (outcome?.method ? ` ${outcome.method}` : "")
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

      case "NeedsChainQuery":
        await this.#handleChainQuery(outcome);
        break;

      case "NeedsChainRpc":
        await this.#handleChainRpc(outcome);
        break;

      case "NeedsChainFollow":
        await this.#handleChainFollow(outcome);
        break;

      case "NeedsNavigate":
        await this.#handleNavigate(outcome);
        break;

      case "NeedsDevicePermission":
        await this.#handleDevicePermission(outcome, productId);
        break;

      case "NeedsChainFollowStop": {
        const stop = this.#chainFollows.get(outcome.request_id);
        if (stop) {
          this.#chainFollows.delete(outcome.request_id);
          stop();
        }
        break;
      }

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

  // Active chainHead follow subscriptions, keyed by the follow request id.
  #chainFollows = new Map();
  // genesis hex -> {promise, resolve} for the server-side follow
  // subscription id. chainHead operations must carry the SERVER's
  // subscription id as their first JSON-RPC param, but the engine only
  // knows the product-SDK's opaque follow_sub_id — the host owns this
  // translation. One live follow per (product, genesis) is assumed, which
  // matches the product-SDK's follow lifecycle.
  #followServerIds = new Map();

  didDestroy() {
    for (const stop of this.#chainFollows.values()) {
      stop();
    }
    this.#chainFollows.clear();
    this.#followServerIds.clear();
  }

  // wasm-bindgen serializes serde_json maps as JS Maps, which JSON.stringify
  // flattens to {}. Convert to plain objects before putting them on the wire.
  static #plainJson(value) {
    if (value instanceof Map) {
      return Object.fromEntries(
        [...value].map(([k, v]) => [k, EpocaProductParent.#plainJson(v)])
      );
    }
    if (Array.isArray(value)) {
      return value.map(v => EpocaProductParent.#plainJson(v));
    }
    return value;
  }

  // Legacy single-chain query: no genesis in the outcome, route to the
  // default network. The raw JSON-RPC response text goes back verbatim;
  // the engine parses and re-encodes it.
  async #handleChainQuery(outcome) {
    try {
      const genesis = lazy.EpocaChainService.defaultGenesis();
      if (!genesis) {
        throw new Error("no chain configured");
      }
      const response = await lazy.EpocaChainService.sendRpc(
        genesis,
        outcome.method,
        EpocaProductParent.#plainJson(outcome.params ?? [])
      );
      await this.#reply(
        "encodeChainQueryResponse",
        outcome.request_id,
        response
      );
    } catch (e) {
      console.error("EpocaProduct: chain query failed", e);
      await this.#reply("encodeChainQueryError", outcome.request_id);
    }
  }

  async #handleChainRpc(outcome) {
    try {
      const genesis = lazy.EpocaChainService.routeByGenesis(
        outcome.genesis_hash
      );
      if (!genesis) {
        throw new Error("unsupported chain");
      }
      let params = EpocaProductParent.#plainJson(outcome.json_rpc_params ?? []);
      if (outcome.follow_sub_id != null) {
        const ack = this.#followServerIds.get(genesis);
        if (!ack) {
          throw new Error("no active chainHead follow for this chain");
        }
        params = [await ack.promise, ...params];
      }
      const response = await lazy.EpocaChainService.sendRpc(
        genesis,
        outcome.json_rpc_method,
        params
      );
      await this.#reply(
        "encodeChainRpcResponse",
        outcome.request_id,
        outcome.request_tag,
        response
      );
    } catch (e) {
      console.error(
        `EpocaProduct: chain rpc ${outcome.json_rpc_method} failed`,
        e
      );
      await this.#reply(
        "encodeChainRpcError",
        outcome.request_id,
        outcome.request_tag,
        String(e?.message || e)
      );
    }
  }

  // Real nodes emit runtime specs with `apis` as an object map; the engine's
  // typed followEvent parser expects an array of [name, version] pairs and
  // silently drops events it cannot parse. Mirror the normalization done by
  // useragent-kit's runtime-chain-service.
  static #normalizeFollowEvent(text) {
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      return text;
    }
    if (parsed?.method !== "chainHead_v1_followEvent") {
      return text;
    }
    const result = parsed.params?.result;
    const runtime =
      result?.event === "initialized"
        ? result.finalizedBlockRuntime
        : result?.event === "newBlock"
          ? result.newRuntime
          : null;
    const apis = runtime?.spec?.apis;
    if (!apis || Array.isArray(apis)) {
      return text;
    }
    runtime.spec.apis = Object.entries(apis).filter(
      ([, version]) => typeof version === "number"
    );
    return JSON.stringify(parsed);
  }

  async #handleChainFollow(outcome) {
    const requestId = outcome.request_id;
    const genesis = lazy.EpocaChainService.routeByGenesis(
      outcome.genesis_hash
    );
    const abort = async () => {
      this.#chainFollows.delete(requestId);
      if (genesis && this.#followServerIds.get(genesis)?.owner === requestId) {
        this.#followServerIds.delete(genesis);
      }
      await this.#reply("encodeChainFollowStop", requestId);
    };
    try {
      if (!genesis) {
        throw new Error("unsupported chain");
      }
      const ack = Promise.withResolvers();
      ack.owner = requestId;
      this.#followServerIds.set(genesis, ack);
      const stop = await lazy.EpocaChainService.startSubscription(
        genesis,
        "chainHead_v1_follow",
        [outcome.with_runtime],
        async jsonRpc => {
          const normalized = EpocaProductParent.#normalizeFollowEvent(jsonRpc);
          let parsed;
          try {
            parsed = JSON.parse(normalized);
          } catch {
            parsed = null;
          }
          // First message is the ack carrying the server subscription id.
          if (parsed?.id != null && typeof parsed.result === "string") {
            ack.resolve(parsed.result);
            return;
          }
          const frame = await lazy.EpocaHostEngine.encodeResponse(
            "encodeChainFollowEvent",
            requestId,
            normalized
          );
          if (frame?.length) {
            this.#sendFrame(frame);
          } else if (normalized.includes("followEvent")) {
            console.warn(
              "EpocaProduct: follow event dropped by encoder:",
              normalized.slice(0, 300)
            );
          }
        },
        () => abort()
      );
      this.#chainFollows.set(requestId, stop);
    } catch (e) {
      console.error("EpocaProduct: chain follow failed", e);
      await abort();
    }
  }

  // Device access (camera for QR scanning, etc): consent once per product
  // and kind per session. A grant also sets the matching Gecko permission
  // on the product's origin for the session, so the follow-up DOM API call
  // (e.g. getUserMedia) doesn't raise a second doorhanger.
  async #handleDevicePermission(outcome, productId) {
    const kind = String(outcome.kind);
    const known = DEVICE_PERMISSIONS.get(kind);
    if (!known) {
      console.warn(`EpocaProduct: unknown device permission kind '${kind}'`);
      await this.#reply("encodeDevicePermissionError", outcome.request_id);
      return;
    }
    const grantKey = `${productId}:${kind}`;
    const granted =
      gDeviceGrants.has(grantKey) ||
      (await this.#confirm(
        "Device access",
        `${productId} wants to ${known.label}.`
      ));
    if (granted) {
      gDeviceGrants.add(grantKey);
      if (known.geckoType) {
        Services.perms.addFromPrincipal(
          this.manager.documentPrincipal,
          known.geckoType,
          Services.perms.ALLOW_ACTION,
          Services.perms.EXPIRE_SESSION
        );
      }
    }
    await this.#reply(
      "encodeDevicePermissionResponse",
      outcome.request_id,
      granted
    );
  }

  // Map a navigateTo target onto the local product scheme. Accepted forms:
  // a bare dot name ("coinflip.dot"), and web-host product URLs that other
  // hosts use ("https://<label>.dot.li/...", "https://<label>.app.paseo.li/
  // ..."), which products construct when they assume the dotli deployment.
  // Everything else is null — a product must not steer its tab to the web.
  static #navigateTargetToDotUrl(target) {
    const name = /^([a-z0-9][a-z0-9-]{0,63})\.dot$/i.exec(target);
    if (name) {
      return `dot://${name[1].toLowerCase()}.dot/`;
    }
    let url;
    try {
      url = new URL(target);
    } catch {
      return null;
    }
    if (url.protocol !== "https:") {
      return null;
    }
    const host = /^([a-z0-9][a-z0-9-]{0,63})(?:\.app\.paseo\.li|\.dot\.li)$/i.exec(
      url.hostname
    );
    if (!host) {
      return null;
    }
    return `dot://${host[1].toLowerCase()}.dot${url.pathname}${url.search}`;
  }

  async #handleNavigate(outcome) {
    const dotUrl = EpocaProductParent.#navigateTargetToDotUrl(
      outcome.url?.trim() ?? ""
    );
    if (dotUrl) {
      try {
        this.browsingContext.top.loadURI(Services.io.newURI(dotUrl), {
          triggeringPrincipal:
            Services.scriptSecurityManager.getSystemPrincipal(),
        });
      } catch (e) {
        console.error(`EpocaProduct: navigate to ${outcome.url} failed`, e);
      }
    } else {
      console.warn(`EpocaProduct: refused navigate to '${outcome.url}'`);
    }
    await this.#reply("encodeNavigateResponse", outcome.request_id);
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
    // dot://<label>.dot documents key host state on the bare label; http(s)
    // PoC pages fall back to their full host.
    const host = this.manager?.documentPrincipal?.host;
    if (!host) {
      return "unknown-product";
    }
    return this.manager.documentPrincipal.schemeIs("dot")
      ? host.replace(/\.dot$/, "")
      : host;
  }
}
