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
};
