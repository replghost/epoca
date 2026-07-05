// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Self-service statement-store allowance claim (Paseo People Next v2).
//
// A lite person proves ring membership with a Bandersnatch ring-VRF and
// submits an origin-less *general* transaction calling
// `Resources::set_statement_store_account`, authorized by the `AsResources`
// transaction extension; the runtime grants the wallet's `//wallet` identity a
// statement-store allowance. The byte layout lives in the vendored wasm
// (`useragent_encoding::allowance`, shared with the native path); this module
// owns only the browser-side RPC: idempotency check, ring discovery,
// submission, and confirmation. It mirrors host-chain's
// `claim_statement_store_allowance` (verified live against spec 1_000_020 / tx 3).

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaChainService: "resource:///modules/EpocaChainService.sys.mjs",
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
  EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
});

ChromeUtils.defineLazyGetter(lazy, "setTimeout", () => {
  return ChromeUtils.importESModule("resource://gre/modules/Timer.sys.mjs")
    .setTimeout;
});

// How many recent ring indices to scan for a committed ring containing us.
const RING_SCAN_WINDOW = 6;
// Confirmation poll attempts after submission, and the delay between them (ms).
const CONFIRM_ATTEMPTS = 10;
const CONFIRM_DELAY_MS = 6_000;

function bytesToHex(bytes) {
  return (
    "0x" + Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
  );
}

function rpcResult(rawText, method) {
  const body = JSON.parse(rawText);
  if (body.error) {
    throw new Error(`${method} rejected: ${JSON.stringify(body.error)}`);
  }
  return body.result;
}

async function storageGet(keyHex) {
  const raw = await lazy.EpocaChainService.ssRpc("state_getStorage", [keyHex]);
  const result = rpcResult(raw, "state_getStorage");
  return typeof result === "string" ? result : null;
}

async function stateGetKeysPaged(prefixHex, count) {
  const raw = await lazy.EpocaChainService.ssRpc("state_getKeysPaged", [
    prefixHex,
    count,
    null,
  ]);
  const result = rpcResult(raw, "state_getKeysPaged");
  return Array.isArray(result) ? result : [];
}

function delay(ms) {
  return new Promise(resolve => lazy.setTimeout(resolve, ms));
}

export const EpocaAllowance = {
  _inFlight: null,

  /**
   * Ensure the wallet's `//wallet` identity holds a statement-store allowance,
   * claiming one via a ring-VRF proof if it doesn't. Idempotent and
   * de-duplicated across concurrent callers.
   *
   * @returns {Promise<object>} one of:
   *   {status:"already-granted"}
   *   {status:"submitted", txHash, confirmed}
   *   {status:"not-ring-included", scanned:{lo,current}}
   */
  ensure() {
    if (!this._inFlight) {
      this._inFlight = this._claim().finally(() => {
        this._inFlight = null;
      });
    }
    return this._inFlight;
  },

  async _claim() {
    const glue = await lazy.EpocaHostEngine.glue();
    const accountId = await lazy.EpocaWallet.walletPublicKey();
    const accountHex = bytesToHex(accountId);

    // Idempotency: skip if the account already has an allowance entry.
    if (await this._allowanceExists(glue, accountHex)) {
      return { status: "already-granted" };
    }

    const memberKey = await lazy.EpocaWallet.ringVrfMemberKey();
    const ring = await this._findCommittedRing(glue, memberKey);
    if (!ring) {
      const current = await this._currentRingIndex(glue);
      return {
        status: "not-ring-included",
        scanned: { lo: Math.max(0, current - RING_SCAN_WINDOW), current },
      };
    }

    const { specVersion, txVersion } = await this._runtimeVersion();
    const genesisHex = await this._genesisHash();
    const period = await this._currentPeriod(glue);
    // Slot 0 is sufficient: the alias is unique per (member, period, seq), so
    // this member's seq-0 slot is exclusively theirs and a re-claim within the
    // same period reuses it (the runtime's replacement-cooldown path).
    const seq = 0;

    const extrinsicHex = await lazy.EpocaWallet.buildAllowanceClaimExtrinsic(
      ring.members,
      ring.index,
      period,
      seq,
      specVersion,
      txVersion,
      genesisHex
    );

    const submitRaw = await lazy.EpocaChainService.ssRpc(
      "author_submitExtrinsic",
      [extrinsicHex]
    );
    const txHash = rpcResult(submitRaw, "author_submitExtrinsic");

    let confirmed = false;
    for (let i = 0; i < CONFIRM_ATTEMPTS; i++) {
      await delay(CONFIRM_DELAY_MS);
      if (await this._allowanceExists(glue, accountHex).catch(() => false)) {
        confirmed = true;
        break;
      }
    }
    return { status: "submitted", txHash, confirmed };
  },

  /** True if any `StmtStoreAllowanceByAccount` entry exists for the account. */
  async _allowanceExists(glue, accountHex) {
    const prefix = glue.allowanceByAccountPrefix(accountHex);
    const keys = await stateGetKeysPaged(prefix, 1);
    return keys.length > 0;
  },

  async _currentRingIndex(glue) {
    const value = await storageGet(glue.allowanceCurrentRingIndexKey());
    return value ? glue.allowanceDecodeRingIndex(value) : 0;
  },

  /**
   * Scan recent ring indices (newest first) for the highest committed ring
   * whose `included` member prefix contains our member key. A freshly-attested
   * member is pending in the still-building current ring (no committed root
   * yet), so the provable ring is often a prior index.
   *
   * @returns {Promise<{index:number, members:string[]}|null>}
   */
  async _findCommittedRing(glue, memberKey) {
    const current = await this._currentRingIndex(glue);
    const lo = Math.max(0, current - RING_SCAN_WINDOW);

    for (let ri = current; ri >= lo; ri--) {
      const page0 = await storageGet(glue.allowanceRingKeysKey(ri, 0));
      if (!page0) {
        continue;
      }
      const hasRoot = (await storageGet(glue.allowanceRingRootKey(ri))) !== null;
      if (!hasRoot) {
        continue;
      }
      const statusValue = await storageGet(glue.allowanceRingStatusKey(ri));
      const included = statusValue
        ? glue.allowanceDecodeRingIncluded(statusValue)
        : 0;

      const members = await this._readAllRingPages(glue, ri, page0);
      const pos = members.findIndex(
        m => m.toLowerCase() === memberKey.toLowerCase()
      );
      if (pos < 0) {
        continue;
      }
      const ringLen = Math.min(included, members.length);
      if (pos < ringLen) {
        return { index: ri, members: members.slice(0, ringLen) };
      }
    }
    return null;
  },

  /**
   * Decode page 0 (already fetched), then append subsequent pages until one is
   * absent. LitePeople is normally single-page; extra reads are defensive.
   */
  async _readAllRingPages(glue, ringIndex, page0Hex) {
    const members = glue.allowanceDecodeRingMembers(page0Hex);
    let page = 1;
    for (;;) {
      const value = await storageGet(glue.allowanceRingKeysKey(ringIndex, page));
      if (!value) {
        break;
      }
      const more = glue.allowanceDecodeRingMembers(value);
      if (!more.length) {
        break;
      }
      members.push(...more);
      page++;
    }
    return members;
  },

  async _currentPeriod(glue) {
    // The runtime validates period against its own clock with no grace window.
    const value = await storageGet(glue.allowanceTimestampNowKey());
    if (!value) {
      throw new Error("Timestamp::Now storage empty");
    }
    return glue.allowancePeriodFromTimestamp(value);
  },

  async _runtimeVersion() {
    const raw = await lazy.EpocaChainService.ssRpc(
      "state_getRuntimeVersion",
      []
    );
    const result = rpcResult(raw, "state_getRuntimeVersion");
    return {
      specVersion: result.specVersion,
      txVersion: result.transactionVersion,
    };
  },

  async _genesisHash() {
    const raw = await lazy.EpocaChainService.ssRpc("chain_getBlockHash", [0]);
    return rpcResult(raw, "chain_getBlockHash");
  },
};
