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
    this.refresh();
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
    const { EpocaWallet, EpocaSs58 } = this._modules;
    try {
      const [username, publicKey] = await Promise.all([
        EpocaWallet.getUsername(),
        EpocaWallet.walletPublicKey(),
      ]);
      this._address = EpocaSs58.encodeSs58(publicKey, 42);
      this._field("epocaAccountUsername").value = username || "—";
      this._field("epocaAccountAddress").value = this._address;
      const status = this._field("epocaAccountAllowance");
      if (!status.hasAttribute("data-pending")) {
        document.l10n.setAttributes(
          status,
          username
            ? "epoca-account-provisioned"
            : "epoca-account-not-provisioned"
        );
      }
    } catch (e) {
      console.error("gEpocaAccount: refresh failed", e);
      this._field("epocaAccountUsername").value = "error";
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
};
