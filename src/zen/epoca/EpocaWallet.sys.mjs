// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// The host wallet: an sr25519 keyring backing the TrUAPI account and
// signing host functions. Each product gets an isolated key via hard
// derivation (//app//<dotns_id>//<index>) — private keys never leave this
// module; products only ever see public keys and signatures.
//
// The mnemonic is generated once per profile and stored encrypted at rest in
// the profile (wallet.json "mnemonicEnc"), using OSKeyStore — the same
// OS-keychain-backed secret store Firefox uses for saved passwords (macOS
// Keychain / Windows Credential Manager / libsecret). A pref override exists
// for tests and dev. (An auto-lock timeout via WalletHandle.lock/tick is a
// later refinement.)

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaHostEngine: "resource:///modules/EpocaHostEngine.sys.mjs",
  JSONFile: "resource://gre/modules/JSONFile.sys.mjs",
  OSKeyStore: "resource://gre/modules/OSKeyStore.sys.mjs",
});

const MNEMONIC_PREF = "epoca.useragent.dev-mnemonic";

export const EpocaWallet = {
  _walletPromise: null,

  _ensure() {
    if (!this._walletPromise) {
      this._walletPromise = this._create();
    }
    return this._walletPromise;
  },

  async _create() {
    const glue = await lazy.EpocaHostEngine.glue();
    const wallet = new glue.WalletHandle();
    wallet.loadMnemonic(await this._mnemonic(glue));
    return wallet;
  },

  async _mnemonic(glue) {
    const override = Services.prefs.getStringPref(MNEMONIC_PREF, "");
    if (override) {
      return override;
    }
    const file = new lazy.JSONFile({
      path: PathUtils.join(PathUtils.profileDir, "epoca", "wallet.json"),
    });
    await file.load();

    // Encrypted at rest (OS keychain-backed).
    if (file.data.mnemonicEnc) {
      return lazy.OSKeyStore.decrypt(file.data.mnemonicEnc);
    }

    // Either first run, or migration of a legacy plaintext mnemonic to
    // encrypted storage.
    const mnemonic = file.data.mnemonic || glue.generateMnemonic();
    file.data.mnemonicEnc = await lazy.OSKeyStore.encrypt(mnemonic);
    delete file.data.mnemonic;
    await file._save();
    return mnemonic;
  },

  /**
   * @returns {Promise<Uint8Array>} 32-byte sr25519 public key for the
   *   product's derived account.
   */
  async appPublicKey(dotnsId, index) {
    const wallet = await this._ensure();
    return wallet.appPublicKey(dotnsId, index);
  },

  /**
   * @param {Uint8Array} payload - opaque bytes to sign (as delivered by the
   *   engine; do not re-decode).
   * @returns {Promise<Uint8Array>} 64-byte sr25519 signature.
   */
  async sign(dotnsId, index, payload) {
    const wallet = await this._ensure();
    return wallet.sign(dotnsId, index, payload);
  },

  /**
   * Deterministic, product-scoped entropy for the `deriveEntropy` host
   * function: a keyed blake2b chain over the BIP-39 root entropy, the product
   * id, and the caller's key. The root entropy never leaves the wallet.
   *
   * @param {string} productId
   * @param {Uint8Array} key - caller-chosen, up to 32 bytes.
   * @returns {Promise<Uint8Array>} 32 bytes.
   */
  async deriveProductEntropy(productId, key) {
    const wallet = await this._ensure();
    return wallet.deriveProductEntropy(productId, key);
  },

  _identityFile() {
    const file = new lazy.JSONFile({
      path: PathUtils.join(PathUtils.profileDir, "epoca", "identity.json"),
    });
    return file.load().then(() => file);
  },

  /**
   * The wallet's registered lite-person username (`<name>.<digits>`), or null
   * if the identity has not been provisioned. Non-secret, so kept out of the
   * encrypted wallet.json.
   *
   * @returns {Promise<string|null>}
   */
  async getUsername() {
    const file = await this._identityFile();
    return file.data.username ?? null;
  },

  /** Persist the chosen username so it is stable across registration retries. */
  async setUsername(username) {
    const file = await this._identityFile();
    file.data.username = username;
    await file._save();
  },

  /**
   * The wallet's chat-identity account (`//wallet`), used as the owner of the
   * statement-store allowance and by the native chat/statement-store protocol.
   *
   * @returns {Promise<Uint8Array>} 32-byte sr25519 public key.
   */
  async walletPublicKey() {
    const wallet = await this._ensure();
    return wallet.walletPublicKey();
  },

  /**
   * The 32-byte lite-person ring-VRF member key (hex, `0x`-prefixed), used to
   * locate this account in the on-chain allowance ring.
   *
   * @returns {Promise<string>}
   */
  async ringVrfMemberKey() {
    const wallet = await this._ensure();
    return wallet.ringVrfMemberKey();
  },

  /**
   * Sign with the `//wallet` chat-identity sr25519 key (the account behind
   * walletPublicKey). Used to answer the DotSpark auth challenge.
   *
   * @param {Uint8Array} payload
   * @returns {Promise<Uint8Array>} 64-byte signature.
   */
  async signWallet(payload) {
    const wallet = await this._ensure();
    return wallet.signWallet(payload);
  },

  /**
   * Build the signed lite-person username-registration payload for submission
   * to the DotSpark backend. Binds the consumer-registration signature to the
   * given verifier ("attester") account.
   *
   * @param {string} fullUsername - `<lowercase-letters>.<digits>`.
   * @param {string} verifierAccountId - attester SS58 or 0x-hex.
   * @returns {Promise<object>} the camelCase payload (0x-hex fields).
   */
  async buildLitePersonRegistrationPayload(fullUsername, verifierAccountId) {
    const wallet = await this._ensure();
    return wallet.buildLitePersonRegistrationPayload(
      fullUsername,
      verifierAccountId
    );
  },

  /**
   * Assemble the statement-store allowance-claim extrinsic (hex, `0x`-prefixed),
   * including the ring-VRF proof signed with the wallet's Bandersnatch key.
   * Chain state (ring members/index, day-period, runtime versions, genesis) is
   * fetched by the caller via RPC and passed in.
   *
   * @returns {Promise<string>}
   */
  async buildAllowanceClaimExtrinsic(
    membersHex,
    ringIndex,
    period,
    seq,
    specVersion,
    txVersion,
    genesisHex
  ) {
    const wallet = await this._ensure();
    return wallet.buildAllowanceClaimExtrinsic(
      membersHex,
      ringIndex,
      period,
      seq,
      specVersion,
      txVersion,
      genesisHex
    );
  },
};
