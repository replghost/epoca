/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

"use strict";

const { EpocaWallet } = ChromeUtils.importESModule(
  "resource:///modules/EpocaWallet.sys.mjs"
);

const hex = u8 => Array.from(u8, b => b.toString(16).padStart(2, "0")).join("");

// Backup/recovery for the account pane: exportMnemonic reveals the phrase,
// importMnemonic restores from one (destructive, deterministic, validated).
add_task(async function test_wallet_backup_restore() {
  const original = await EpocaWallet.exportMnemonic();
  ok(
    [12, 15, 18, 21, 24].includes(original.trim().split(/\s+/).length),
    "exported a valid-length recovery phrase"
  );
  const origKey = hex(await EpocaWallet.walletPublicKey());

  // The well-known Substrate dev phrase — a valid BIP-39 mnemonic.
  const TEST =
    "bottom drive obey lake curtain smoke basket hold race lonely fit walk";
  try {
    await EpocaWallet.importMnemonic(TEST);
    const newKey = hex(await EpocaWallet.walletPublicKey());
    isnot(newKey, origKey, "importing a phrase switches the wallet key");

    await EpocaWallet.importMnemonic(TEST);
    is(
      hex(await EpocaWallet.walletPublicKey()),
      newKey,
      "same phrase restores the same key (deterministic)"
    );

    await Assert.rejects(
      EpocaWallet.importMnemonic("too few words here"),
      /12/,
      "a malformed phrase is rejected"
    );
    is(
      hex(await EpocaWallet.walletPublicKey()),
      newKey,
      "a rejected import leaves the wallet intact"
    );
  } finally {
    await EpocaWallet.importMnemonic(original);
  }
  is(
    hex(await EpocaWallet.walletPublicKey()),
    origKey,
    "the original wallet is restored"
  );
});
