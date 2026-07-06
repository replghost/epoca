/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Validates the SS58 encoder (vendored blake2b-512 + base58) that DotSpark
// registration relies on for candidateAccountId. Runs in Gecko to confirm the
// crypto matches known vectors here, not just under Node. Also smoke-imports
// the registration/allowance modules so a load-time error surfaces in CI.

const { encodeSs58 } = ChromeUtils.importESModule(
  "resource:///modules/EpocaSs58.sys.mjs"
);

function hexToBytes(hex) {
  return Uint8Array.from(hex.match(/../g).map(b => parseInt(b, 16)));
}

add_task(function test_ss58_known_vectors() {
  const cases = [
    // Well-known //Alice account.
    [
      "d43593c715fdd31c61141abd04a99fd6822c8558854ccde39a5684e7a56da27d",
      "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY",
    ],
    // //Bob.
    [
      "8eaf04151687736326c9fea17e25fc5287613693c912909cb226aa4794f26a48",
      "5FHneW46xGXgs5mUiveU4sbTyGBzmstUspZC92UhjJM694ty",
    ],
  ];
  for (const [hex, expected] of cases) {
    is(encodeSs58(hexToBytes(hex), 42), expected, `SS58 of ${hex.slice(0, 8)}`);
  }
});

add_task(function test_ss58_rejects_large_prefix() {
  Assert.throws(
    () => encodeSs58(new Uint8Array(32), 64),
    /unsupported SS58 prefix/,
    "prefix >= 64 is rejected"
  );
});

add_task(function test_modules_load() {
  const { EpocaRegistration } = ChromeUtils.importESModule(
    "resource:///modules/EpocaRegistration.sys.mjs"
  );
  const { EpocaAllowance } = ChromeUtils.importESModule(
    "resource:///modules/EpocaAllowance.sys.mjs"
  );
  is(typeof EpocaRegistration.register, "function", "register() exists");
  is(typeof EpocaAllowance.ensure, "function", "ensure() exists");
});
