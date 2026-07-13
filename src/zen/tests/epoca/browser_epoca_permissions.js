/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

"use strict";

const { EpocaPermissions } = ChromeUtils.importESModule(
  "resource:///modules/EpocaPermissions.sys.mjs"
);

// EpocaPermissions is the persistent, listable source of truth for the
// account/device grants epoca hands dot:// products (formerly per-session Sets
// in EpocaProductParent). Exercise record / has / list / revoke.

add_task(async function test_permissions_roundtrip() {
  const pid = "test-product-" + Date.now();
  try {
    is(await EpocaPermissions.has(pid, "account"), false, "clean to start");

    await EpocaPermissions.record(pid, "account");
    await EpocaPermissions.record(pid, "device", "Camera");
    ok(await EpocaPermissions.has(pid, "account"), "account grant recorded");
    ok(
      await EpocaPermissions.has(pid, "device", "Camera"),
      "device grant recorded"
    );
    is(
      await EpocaPermissions.has(pid, "device", "Microphone"),
      false,
      "an unrelated device kind is not granted"
    );

    // record is idempotent.
    await EpocaPermissions.record(pid, "account");
    const mine = (await EpocaPermissions.list()).filter(
      g => g.productId === pid
    );
    is(mine.length, 2, "no duplicate grant rows");

    const removed = await EpocaPermissions.revoke(pid, "account");
    is(removed, 1, "revoke removed exactly one grant");
    is(
      await EpocaPermissions.has(pid, "account"),
      false,
      "account grant revoked"
    );
    ok(
      await EpocaPermissions.has(pid, "device", "Camera"),
      "device grant untouched by account revoke"
    );
  } finally {
    await EpocaPermissions.revoke(pid, "device", "Camera");
    await EpocaPermissions.revoke(pid, "account");
  }
});
