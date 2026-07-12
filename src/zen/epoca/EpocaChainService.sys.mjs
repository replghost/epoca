// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process JSON-RPC transport to the chains the host supports,
// backing the engine's NeedsChain* outcomes. One WebSocket per genesis,
// opened lazily and reopened on the next request after a drop. Message
// strings are passed through raw in both directions — the engine parses
// and re-encodes them (mirrors useragent-kit's runtime-chain-service).

const NETWORKS_PREF = "epoca.chain.networks";
const ENVIRONMENT_PREF = "epoca.chain.environment";
const DEFAULT_ENVIRONMENT = "paseo-next-v2";
const ENVIRONMENT_URL = name =>
  `resource:///modules/epoca/environments/${name}.json`;

// Read a resource: URL as text. Mirrors EpocaHostEngine.readBinaryResource:
// prefer fetch, fall back to resolving the resource to a file (unpackaged
// local builds where fetch is unavailable in the system global).
async function readTextResource(url) {
  if (typeof fetch === "function") {
    const response = await fetch(url);
    return response.text();
  }
  const resHandler = Services.io
    .getProtocolHandler("resource")
    .QueryInterface(Ci.nsIResProtocolHandler);
  const fileUrl = resHandler.resolveURI(Services.io.newURI(url));
  const path = Services.io
    .newURI(fileUrl)
    .QueryInterface(Ci.nsIFileURL).file.path;
  return new TextDecoder().decode(await IOUtils.read(path));
}

const STOP_METHODS = new Map([
  ["chainHead_v1_follow", "chainHead_v1_unfollow"],
  ["statement_subscribeStatement", "statement_unsubscribeStatement"],
]);

function hexToBytes(hex) {
  return Uint8Array.from(
    hex.replace(/^0x/, "")
      .match(/../g)
      .map(b => parseInt(b, 16))
  );
}

function bytesToHex(bytes) {
  return (
    "0x" + Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
  );
}

class ChainConnection {
  #url;
  #socket = null;
  #openPromise = null;
  #nextId = 1;
  // id -> {resolve, reject}
  #pendingRequests = new Map();
  // id -> subscription state (awaiting the ack that carries the sub id)
  #pendingSubscriptions = new Map();
  // server subscription id -> subscription state
  #activeSubscriptions = new Map();

  constructor(url) {
    this.#url = url;
  }

  // Open the socket ahead of any request so the first real chain call
  // doesn't pay TCP+TLS+WS connect latency. Fire-and-forget.
  prewarm() {
    this.#ensureOpen().catch(() => {});
  }

  #ensureOpen() {
    if (this.#socket?.readyState === WebSocket.OPEN) {
      return Promise.resolve();
    }
    if (!this.#openPromise) {
      this.#openPromise = new Promise((resolve, reject) => {
        const ws = new WebSocket(this.#url);
        ws.onopen = () => resolve();
        ws.onmessage = event => this.#onMessage(event.data);
        ws.onerror = () => reject(new Error(`websocket error: ${this.#url}`));
        ws.onclose = () => this.#onClose();
        this.#socket = ws;
      }).finally(() => {
        this.#openPromise = null;
      });
    }
    return this.#openPromise;
  }

  #onClose() {
    this.#socket = null;
    const aborted = [
      ...this.#pendingSubscriptions.values(),
      ...this.#activeSubscriptions.values(),
    ];
    if (this.#pendingRequests.size || aborted.length) {
      console.warn(
        `EpocaChain: connection closed url=${this.#url} ` +
          `pendingReqs=${this.#pendingRequests.size} abortedSubs=${aborted.length}`
      );
    }
    for (const { reject } of this.#pendingRequests.values()) {
      reject(new Error("chain connection closed"));
    }
    this.#pendingRequests.clear();
    this.#pendingSubscriptions.clear();
    this.#activeSubscriptions.clear();
    for (const sub of aborted) {
      try {
        sub.onAbort("chain connection closed");
      } catch (e) {
        console.error("EpocaChain: onAbort failed", e);
      }
    }
  }

  #onMessage(text) {
    if (Services.prefs.getBoolPref("epoca.chain.log", false)) {
      console.debug(`EpocaChain << ${text.slice(0, 300)}`);
    }
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      return;
    }

    if (parsed.id !== undefined && parsed.id !== null) {
      const pending = this.#pendingRequests.get(parsed.id);
      if (pending) {
        this.#pendingRequests.delete(parsed.id);
        pending.resolve(text);
        return;
      }
      const sub = this.#pendingSubscriptions.get(parsed.id);
      if (sub) {
        this.#pendingSubscriptions.delete(parsed.id);
        if (typeof parsed.result === "string") {
          if (sub.cancelled) {
            this.#sendStop(sub.stopMethod, parsed.result);
          } else {
            this.#activeSubscriptions.set(parsed.result, sub);
          }
        } else {
          // No subscription id in the ack — the server rejected the
          // subscribe (e.g. method unsupported / not permitted). Surface it;
          // the sub is now neither pending nor active.
          console.warn(
            `EpocaChain: subscription ack has no id (${sub.stopMethod}): ` +
              text.slice(0, 300)
          );
        }
        if (!sub.cancelled) {
          sub.onMessage(text);
        }
        return;
      }
      return;
    }

    const subId = parsed.params?.subscription;
    if (subId !== undefined) {
      const sub = this.#activeSubscriptions.get(subId);
      if (sub) {
        sub.onMessage(text);
      }
    }
  }

  #send(method, params) {
    const id = this.#nextId++;
    const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params });
    if (Services.prefs.getBoolPref("epoca.chain.log", false)) {
      console.debug(`EpocaChain >> ${payload.slice(0, 300)}`);
    }
    this.#socket.send(payload);
    return id;
  }

  #sendStop(stopMethod, serverSubId) {
    try {
      this.#send(stopMethod, [serverSubId]);
    } catch (e) {
      console.error("EpocaChain: stop request failed", e);
    }
    this.#activeSubscriptions.delete(serverSubId);
  }

  async sendRpc(method, params) {
    await this.#ensureOpen();
    return new Promise((resolve, reject) => {
      const id = this.#send(method, params);
      this.#pendingRequests.set(id, { resolve, reject });
    });
  }

  async startSubscription(method, params, onMessage, onAbort) {
    await this.#ensureOpen();
    const stopMethod = STOP_METHODS.get(method) ?? `${method}_unsubscribe`;
    const sub = { onMessage, onAbort, stopMethod, cancelled: false };
    const id = this.#send(method, params);
    this.#pendingSubscriptions.set(id, sub);
    return () => {
      sub.cancelled = true;
      for (const [serverSubId, active] of this.#activeSubscriptions) {
        if (active === sub) {
          this.#sendStop(stopMethod, serverSubId);
        }
      }
    };
  }
}

export const EpocaChainService = {
  // genesis hex -> ChainConnection
  _connections: new Map(),

  // Chains + statement-store endpoint derived from the vendored useragent-kit
  // environment bundle (the canonical network registry). Populated by
  // ensureEnvironment(); the epoca.chain.networks pref merges on top as an
  // override. Null until the bundle has loaded.
  _bundleNetworks: null,
  _bundleSsEndpoint: null,
  _envPromise: null,

  /**
   * Load the environment bundle (once) and derive the supported chains +
   * statement-store endpoint from it. The bundle is the canonical source of
   * genesis→RPC mappings (see useragent-kit host-chain-core/environments);
   * epoca no longer hand-maintains the full chain list.
   */
  ensureEnvironment() {
    if (!this._envPromise) {
      this._envPromise = (async () => {
        const name = Services.prefs.getStringPref(
          ENVIRONMENT_PREF,
          DEFAULT_ENVIRONMENT
        );
        try {
          const bundle = JSON.parse(
            await readTextResource(ENVIRONMENT_URL(name))
          );
          const nets = {};
          for (const chain of Object.values(bundle.chains || {})) {
            const urls = chain.rpc_urls || [];
            if (chain.genesis_hash && urls.length) {
              const hex =
                "0x" + chain.genesis_hash.replace(/^0x/, "").toLowerCase();
              nets[hex] = urls;
            }
          }
          this._bundleNetworks = nets;
          this._bundleSsEndpoint =
            bundle.services?.statement_store?.endpoints?.[0] ?? null;
          console.debug(
            `EpocaChain: environment '${name}' → ${
              Object.keys(nets).length
            } chain(s)`
          );
        } catch (e) {
          console.error("EpocaChain: failed to load environment bundle", e);
          this._bundleNetworks = {};
        }
      })();
    }
    return this._envPromise;
  },

  _prefNetworks() {
    try {
      return JSON.parse(Services.prefs.getStringPref(NETWORKS_PREF, "{}"));
    } catch {
      return {};
    }
  },

  // Bundle chains (canonical base) with the pref merged on top as an override.
  _networks() {
    return { ...(this._bundleNetworks || {}), ...this._prefNetworks() };
  },

  /** @returns {Uint8Array[]} genesis hashes of all configured networks. */
  supportedGenesisHashes() {
    return Object.keys(this._networks()).map(hexToBytes);
  },

  /**
   * Map a genesis hash (bytes or number[]) to its configured hex key.
   *
   * @returns {string|null}
   */
  routeByGenesis(genesisHash) {
    const hex = bytesToHex(genesisHash).toLowerCase();
    return Object.keys(this._networks()).find(k => k.toLowerCase() === hex)
      ? hex
      : null;
  },

  /** The first configured genesis, for legacy single-chain queries. */
  defaultGenesis() {
    return Object.keys(this._networks())[0] ?? null;
  },

  _connection(genesisHex) {
    let connection = this._connections.get(genesisHex);
    if (!connection) {
      const urls = this._networks()[genesisHex];
      if (!urls?.length) {
        throw new Error(`no RPC endpoint configured for ${genesisHex}`);
      }
      connection = new ChainConnection(urls[0]);
      this._connections.set(genesisHex, connection);
    }
    return connection;
  },

  /**
   * Open the connection for a genesis (default: the first configured chain)
   * without issuing a request, so a product's first chain call is fast.
   */
  prewarm(genesisHex = this.defaultGenesis()) {
    if (!genesisHex) {
      return;
    }
    try {
      this._connection(genesisHex).prewarm();
    } catch (e) {
      console.warn("EpocaChain: prewarm failed", e);
    }
  },

  /**
   * Send a JSON-RPC request; resolves with the raw response message text.
   */
  sendRpc(genesisHex, method, params) {
    return this._connection(genesisHex).sendRpc(method, params);
  },

  /**
   * Start a JSON-RPC subscription. onMessage receives every raw message
   * (the ack and each notification); onAbort fires if the connection drops.
   *
   * @returns {Promise<() => void>} stop function.
   */
  startSubscription(genesisHex, method, params, onMessage, onAbort) {
    return this._connection(genesisHex).startSubscription(
      method,
      params,
      onMessage,
      onAbort
    );
  },

  // Dedicated connection to the statement-store chain (People Next), which is
  // a distinct endpoint from the product's own chains.
  _ssConnection: null,

  _ssConn() {
    const url =
      Services.prefs.getStringPref(
        "epoca.chain.statement-store-endpoint",
        ""
      ) || this._bundleSsEndpoint;
    if (!url) {
      throw new Error("no statement-store endpoint configured");
    }
    if (!this._ssConnection) {
      this._ssConnection = new ChainConnection(url);
    }
    return this._ssConnection;
  },

  /**
   * Send a JSON-RPC request to the statement-store chain (People Next);
   * resolves with the raw response message text. Used by the allowance-claim
   * flow for storage reads and extrinsic submission.
   */
  ssRpc(method, params) {
    return this._ssConn().sendRpc(method, params);
  },

  /**
   * Subscribe to statement-store statements matching a filter. onMessage
   * receives every raw JSON-RPC message (ack + notifications); the caller
   * parses statements out of the notifications.
   *
   * @param {Array} params - statement_subscribeStatement params (filter).
   * @returns {Promise<() => void>} stop function.
   */
  subscribeStatements(params, onMessage, onAbort) {
    let conn;
    try {
      conn = this._ssConn();
    } catch (e) {
      return Promise.reject(e);
    }
    return conn.startSubscription(
      "statement_subscribeStatement",
      params,
      onMessage,
      onAbort
    );
  },
};
