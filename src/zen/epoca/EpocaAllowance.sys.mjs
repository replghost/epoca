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
  EpocaRegistration: "resource:///modules/EpocaRegistration.sys.mjs",
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
// After registration, how long to wait for the freshly-attested member to land
// in a committed ring (ring commitment is async, chain-driven).
const RING_WAIT_ATTEMPTS = 20;
const RING_WAIT_DELAY_MS = 15_000;

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
   * @param {object} [options]
   * @param {boolean} [options.provision] - if the identity is not yet a
   *   committed ring member, register it via DotSpark and wait for ring
   *   commitment before claiming.
   * @returns {Promise<object>} one of:
   *   {status:"already-granted"}
   *   {status:"submitted", txHash, confirmed, registered?, username?}
   *   {status:"not-ring-included", scanned:{lo,current}}
   *   {status:"registered-awaiting-ring-commit", username, account}
   */
  ensure(options = {}) {
    if (!this._inFlight) {
      this._inFlight = this._claim(options).finally(() => {
        this._inFlight = null;
      });
    }
    return this._inFlight;
  },

  async _claim({ provision = false } = {}) {
    const glue = await lazy.EpocaHostEngine.glue();
    const accountId = await lazy.EpocaWallet.walletPublicKey();
    const accountHex = bytesToHex(accountId);

    console.info(`[epoca:allowance] claim start account=${accountHex}`);

    // Idempotency: skip if the account already has an allowance entry.
    if (await this._allowanceExists(glue, accountHex)) {
      console.info("[epoca:allowance] already granted");
      return { status: "already-granted" };
    }

    const memberKey = await lazy.EpocaWallet.ringVrfMemberKey();
    await this._diagnoseRings(glue, memberKey);
    let ring = await this._findCommittedRing(glue, memberKey);
    console.info(
      `[epoca:allowance] committed ring lookup: ${
        ring ? `found index ${ring.index}` : "none"
      } (provision=${provision})`
    );

    let registeredUsername = null;
    if (!ring && provision) {
      // Self-provision: register the identity, then wait for it to land in a
      // committed ring (async on the chain side) before claiming.
      const reg = await lazy.EpocaRegistration.register();
      registeredUsername = reg.username;
      console.info(
        "[epoca:allowance] registered; waiting for ring commitment " +
          "(chain-driven, minutes)…"
      );
      ring = await this._waitForCommittedRing(glue, memberKey);
      if (!ring) {
        console.info(
          "[epoca:allowance] status=registered-awaiting-ring-commit " +
            `username=${registeredUsername}`
        );
        return {
          status: "registered-awaiting-ring-commit",
          username: registeredUsername,
          account: accountHex,
        };
      }
      console.info(`[epoca:allowance] ring committed at index ${ring.index}`);
    }

    if (!ring) {
      const current = await this._currentRingIndex(glue);
      console.info(
        `[epoca:allowance] status=not-ring-included (current ring ${current})`
      );
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
    console.info(
      `[epoca:allowance] claim extrinsic submitted tx=${txHash}; confirming…`
    );

    let confirmed = false;
    for (let i = 0; i < CONFIRM_ATTEMPTS; i++) {
      await delay(CONFIRM_DELAY_MS);
      if (await this._allowanceExists(glue, accountHex).catch(() => false)) {
        confirmed = true;
        break;
      }
    }
    console.info(
      `[epoca:allowance] status=submitted tx=${txHash} confirmed=${confirmed}`
    );
    return {
      status: "submitted",
      txHash,
      confirmed,
      ...(registeredUsername
        ? { registered: true, username: registeredUsername }
        : {}),
    };
  },

  /**
   * Poll for the member key to appear in a committed ring, after registration.
   *
   * @returns {Promise<{index:number, members:string[]}|null>}
   */
  async _waitForCommittedRing(glue, memberKey) {
    for (let i = 0; i < RING_WAIT_ATTEMPTS; i++) {
      const ring = await this._findCommittedRing(glue, memberKey);
      if (ring) {
        return ring;
      }
      await delay(RING_WAIT_DELAY_MS);
    }
    return null;
  },

  /**
   * Diagnostic: scan a wide window of ring indices (well past the claim
   * window) and report whether our member key appears in ANY ring — as a
   * committed member (pos < included), a pending building-ring member
   * (pos >= included, attestation landed but not yet committed), or nowhere
   * (attestation never made it on-chain). Read-only; logs its finding.
   */
  async _diagnoseRings(glue, memberKey) {
    try {
      const current = await this._currentRingIndex(glue);
      const lo = Math.max(0, current - 24);
      console.info(`[epoca:allowance:diag] currentRingIndex=${current}`);
      let populated = 0;
      let totalMembers = 0;
      for (let ri = current; ri >= lo; ri--) {
        const page0 = await storageGet(glue.allowanceRingKeysKey(ri, 0));
        if (!page0) {
          continue;
        }
        const statusValue = await storageGet(glue.allowanceRingStatusKey(ri));
        const included = statusValue
          ? glue.allowanceDecodeRingIncluded(statusValue)
          : 0;
        const members = await this._readAllRingPages(glue, ri, page0);
        populated++;
        totalMembers += members.length;
        // Per-ring population: are OTHER accounts landing (commitment alive)?
        console.info(
          `[epoca:allowance:diag] ring=${ri} members=${members.length} included=${included}`
        );
        const pos = members.findIndex(
          m => m.toLowerCase() === memberKey.toLowerCase()
        );
        if (pos >= 0) {
          const committed = pos < included;
          console.info(
            `[epoca:allowance:diag] member=${memberKey.slice(0, 14)}… ` +
              `FOUND ring=${ri} pos=${pos}/${members.length} included=${included} ` +
              `${committed ? "COMMITTED" : "BUILDING (attested, not yet committed)"} ` +
              `(currentRing=${current})`
          );
          return;
        }
      }
      console.info(
        `[epoca:allowance:diag] member=${memberKey.slice(0, 14)}… NOT in any ` +
          `ring [${lo}..${current}] — populatedRings=${populated} ` +
          `totalMembers=${totalMembers} (rings growing with OTHERS but not us => ` +
          `bravo not landing our attestation; empty rings => commitment stalled)`
      );
    } catch (e) {
      console.warn("[epoca:allowance:diag] scan failed", e);
    }
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
