/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

"use strict";

// The Epoca "Account" preferences pane (paneEpocaAccount) is the identity
// surface that replaced the retired urlbar pink-dot popup. Verify it registers
// in about:preferences and that its controller populates the wallet address.

add_task(async function test_epoca_account_pane() {
  await BrowserTestUtils.withNewTab("about:preferences", async browser => {
    const doc = browser.contentDocument;
    const win = browser.contentWindow;

    // Category button is present in the nav.
    const category = doc.getElementById("category-epoca-account");
    ok(category, "Epoca account category button exists");
    is(
      category.getAttribute("view"),
      "paneEpocaAccount",
      "category points at paneEpocaAccount"
    );

    // Selecting the pane expands its template and runs gEpocaAccount.init().
    category.click();

    await BrowserTestUtils.waitForCondition(
      () => doc.getElementById("epocaAccountGroup") && win.gEpocaAccount,
      "pane group + controller present"
    );
    ok(doc.getElementById("epocaAccountGroup"), "account groupbox exists");
    ok(win.gEpocaAccount, "gEpocaAccount controller defined");

    // The controller resolves the wallet and fills the SS58 address. This
    // exercises the real Epoca* modules end to end.
    const addr = doc.getElementById("epocaAccountAddress");
    await BrowserTestUtils.waitForCondition(
      () => addr.value && addr.value !== "—" && addr.value !== "error",
      "wallet address populated"
    );
    ok(addr.value.length > 40, `address looks like SS58: ${addr.value}`);
  });
});
