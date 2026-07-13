/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// Controller for the Epoca "Account" preferences pane (paneEpocaAccount).
// Surfaces the headless host wallet — its lite-person username, //wallet
// address, and statement-store allowance — plus a manual register/claim
// action. Runs in the about:preferences chrome window (parent, system
// principal), so it imports the Epoca* modules directly. This replaces the
// old urlbar identity "pink dot" popup (EpocaIdentityPanel).

"use strict";

var gEpocaAccount = {
  __hasInitialized: false,
  _address: "",

  get _modules() {
    if (!this.__modules) {
      this.__modules = {};
      ChromeUtils.defineESModuleGetters(this.__modules, {
        EpocaAllowance: "resource:///modules/EpocaAllowance.sys.mjs",
        EpocaPermissions: "resource:///modules/EpocaPermissions.sys.mjs",
        EpocaRegistration: "resource:///modules/EpocaRegistration.sys.mjs",
        EpocaSs58: "resource:///modules/EpocaSs58.sys.mjs",
        EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
      });
    }
    return this.__modules;
  },

  init() {
    if (this.__hasInitialized) {
      return;
    }
    this.__hasInitialized = true;
    this._field("epocaAccountCopyAddress").addEventListener("command", () =>
      this.copyAddress()
    );
    this._field("epocaAccountProvision").addEventListener("command", () =>
      this.provision()
    );
    this._field("epocaRevealPhrase").addEventListener("command", () =>
      this.revealPhrase()
    );
    this._field("epocaRestorePhrase").addEventListener("command", () =>
      this.restorePhrase()
    );
    this._field("epocaCopyPhrase").addEventListener("command", () =>
      this.copyPhrase()
    );
    this._field("epocaHidePhrase").addEventListener("command", () =>
      this.hidePhrase()
    );
    // Never leave the decrypted phrase in the DOM once the pane goes away.
    window.addEventListener("unload", () => this.hidePhrase());
    this.refresh();
    this.refreshPermissions();
    // A claim runs in the shared parent module, so it survives navigating away
    // from this pane. If one is already in flight (e.g. started, left, and
    // reopened), re-attach so the spinner + result show here too.
    const { EpocaAllowance } = this._modules;
    if (EpocaAllowance._inFlight) {
      this._driveClaim(EpocaAllowance._inFlight);
    }
  },

  _field(id) {
    return document.getElementById(id);
  },

  async refresh() {
    const { EpocaWallet, EpocaSs58, EpocaRegistration } = this._modules;
    const nameField = this._field("epocaAccountUsername");
    try {
      const publicKey = await EpocaWallet.walletPublicKey();
      this._address = EpocaSs58.encodeSs58(publicKey, 42);
      this._field("epocaAccountAddress").value = this._address;
    } catch (e) {
      console.error("gEpocaAccount: wallet load failed", e);
      nameField.value = "error";
      return;
    }
    // Show the locally-cached handle immediately, then override with the
    // authoritative on-chain name — which resolves even for a wallet restored
    // from a recovery phrase (no local handle) and prefers the full handle.
    let display = await EpocaWallet.getUsername();
    nameField.value = display || "resolving…";
    // The authoritative name lives on-chain (Resources.Consumers), which needs
    // a live People-chain RPC. Gated by a pref so tests (which forbid non-local
    // connections) can stay offline; on by default in real use.
    if (
      Services.prefs.getBoolPref("epoca.identity.resolve-onchain", true)
    ) {
      try {
        const onchain = await EpocaRegistration.resolveUsername();
        if (onchain?.display) {
          display = onchain.display;
        }
      } catch (e) {
        console.error("gEpocaAccount: on-chain username resolve failed", e);
      }
    }
    nameField.value = display || "not registered";
    const status = this._field("epocaAccountAllowance");
    if (!status.hasAttribute("data-pending")) {
      document.l10n.setAttributes(
        status,
        display ? "epoca-account-provisioned" : "epoca-account-not-provisioned"
      );
    }
    // Reflect state on the action button: "Register & claim" when there's no
    // handle yet, "Claim allowance" once the handle is registered (the
    // register half is done). Left enabled — a claim is idempotent (returns
    // "already-granted" if the allowance is already held). Don't relabel while
    // a claim is in flight.
    const action = this._field("epocaAccountProvision");
    if (!action.disabled) {
      document.l10n.setAttributes(
        action,
        display ? "epoca-account-claim" : "epoca-account-provision"
      );
    }
  },

  copyAddress() {
    if (!this._address) {
      return;
    }
    Cc["@mozilla.org/widget/clipboardhelper;1"]
      .getService(Ci.nsIClipboardHelper)
      .copyString(this._address);
  },

  // Prompt for a handle the first time so the on-chain username is the user's
  // choice; a 4-digit tag is appended for global uniqueness. Returns a full
  // "<name>.<digits>" username, or null if cancelled.
  _chooseUsername() {
    const input = { value: "" };
    const ok = Services.prompt.prompt(
      window,
      "Choose your epoca handle",
      "Lowercase letters only. A 4-digit tag is added to keep it unique.",
      input,
      null,
      { value: false }
    );
    if (!ok) {
      return null;
    }
    const stem = input.value.toLowerCase().replace(/[^a-z]/g, "");
    if (!stem) {
      return null;
    }
    const buf = new Uint8Array(4);
    crypto.getRandomValues(buf);
    const digits = Array.from(buf, b => b % 10).join("");
    return `${stem}.${digits}`;
  },

  _setBusy(busy) {
    this._field("epocaAccountBusy").hidden = !busy;
    this._field("epocaAccountProvision").disabled = busy;
  },

  // Drive the pane UI off a claim promise (a spinner while it runs, the result
  // when it settles). Shared by provision() and the reopen re-attach path.
  async _driveClaim(promise) {
    const status = this._field("epocaAccountAllowance");
    // data-pending keeps refresh() from overwriting the status line.
    status.setAttribute("data-pending", "1");
    status.removeAttribute("data-l10n-id");
    status.value = "registering on-chain…";
    this._setBusy(true);
    try {
      const result = await promise;
      await this.refresh();
      status.value =
        result.status + (result.username ? ` — ${result.username}` : "");
    } catch (e) {
      console.error("gEpocaAccount: claim failed", e);
      status.value = `error: ${e.message}`;
    } finally {
      status.removeAttribute("data-pending");
      this._setBusy(false);
    }
  },

  async provision() {
    const { EpocaWallet, EpocaAllowance } = this._modules;
    if (this._field("epocaAccountProvision").disabled) {
      return; // a claim is already in flight
    }
    // First-time provisioning: let the user pick their handle before we
    // register it on-chain (ensure() reuses the persisted username).
    if (!(await EpocaWallet.getUsername())) {
      const chosen = this._chooseUsername();
      if (!chosen) {
        return;
      }
      await EpocaWallet.setUsername(chosen);
      await this.refresh();
    }
    await this._driveClaim(EpocaAllowance.ensure({ provision: true }));
  },

  _grantSummary(grant) {
    const what = grant.type === "account" ? "Account access" : grant.kind;
    return `${grant.productId} — ${what}`;
  },

  async refreshPermissions() {
    const { EpocaPermissions } = this._modules;
    const list = this._field("epocaPermissionsList");
    const empty = this._field("epocaPermissionsEmpty");
    let grants;
    try {
      grants = await EpocaPermissions.list();
    } catch (e) {
      console.error("gEpocaAccount: list permissions failed", e);
      return;
    }
    while (list.firstChild) {
      list.firstChild.remove();
    }
    empty.hidden = grants.length > 0;
    for (const grant of grants) {
      const row = document.createXULElement("hbox");
      row.setAttribute("align", "center");
      row.style.gap = "8px";

      const label = document.createXULElement("label");
      label.setAttribute("flex", "1");
      label.setAttribute("crop", "end");
      label.value = this._grantSummary(grant);
      row.appendChild(label);

      const revoke = document.createXULElement("button");
      document.l10n.setAttributes(revoke, "epoca-permissions-revoke");
      revoke.addEventListener("command", async () => {
        revoke.disabled = true;
        await EpocaPermissions.revoke(grant.productId, grant.type, grant.kind);
        await this.refreshPermissions();
      });
      row.appendChild(revoke);

      list.appendChild(row);
    }
  },

  async revealPhrase() {
    const { EpocaWallet } = this._modules;
    const ok = Services.prompt.confirm(
      window,
      "Reveal recovery phrase",
      "Your recovery phrase gives full control of this account. Make sure no " +
        "one can see your screen. Reveal it now?"
    );
    if (!ok) {
      return;
    }
    try {
      // Decrypt goes through OSKeyStore, which reauthenticates the user.
      const phrase = await EpocaWallet.exportMnemonic();
      this._field("epocaPhraseField").value = phrase;
      this._field("epocaPhraseBox").hidden = false;
    } catch (e) {
      console.error("gEpocaAccount: reveal phrase failed", e);
    }
  },

  copyPhrase() {
    const value = this._field("epocaPhraseField").value;
    if (!value) {
      return;
    }
    Cc["@mozilla.org/widget/clipboardhelper;1"]
      .getService(Ci.nsIClipboardHelper)
      .copyString(value);
  },

  hidePhrase() {
    this._field("epocaPhraseField").value = "";
    this._field("epocaPhraseBox").hidden = true;
  },

  async restorePhrase() {
    const { EpocaWallet } = this._modules;
    const input = { value: "" };
    const entered = Services.prompt.prompt(
      window,
      "Restore from recovery phrase",
      "Enter your 12–24 word recovery phrase. This replaces the current " +
        "account on this device.",
      input,
      null,
      { value: false }
    );
    if (!entered || !input.value.trim()) {
      return;
    }
    const confirmed = Services.prompt.confirm(
      window,
      "Replace this account?",
      "Restoring will replace the account currently on this device. Make sure " +
        "you have its recovery phrase backed up first. Continue?"
    );
    if (!confirmed) {
      return;
    }
    try {
      await EpocaWallet.importMnemonic(input.value);
      this.hidePhrase();
      await this.refresh();
    } catch (e) {
      console.error("gEpocaAccount: restore phrase failed", e);
      Services.prompt.alert(
        window,
        "Restore failed",
        e.message || String(e)
      );
    }
  },
};
