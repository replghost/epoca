// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Statement-store I/O driver for the CRDT extension's RelayCrdtRuntime.
//
// The runtime (in EpocaHostEngine's extension registry) is sans-IO: it queues
// outbound statements and expects inbound ones, but performs no network itself.
// This module supplies that I/O over the People Next statement store:
//   - reconcile live subscriptions to `relay.desiredTopics()`, and fetch each
//     new topic's existing statements once (late-join backfill);
//   - `relay.drainOutbound()` -> sign (host account) + submit each statement;
//   - inbound statement notifications -> `relay.ingestStatement()`.
// After ingest/dispatch, EpocaHostEngine fans the runtime's drained CRDT events
// to product tabs. Loop-safety is the runtime's: `ingest` never re-enqueues
// outbound, own-sender/foreign-protocol envelopes are dropped, and Yjs is
// idempotent. Inbound statements go only to `ingestStatement`; only
// `drainOutbound` output is submitted — the two pipes never cross.
//
// Submitting requires the host `//wallet` account to hold a statement-store
// allowance (see EpocaAllowance); until it does, outbound is left queued in the
// runtime rather than drained-and-dropped, and provisioning is kicked off.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaAllowance: "resource:///modules/EpocaAllowance.sys.mjs",
  EpocaChainService: "resource:///modules/EpocaChainService.sys.mjs",
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
  EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
});

ChromeUtils.defineLazyGetter(lazy, "setInterval", () => {
  return ChromeUtils.importESModule("resource://gre/modules/Timer.sys.mjs")
    .setInterval;
});

// A slow tick drains outbound queued while the host account was still being
// provisioned, and re-subscribes after a dropped connection, even when the
// product goes idle (no dispatches to drive pump()).
const TICK_MS = 4_000;

// Matches host-chain's v2 priority base (statement priority is a seconds-since
// this-epoch timestamp; newer statements win LWW retention).
const UNIX_OFFSET = 1_763_164_800;

function hexToBytes(hex) {
  const h = hex.replace(/^0x/, "");
  const out = new Uint8Array(h.length / 2);
  for (let i = 0; i < out.length; i++) {
    out[i] = parseInt(h.substr(i * 2, 2), 16);
  }
  return out;
}

function bytesToHex(bytes) {
  return (
    "0x" + Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
  );
}

function base64ToBytes(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
}

function rpcResult(rawText) {
  const body = JSON.parse(rawText);
  if (body.error) {
    throw new Error(`statement rpc: ${JSON.stringify(body.error)}`);
  }
  return body.result;
}

export const EpocaCrdtRelay = {
  _stmt: null, // StatementHandle (core wasm)
  _chain: null, // ChainClientHandle (core wasm)
  _subscribedKey: "", // sorted-topics key of the current subscription
  _stopSub: null, // stop fn for the current statement subscription
  _pumping: false,
  _allowancePromise: null,
  _allowanceReady: false,
  _ticking: false,

  // Begin periodic pumping. Idempotent; called once the relay runtime exists.
  start() {
    if (this._ticking) {
      return;
    }
    this._ticking = true;
    lazy.setInterval(() => this.pump(), TICK_MS);
  },

  async _ensureHandles() {
    if (!this._stmt) {
      const glue = await lazy.EpocaHostEngine.glue();
      this._stmt = new glue.StatementHandle();
      this._chain = new glue.ChainClientHandle();
    }
  },

  /**
   * Drive one relay cycle: reconcile subscriptions + backfill, then (once the
   * host account can publish) sign and submit queued outbound statements.
   * Safe to call after every dispatch and on a periodic tick; reentrancy-guarded.
   */
  async pump() {
    const relay = lazy.EpocaHostEngine._relay;
    if (!relay || this._pumping) {
      return;
    }
    this._pumping = true;
    try {
      await this._ensureHandles();
      await this._reconcile(relay);
      await this._drainOutbound(relay);
    } catch (e) {
      console.error("EpocaCrdtRelay: pump failed", e);
    } finally {
      this._pumping = false;
    }
  },

  // Keep the live subscription matching the runtime's desired topic set. The
  // People Next node has no fetch-by-topic RPC (only submit + subscribe), but
  // statement_subscribeStatement delivers the store's existing matching
  // statements on subscribe as well as new ones, so late joiners converge from
  // the retained snapshots + recent updates without a separate backfill.
  async _reconcile(relay) {
    let desired;
    try {
      desired = JSON.parse(relay.desiredTopics());
    } catch {
      return;
    }
    const topics = desired.map(d => d.topicHex);
    const key = [...topics].sort().join(",");
    if (key === this._subscribedKey) {
      return;
    }

    // (Re)subscribe to the union of desired topics.
    this._stopSub?.();
    this._stopSub = null;
    this._subscribedKey = key;
    if (!topics.length) {
      return;
    }
    this._stopSub = await lazy.EpocaChainService.subscribeStatements(
      [{ matchAny: topics }],
      text => this._onNotification(text),
      () => {
        // Connection dropped; force a resubscribe on the next pump.
        this._subscribedKey = "";
      }
    );
  },

  _onNotification(text) {
    let parsed;
    try {
      parsed = JSON.parse(text);
    } catch {
      return;
    }
    const result = parsed?.params?.result;
    const statements =
      result?.data?.statements ??
      result?.newStatements?.statements ??
      result?.statements;
    if (!Array.isArray(statements)) {
      return;
    }
    for (const stmtHex of statements) {
      try {
        this._ingestStatement(hexToBytes(stmtHex));
      } catch (e) {
        console.error("EpocaCrdtRelay: ingest failed", e);
      }
    }
    this._fanEvents();
  },

  // Decode a SignedStatement and feed its data to the runtime under each of its
  // topics. The runtime drops unknown topics, own-sender echoes, and foreign
  // protocols, so this never loops.
  _ingestStatement(statementBytes) {
    const relay = lazy.EpocaHostEngine._relay;
    if (!relay) {
      return;
    }
    const decoded = this._stmt.decodeStatement(statementBytes);
    const data = decoded?.data;
    const topics = decoded?.topics;
    if (!data || !Array.isArray(topics)) {
      return;
    }
    const dataBytes = data instanceof Uint8Array ? data : new Uint8Array(data);
    for (const topic of topics) {
      const topicBytes = topic instanceof Uint8Array ? topic : new Uint8Array(topic);
      relay.ingestStatement(bytesToHex(topicBytes), dataBytes);
    }
  },

  _fanEvents() {
    // The runtime queued CRDT events (crdtRemoteUpdate/awareness/peer) as a
    // result of ingest; hand them to EpocaHostEngine to fan to product tabs.
    lazy.EpocaHostEngine.fanExtensionEvents();
  },

  async _drainOutbound(relay) {
    // drainOutbound() REMOVES statements from the runtime queue, so only drain
    // once the host account can actually publish — otherwise the updates are
    // lost. Until then leave them queued (the runtime bounds the queue via
    // snapshot compaction) and kick off provisioning.
    if (!this._allowanceReady) {
      this._ensureAllowance();
      return;
    }
    let outbound;
    try {
      outbound = JSON.parse(relay.drainOutbound());
    } catch {
      return;
    }
    for (const o of outbound) {
      await this._submit(o).catch(e =>
        console.error("EpocaCrdtRelay: submit failed", e)
      );
    }
  },

  _ensureAllowance() {
    if (this._allowancePromise) {
      return this._allowancePromise;
    }
    this._allowancePromise = lazy.EpocaAllowance.ensure({ provision: true })
      .then(result => {
        this._allowanceReady =
          result?.status === "already-granted" ||
          result?.status === "submitted";
        return result;
      })
      .catch(e => {
        console.error("EpocaCrdtRelay: allowance ensure failed", e);
        this._allowancePromise = null;
      });
    return this._allowancePromise;
  },

  async _submit(o) {
    const nowSecs = Math.floor(Date.now() / 1000);
    const priority = Math.max(0, nowSecs - UNIX_OFFSET);
    const topicBytes = hexToBytes(o.topicHex);
    const channelBytes = o.channelHex ? hexToBytes(o.channelHex) : null;
    const dataBytes = base64ToBytes(o.dataBase64);

    const payloadWithHeader = this._stmt.buildSigningPayload(
      nowSecs,
      null,
      channelBytes,
      priority,
      topicBytes,
      dataBytes
    );
    const pubkey = await lazy.EpocaWallet.walletPublicKey();
    const sig = await lazy.EpocaWallet.signWallet(payloadWithHeader.slice(4));
    const signed = this._stmt.assembleStatement(payloadWithHeader, pubkey, sig);

    const req = JSON.parse(
      this._chain.statementSubmitRequest(bytesToHex(signed), 1)
    );
    const raw = await lazy.EpocaChainService.ssRpc(req.method, req.params);
    rpcResult(raw); // throws on error
  },
};
