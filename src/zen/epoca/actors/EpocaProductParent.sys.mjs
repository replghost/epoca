// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process half of the product bridge. Receives TrUAPI frames from
// the content actor and dispatches them to the UserAgentKit host engine,
// mediating the outcomes that require host resources: storage, account
// access, and signing.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaAllowance: "resource:///modules/EpocaAllowance.sys.mjs",
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
  // A product that has chain access will follow the chain shortly after it
  // loads; open the WebSocket now so that first follow reaches a finalized
  // block quickly (avoids a cold-connect race where an eager chain check
  // runs before the connection is up). dot:// products only.
  actorCreated() {
    try {
      if (
        Services.prefs.getBoolPref("epoca.chain.prewarm", true) &&
        this.manager?.documentPrincipal?.schemeIs("dot")
      ) {
        lazy.EpocaChainService.prewarm();
      }
    } catch (e) {
      console.warn("EpocaProduct: chain prewarm skipped", e);
    }
    // Forward extension push events (CRDT updates, awareness, peer changes)
    // to this page. Room ids are namespaced "<productId>/<roomId>" on the
    // way in, so only events for this page's product pass the filter.
    this.#unsubscribeExtEvents = lazy.EpocaHostEngine.subscribeExtensionEvents(
      events => {
        const productId = this.#productId();
        for (const { event, payloadJson } of events) {
          const scoped = EpocaProductParent.#payloadForProduct(
            payloadJson,
            productId
          );
          if (scoped === null) {
            continue;
          }
          try {
            this.sendAsyncMessage("EpocaProduct:ExtPush", {
              event,
              payloadJson: scoped,
            });
          } catch (e) {
            // Actor torn down mid-broadcast — the unsubscribe in didDestroy
            // stops future deliveries.
          }
        }
      }
    );
  }

  async receiveMessage(message) {
    if (message.name === "EpocaProduct:ExtCall") {
      await this.#handleExtensionCall(message.data);
      return;
    }
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

      case "NeedsDeriveEntropy":
        try {
          const entropy = await lazy.EpocaWallet.deriveProductEntropy(
            productId,
            Uint8Array.from(outcome.key)
          );
          await this.#reply(
            "encodeDeriveEntropyResponse",
            outcome.request_id,
            entropy
          );
        } catch (e) {
          console.error("EpocaProduct: deriveEntropy failed", e);
          await this.#reply("encodeDeriveEntropyError", outcome.request_id);
        }
        break;

      case "NeedsGetUserId":
        // epoca has no dotNS "primary username" identity for the wallet, so
        // report the typed not-connected error; products fall back to their
        // own connect/onboarding UI.
        await this.#reply(
          "encodeGetUserIdError",
          outcome.request_id,
          "NotConnected",
          null
        );
        break;

      case "NeedsThemeSubscription":
        // Report the browser's current color scheme once (Theme enum:
        // 0 = Light, 1 = Dark). Theme rarely changes mid-session and the
        // product only needs an initial value to render.
        await this.#reply(
          "encodeThemeReceive",
          outcome.request_id,
          [this.#isDarkTheme() ? 1 : 0]
        );
        break;

      case "NeedsStatementStoreSubscription":
        await this.#handleStatementStoreSubscription(outcome);
        break;

      case "NeedsStatementStoreUnsubscribe": {
        const stop = this.#statementSubs.get(outcome.request_id);
        if (stop) {
          this.#statementSubs.delete(outcome.request_id);
          stop();
        }
        break;
      }

      case "NeedsPreimageLookupSubscription":
        await this.#handlePreimageLookup(outcome);
        break;

      case "NeedsPreimageLookupUnsubscribe": {
        const pending = this.#preimageLookups.get(outcome.request_id);
        if (pending) {
          pending.cancelled = true;
          this.#preimageLookups.delete(outcome.request_id);
        }
        break;
      }

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

  // Unsubscribe handle for extension push events (set in actorCreated).
  #unsubscribeExtEvents = null;

  // Extension call from the page's window.__hostCall: dispatch to the
  // shared registry with the room id scoped to this product, then resolve
  // the page's pending promise with the raw JSON result (mirrors the
  // reference ProductView semantics: error envelopes resolve, not reject).
  async #handleExtensionCall(json) {
    let call;
    try {
      call = JSON.parse(json);
    } catch {
      return;
    }
    const { callId, channel, method } = call;
    if (
      typeof callId !== "number" ||
      typeof channel !== "string" ||
      typeof method !== "string"
    ) {
      return;
    }
    const productId = this.#productId();
    let params =
      call.params && typeof call.params === "object" ? call.params : {};
    if (typeof params.roomId === "string") {
      params = { ...params, roomId: `${productId}/${params.roomId}` };
    }
    let valueJson = null;
    try {
      valueJson = await lazy.EpocaHostEngine.extensionDispatch(
        channel,
        method,
        JSON.stringify(params)
      );
    } catch (e) {
      console.error("EpocaProduct: extension dispatch failed", e);
      try {
        this.sendAsyncMessage("EpocaProduct:ExtResolve", {
          callId,
          ok: false,
          valueJson: JSON.stringify(String(e?.message || e)),
        });
      } catch (sendErr) {
        // Actor torn down.
      }
      return;
    }
    if (valueJson !== null) {
      valueJson = EpocaProductParent.#payloadForProduct(valueJson, productId, {
        passWithoutRoom: true,
      });
    }
    try {
      this.sendAsyncMessage("EpocaProduct:ExtResolve", {
        callId,
        ok: true,
        valueJson,
      });
    } catch (e) {
      // Actor torn down.
    }
  }

  // Rewrite a JSON payload's "<productId>/<roomId>" back to the page-visible
  // room id. Returns the rewritten JSON, the payload unchanged when it has
  // no roomId (only if `passWithoutRoom`; events without a room would leak
  // across products otherwise), or null when the room belongs to another
  // product.
  static #payloadForProduct(payloadJson, productId, { passWithoutRoom } = {}) {
    let payload;
    try {
      payload = JSON.parse(payloadJson);
    } catch {
      return passWithoutRoom ? payloadJson : null;
    }
    if (!payload || typeof payload.roomId !== "string") {
      return passWithoutRoom ? payloadJson : null;
    }
    const prefix = `${productId}/`;
    if (!payload.roomId.startsWith(prefix)) {
      return null;
    }
    payload.roomId = payload.roomId.slice(prefix.length);
    return JSON.stringify(payload);
  }

  didDestroy() {
    this.#unsubscribeExtEvents?.();
    this.#unsubscribeExtEvents = null;
    for (const stop of this.#chainFollows.values()) {
      stop();
    }
    this.#chainFollows.clear();
    this.#followServerIds.clear();
    for (const entry of this.#preimageLookups.values()) {
      entry.cancelled = true;
    }
    this.#preimageLookups.clear();
    for (const stop of this.#statementSubs.values()) {
      stop();
    }
    this.#statementSubs.clear();
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

  // In-flight preimage lookups, keyed by request id, so an unsubscribe that
  // arrives mid-fetch can suppress delivery.
  #preimageLookups = new Map();

  // Active statement-store subscriptions: request id -> stop function.
  #statementSubs = new Map();

  // Statement-store subscription (Bulletin statements on the People Next
  // chain): translate the product's TopicFilter to statement_subscribeStatement
  // params, subscribe over the host's statement-store WebSocket, parse each
  // notification into signed statements, and stream them back. Fetching runs
  // in the parent; the product stays network-locked.
  async #handleStatementStoreSubscription(outcome) {
    // Kill switch (default on). Statements gossiped by the store are already
    // SCALE SignedStatement structs, so the notification bytes go straight to
    // encodeStatementStoreReceive; verified end to end against sk3chy.dot
    // (subscribe -> receive -> unsubscribe, no decode errors).
    if (!Services.prefs.getBoolPref("epoca.statement-store.deliver", true)) {
      return;
    }

    // A product using the statement store is the signal to provision the
    // host identity's allowance (register on-chain if needed, then claim).
    // Off by default: it registers a persistent on-chain identity. Fire and
    // forget; EpocaAllowance de-duplicates across subscriptions.
    if (Services.prefs.getBoolPref("epoca.registration.auto-provision", false)) {
      lazy.EpocaAllowance.ensure({ provision: true })
        .then(result =>
          console.debug("EpocaProduct: allowance ensure", result?.status)
        )
        .catch(e => console.error("EpocaProduct: allowance ensure failed", e));
    }

    const requestId = outcome.request_id;
    const abort = async () => {
      this.#statementSubs.delete(requestId);
      await this.#reply("encodeStatementStoreInterrupt", requestId);
    };
    try {
      const params = EpocaProductParent.#statementFilterToParams(outcome.filter);
      const stop = await lazy.EpocaChainService.subscribeStatements(
        params,
        async text => {
          const statements = EpocaProductParent.#parseStatementNotification(text);
          if (statements.length) {
            await this.#reply(
              "encodeStatementStoreReceive",
              requestId,
              statements,
              false
            );
          }
        },
        () => abort()
      );
      this.#statementSubs.set(requestId, stop);
    } catch (e) {
      console.error("EpocaProduct: statement-store subscribe failed", e);
      await abort();
    }
  }

  // TopicFilter -> statement_subscribeStatement params. Mirrors host-chain
  // statement_subscribe_params: Any -> ["any"], MatchAny(topics) ->
  // [{ matchAny: [hex...] }]. Defensive about the serialized enum shape.
  static #statementFilterToParams(filter) {
    const topics =
      filter?.MatchAny ?? filter?.matchAny ?? (Array.isArray(filter) ? filter : null);
    if (Array.isArray(topics) && topics.length) {
      const hexes = topics.map(t =>
        typeof t === "string" ? t : EpocaProductParent.#bytesToHex(t)
      );
      return [{ matchAny: hexes }];
    }
    return ["any"];
  }

  // Extract signed statements (as Uint8Array) from a statement_subscribeStatement
  // notification. Mirrors host-chain-core parse_statement_notification, which
  // reads params.result.{data,newStatements}.statements (hex strings).
  static #parseStatementNotification(text) {
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      return [];
    }
    // Live People Next notifies via method "statement_statement" with
    // result.event = "newStatements"; older nodes used
    // "statement_subscribeStatement". Accept both.
    if (
      parsed?.method !== "statement_statement" &&
      parsed?.method !== "statement_subscribeStatement"
    ) {
      return [];
    }
    const result = parsed.params?.result;
    const arr =
      result?.data?.statements ??
      result?.newStatements?.statements ??
      result?.statements;
    if (!Array.isArray(arr)) {
      return [];
    }
    return arr
      .filter(s => typeof s === "string")
      .map(s => EpocaProductParent.#hexToBytes(s));
  }

  static #hexToBytes(hex) {
    const raw = hex.startsWith("0x") ? hex.slice(2) : hex;
    const out = new Uint8Array(raw.length >> 1);
    for (let i = 0; i < out.length; i++) {
      out[i] = parseInt(raw.slice(i * 2, i * 2 + 2), 16);
    }
    return out;
  }

  static #bytesToHex(bytes) {
    return (
      "0x" +
      Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
    );
  }

  #isDarkTheme() {
    try {
      return !!this.browsingContext?.topChromeWindow?.matchMedia(
        "(prefers-color-scheme: dark)"
      ).matches;
    } catch {
      return false;
    }
  }

  // Preimage lookup (Bulletin content-addressed data, e.g. product icons):
  // the engine hands us a 32-byte hex key; the host resolves it to a CIDv1
  // (raw codec, blake2b-256 multihash) and fetches the body from the Bulletin
  // IPFS gateway. Fetching happens here in the parent, which is not subject to
  // the product network lockdown. Content is immutable, so the "subscription"
  // is effectively one-shot: deliver once, then the product unsubscribes.
  async #handlePreimageLookup(outcome) {
    const requestId = outcome.request_id;
    const entry = { cancelled: false };
    this.#preimageLookups.set(requestId, entry);
    try {
      const cid = EpocaProductParent.#preimageKeyToCid(outcome.key);
      const gateway = Services.prefs.getStringPref(
        "epoca.dotapp.preimage-gateway",
        ""
      );
      let value = null;
      if (cid && gateway) {
        const response = await fetch(`${gateway}/ipfs/${cid}`);
        if (response.ok) {
          value = new Uint8Array(await response.arrayBuffer());
        } else if (response.status !== 404) {
          console.warn(
            `EpocaProduct: preimage fetch ${cid} -> HTTP ${response.status}`
          );
        }
      }
      if (entry.cancelled) {
        return;
      }
      // encodePreimageLookupReceive takes Some(bytes) or None (key absent).
      await this.#reply(
        "encodePreimageLookupReceive",
        requestId,
        value ? Array.from(value) : null
      );
    } catch (e) {
      console.error(`EpocaProduct: preimage lookup failed`, e);
      if (!entry.cancelled) {
        await this.#reply("encodePreimageLookupInterrupt", requestId);
      }
    } finally {
      this.#preimageLookups.delete(requestId);
    }
  }

  // Mirror useragent-kit host-chain bulletin::preimage_key_to_cid: a 32-byte
  // key becomes CIDv1 / raw codec (0x55) / blake2b-256 multihash (0xb220,
  // varint 0xa0 0xe4 0x02) / 32-byte digest, base32-lower ('b' multibase).
  static #preimageKeyToCid(key) {
    const raw = key.startsWith("0x") ? key.slice(2) : key;
    if (raw.length !== 64 || /[^0-9a-fA-F]/.test(raw)) {
      return null;
    }
    const digest = new Uint8Array(32);
    for (let i = 0; i < 32; i++) {
      digest[i] = parseInt(raw.slice(i * 2, i * 2 + 2), 16);
    }
    // CIDv1(0x01) raw(0x55) blake2b-256 multihash(0xa0 0xe4 0x02) len(0x20)
    // = 6-byte prefix, then the 32-byte digest.
    const cid = new Uint8Array(38);
    cid.set([0x01, 0x55, 0xa0, 0xe4, 0x02, 0x20], 0);
    cid.set(digest, 6);
    return "b" + EpocaProductParent.#base32LowerNoPad(cid);
  }

  static #base32LowerNoPad(bytes) {
    const alphabet = "abcdefghijklmnopqrstuvwxyz234567";
    let out = "";
    let buffer = 0;
    let bits = 0;
    for (const byte of bytes) {
      buffer = (buffer << 8) | byte;
      bits += 8;
      while (bits >= 5) {
        bits -= 5;
        out += alphabet[(buffer >> bits) & 0x1f];
      }
    }
    if (bits > 0) {
      out += alphabet[(buffer << (5 - bits)) & 0x1f];
    }
    return out;
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
