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

add_task(async function test_permissions_list_and_revoke() {
  const { EpocaPermissions } = ChromeUtils.importESModule(
    "resource:///modules/EpocaPermissions.sys.mjs"
  );
  const pid = "pane-test-" + Date.now();
  await EpocaPermissions.record(pid, "account");

  try {
    await BrowserTestUtils.withNewTab("about:preferences", async browser => {
      const doc = browser.contentDocument;
      doc.getElementById("category-epoca-account").click();

      await BrowserTestUtils.waitForCondition(
        () => doc.getElementById("epocaPermissionsList"),
        "permissions list present"
      );

      // The recorded grant renders as a row with a Revoke button.
      const list = doc.getElementById("epocaPermissionsList");
      const rowFor = () =>
        [...list.children].find(r =>
          (r.querySelector("label")?.value ?? "").includes(pid)
        );
      let row;
      await BrowserTestUtils.waitForCondition(() => {
        row = rowFor();
        return !!row;
      }, "granted product appears in the list");
      ok(row, `permission row for ${pid} rendered`);

      // Revoking removes it from the list and the store.
      row.querySelector("button").click();
      await BrowserTestUtils.waitForCondition(
        () => !rowFor(),
        "row removed after revoke"
      );
      is(
        await EpocaPermissions.has(pid, "account"),
        false,
        "grant revoked in the store"
      );
    });
  } finally {
    await EpocaPermissions.revoke(pid, "account");
  }
});
