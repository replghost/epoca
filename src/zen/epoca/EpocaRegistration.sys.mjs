// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// DotSpark backend lite-person username registration (Paseo People Next v2).
//
// On v2 the People chain only accepts `PeopleLite.attest` extrinsics signed by
// a backend-provisioned verifier ("attester"), whose secret lives server-side.
// The wallet builds a signed candidate payload locally (never exposing key
// material), proves ownership of its `//wallet` identity via a challenge-
// response handshake, and the backend submits the attestation on its behalf.
// Once it lands on-chain the identity becomes a committed ring member, which is
// the precondition for the statement-store allowance claim (see EpocaAllowance).
//
// This is a parent-process port of host-chain-core's `dotspark` module and
// host-mobile's `register_lite_username_via_backend`. Flow:
//   1. GET  /api/v1/attester                      -> verifier SS58
//   2. build candidate payload (wasm), bound to that verifier
//   3. POST /api/v1/auth/challenge {public_key}   -> nonce
//      sign the ASCII bytes of the hex nonce with the //wallet sr25519 key
//   4. POST /api/v1/auth/verify {public_key,sig}  -> bearer token
//   5. POST /api/v1/attestations (Bearer) + payload
//   6. poll GET /api/v1/usernames/{fullUsername} until assigned

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaSs58: "resource:///modules/EpocaSs58.sys.mjs",
  EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
});

ChromeUtils.defineLazyGetter(lazy, "setTimeout", () => {
  return ChromeUtils.importESModule("resource://gre/modules/Timer.sys.mjs")
    .setTimeout;
});

const BACKEND_PREF = "epoca.registration.backend";
const DEFAULT_BACKEND = "https://bravo.product.parity.io";

// Assignment poll cadence (mirrors host-mobile: 30 x 4s).
const ASSIGN_ATTEMPTS = 30;
const ASSIGN_DELAY_MS = 4_000;

function bareHex(bytes) {
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}

// Match pwallet's scheme: a random 9-letter lowercase stem plus 4 digits. The
// identity is a host wallet account, not a human-chosen handle, so an
// auto-generated name is expected.
function randomUsername() {
  const letters = "abcdefghijklmnopqrstuvwxyz";
  const pick = (set, n) => {
    const buf = new Uint8Array(n);
    crypto.getRandomValues(buf);
    return Array.from(buf, b => set[b % set.length]).join("");
  };
  return `${pick(letters, 9)}.${pick("0123456789", 4)}`;
}

function delay(ms) {
  return new Promise(resolve => lazy.setTimeout(resolve, ms));
}

function backendBase() {
  const pref = Services.prefs.getStringPref(BACKEND_PREF, "").trim();
  return (pref || DEFAULT_BACKEND).replace(/\/+$/, "");
}

async function getJson(url) {
  const resp = await fetch(url, { headers: { Accept: "application/json" } });
  const text = await resp.text();
  return { status: resp.status, text };
}

async function postJson(url, body, bearer) {
  const headers = { "Content-Type": "application/json" };
  if (bearer) {
    headers.Authorization = `Bearer ${bearer}`;
  }
  const resp = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify(body),
  });
  const text = await resp.text();
  return { status: resp.status, text };
}

function field(text, name) {
  try {
    return JSON.parse(text)?.[name];
  } catch {
    return undefined;
  }
}

export const EpocaRegistration = {
  /**
   * Register a lite-person username through the DotSpark backend and wait until
   * it is assigned on-chain (so the wallet identity becomes ring-included).
   *
   * @param {string} [fullUsername] - `<lowercase-letters>.<digits>`, e.g.
   *   "alice.42". Defaults to an auto-generated name.
   * @returns {Promise<object>} { account, username, submitted, assigned }.
   */
  async register(fullUsername = randomUsername()) {
    const backend = backendBase();

    // 1. Resolve the live attester; the payload's consumer-registration
    //    signature binds to it, so this must precede building the payload.
    const attRes = await getJson(`${backend}/api/v1/attester`);
    const attester = field(attRes.text, "attester");
    if (attRes.status < 200 || attRes.status >= 300 || !attester) {
      throw new Error(
        `attester lookup HTTP ${attRes.status}: ${attRes.text}`
      );
    }

    // 2. Build the signed candidate payload, matching the wire shape the
    //    backend actually acts on (verified live): `candidateAccountId` must be
    //    SS58 — a hex account id is accepted by the HTTP layer but silently
    //    fails the on-chain PeopleLite.attest, so the wasm's 0x-hex value is
    //    overridden here. The extra wasm-only fields are dropped.
    const payload = await lazy.EpocaWallet.buildLitePersonRegistrationPayload(
      fullUsername,
      attester
    );
    delete payload.accountIdHex;
    delete payload.fullUsername;

    const accountId = await lazy.EpocaWallet.walletPublicKey();
    payload.candidateAccountId = lazy.EpocaSs58.encodeSs58(accountId, 42);
    const publicKeyHex = bareHex(accountId);

    // 3-4. Challenge-response auth (nonce is single-use, so re-challenge on retry).
    const token = await this._authenticate(backend, publicKeyHex);

    // 5. Submit the attestation.
    const submit = await postJson(
      `${backend}/api/v1/attestations`,
      payload,
      token
    );
    if (submit.status < 200 || submit.status >= 300) {
      throw new Error(
        `attestation submit HTTP ${submit.status}: ${submit.text}`
      );
    }

    // 6. Poll until the backend reports the username assigned on-chain.
    const assigned = await this._pollAssignment(backend, fullUsername);

    return {
      account: "0x" + publicKeyHex,
      username: fullUsername,
      submitted: true,
      assigned,
    };
  },

  async _authenticate(backend, publicKeyHex) {
    let lastErr = "no auth attempts ran";
    for (let i = 0; i < 3; i++) {
      const ch = await postJson(`${backend}/api/v1/auth/challenge`, {
        public_key: publicKeyHex,
      });
      if (ch.status < 200 || ch.status >= 300) {
        lastErr = `auth challenge HTTP ${ch.status}: ${ch.text}`;
        continue;
      }
      const nonce = field(ch.text, "nonce");
      if (!nonce) {
        lastErr = `auth challenge missing nonce: ${ch.text}`;
        continue;
      }
      const signature = await lazy.EpocaWallet.signWallet(
        new TextEncoder().encode(nonce)
      );
      const vr = await postJson(`${backend}/api/v1/auth/verify`, {
        public_key: publicKeyHex,
        signature: bareHex(signature),
      });
      if (vr.status < 200 || vr.status >= 300) {
        lastErr = `auth verify HTTP ${vr.status}: ${vr.text}`;
        continue;
      }
      const token = field(vr.text, "token");
      if (token) {
        return token;
      }
      lastErr = `auth verify missing token: ${vr.text}`;
    }
    throw new Error(`wallet authentication failed: ${lastErr}`);
  },

  async _pollAssignment(backend, fullUsername) {
    // The status endpoint requires a `network` selector that no reference
    // client sends (recent backend drift). Without a configured value there is
    // no point polling — the ground-truth confirmation is on-chain ring
    // inclusion, which EpocaAllowance's ring scan already provides. Return
    // `null` (unknown) rather than a misleading `false`.
    const network = Services.prefs
      .getStringPref("epoca.registration.network", "")
      .trim();
    if (!network) {
      return null;
    }
    const url = `${backend}/api/v1/usernames/${encodeURIComponent(
      fullUsername
    )}?network=${encodeURIComponent(network)}`;
    for (let i = 0; i < ASSIGN_ATTEMPTS; i++) {
      const r = await getJson(url);
      // 404 (not yet recorded) and other transient non-2xx are treated as
      // pending; the on-chain allowance claim is the ground-truth confirmation.
      if (r.status >= 200 && r.status < 300) {
        const status = String(field(r.text, "status") || "").toUpperCase();
        if (status === "ASSIGNED" || status === "ATTESTED") {
          return true;
        }
        if (status === "FAILED" || status === "ERROR" || status === "REJECTED") {
          throw new Error(`registration of "${fullUsername}" was rejected`);
        }
      }
      if (i + 1 < ASSIGN_ATTEMPTS) {
        await delay(ASSIGN_DELAY_MS);
      }
    }
    return false;
  },
};
