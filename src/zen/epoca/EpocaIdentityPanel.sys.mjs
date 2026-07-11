// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// A per-window toolbar button + arrow panel that surfaces the epoca host
// wallet: its lite-person username, its //wallet address, and the
// statement-store allowance status, plus a manual "provision" action
// (register on-chain + claim). The wallet is otherwise headless; this is the
// only chrome UI for it. Runs in the chrome window (parent, system
// principal), so it calls the Epoca* modules directly.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaAllowance: "resource:///modules/EpocaAllowance.sys.mjs",
  EpocaSs58: "resource:///modules/EpocaSs58.sys.mjs",
  EpocaWallet: "resource:///modules/EpocaWallet.sys.mjs",
});

// Pink "spark" mark (dot/epoca brand) as a self-contained toolbar icon.
const ICON =
  "data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' " +
  "height='16' viewBox='0 0 16 16'%3E%3Ccircle cx='8' cy='8' r='6' " +
  "fill='%23e6007a'/%3E%3C/svg%3E";

export class EpocaIdentityPanel {
  #address = "";

  constructor(window) {
    this.window = window;
    this.document = window.document;
    try {
      this.#createButton();
      this.#createPanel();
    } catch (e) {
      console.error("EpocaIdentityPanel: init failed", e);
    }
  }

  #createButton() {
    const frag = this.window.MozXULElement.parseXULToFragment(`
      <toolbarbutton id="epoca-identity-button"
                     class="toolbarbutton-1 chromeclass-toolbar-additional"
                     removable="true"
                     image="${ICON}"
                     label="epoca"
                     tooltiptext="epoca identity"/>
    `);
    this.button = frag.querySelector("#epoca-identity-button");
    const navbar = this.document.getElementById("nav-bar");
    const anchorRef = this.document.getElementById("PanelUI-button");
    if (anchorRef?.parentNode === navbar) {
      navbar.insertBefore(frag, anchorRef);
    } else {
      navbar.appendChild(frag);
    }
    this.button.addEventListener("command", () => this.#open());
  }

  #createPanel() {
    const frag = this.window.MozXULElement.parseXULToFragment(`
      <panel id="epoca-identity-panel" type="arrow" orient="vertical"
             role="dialog" aria-label="epoca identity">
        <vbox style="padding: 16px; min-width: 320px; gap: 4px;">
          <label style="font-weight: 600; font-size: 1.1em; margin-bottom: 8px;"
                 value="epoca identity"/>
          <label style="opacity: 0.7;" value="Username"/>
          <label id="epoca-id-username" style="margin-bottom: 8px;"
                 value="—"/>
          <label style="opacity: 0.7;" value="Wallet address"/>
          <label id="epoca-id-address"
                 style="font-family: monospace; margin-bottom: 8px;"
                 crop="end" value="—"/>
          <label style="opacity: 0.7;" value="Statement-store allowance"/>
          <label id="epoca-id-status" style="margin-bottom: 12px;"
                 value="—"/>
          <hbox style="gap: 8px;">
            <button id="epoca-id-copy" label="Copy address"/>
            <button id="epoca-id-provision" label="Register &amp; claim"/>
          </hbox>
        </vbox>
      </panel>
    `);
    this.panel = frag.querySelector("#epoca-identity-panel");
    this.document.getElementById("mainPopupSet").appendChild(frag);
    this.panel
      .querySelector("#epoca-id-copy")
      .addEventListener("command", () => this.#copyAddress());
    this.panel
      .querySelector("#epoca-id-provision")
      .addEventListener("command", () => this.#provision());
  }

  #field(id) {
    return this.panel.querySelector(`#${id}`);
  }

  async #open() {
    this.panel.openPopup(this.button, "bottomright topright", 0, 0, false, false);
    await this.#refresh();
  }

  async #refresh() {
    try {
      const [username, publicKey] = await Promise.all([
        lazy.EpocaWallet.getUsername(),
        lazy.EpocaWallet.walletPublicKey(),
      ]);
      this.#address = lazy.EpocaSs58.encodeSs58(publicKey, 42);
      this.#field("epoca-id-username").value = username || "not registered";
      this.#field("epoca-id-address").value = this.#address;
      if (this.#field("epoca-id-status").value === "—") {
        this.#field("epoca-id-status").value = username
          ? "provisioned"
          : "not provisioned";
      }
    } catch (e) {
      console.error("EpocaIdentityPanel: refresh failed", e);
      this.#field("epoca-id-username").value = "error";
    }
  }

  #copyAddress() {
    if (!this.#address) {
      return;
    }
    Cc["@mozilla.org/widget/clipboardhelper;1"]
      .getService(Ci.nsIClipboardHelper)
      .copyString(this.#address);
  }

  // Prompt for a handle the first time, so the on-chain username is the
  // user's choice rather than a random stem. Returns a full "<name>.<digits>"
  // username, or null if cancelled. Digits are appended for global uniqueness.
  #chooseUsername() {
    const input = { value: "" };
    const ok = Services.prompt.prompt(
      this.window,
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
    this.window.crypto.getRandomValues(buf);
    const digits = Array.from(buf, b => b % 10).join("");
    return `${stem}.${digits}`;
  }

  async #provision() {
    const status = this.#field("epoca-id-status");
    // First-time provisioning: let the user pick their handle before we
    // register it on-chain (register() reuses the persisted username).
    if (!(await lazy.EpocaWallet.getUsername())) {
      const chosen = this.#chooseUsername();
      if (!chosen) {
        return;
      }
      await lazy.EpocaWallet.setUsername(chosen);
      await this.#refresh();
    }
    status.value = "provisioning… (on-chain, may take a few minutes)";
    try {
      const result = await lazy.EpocaAllowance.ensure({ provision: true });
      status.value =
        result.status + (result.username ? ` — ${result.username}` : "");
      await this.#refresh();
    } catch (e) {
      console.error("EpocaIdentityPanel: provision failed", e);
      status.value = `error: ${e.message}`;
    }
  }
}
