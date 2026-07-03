/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Exercises the TrUAPI account and signing host functions through the
// bridge, driven by a fixed dev mnemonic so derived keys are deterministic.
//
// Vectors from the TrUAPI v0.2 golden set (account-get, sign-raw). The
// account-get response is fully deterministic and asserted byte-exact; the
// sign response embeds a randomized sr25519 signature, so only its framing
// is asserted.
const DEV_MNEMONIC =
  "bottom drive obey lake curtain smoke basket hold race lonely fit walk";

const ACCOUNT_GET_REQUEST = "2c6163636f756e742d676574160024616c6963652e646f7407000000";
const ACCOUNT_GET_RESPONSE =
  "2c6163636f756e742d67657417000080" +
  "bcaff4d9c84a01efa9929a32540aff8bde095500465e501b24f0ad43c8b8453f";
const ACCOUNT_GET_REJECTED = "2c6163636f756e742d67657417000101";

const SIGN_RAW_REQUEST = "207369676e2d726177720024616c6963652e646f7407000000000c090807";
// id "sign-raw" + resp tag 115 + v0 + Ok + compact(64); then 64 sig bytes
// (randomized) + Option::None(0x00). Only the framing is deterministic.
const SIGN_RAW_OK_PREFIX = "207369676e2d7261777300000101";
const SIGN_RAW_OK_LENGTH = SIGN_RAW_OK_PREFIX.length + 128 + 2;
const SIGN_RAW_REJECTED = "207369676e2d72617773000101";

function hexToBytes(hex) {
  return Uint8Array.from(hex.match(/../g).map(byte => parseInt(byte, 16)));
}

function b64(bytes) {
  return ChromeUtils.base64URLEncode(bytes, { pad: false });
}

function bytesToHex(bytes) {
  return Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("");
}

// Post a frame on the product port and resolve with the reply as hex.
async function exchange(browser, requestHex) {
  const replyB64 = await SpecialPowers.spawn(
    browser,
    [requestHex],
    async hex => {
      const port = content.wrappedJSObject.__HOST_API_PORT__;
      const frame = Cu.cloneInto(
        Uint8Array.from(hex.match(/../g).map(byte => parseInt(byte, 16))),
        content
      );
      return new Promise(resolve => {
        port.addEventListener(
          "message",
          event =>
            resolve(ChromeUtils.base64URLEncode(event.data, { pad: false })),
          { once: true }
        );
        port.start();
        port.postMessage(frame);
      });
    }
  );
  return bytesToHex(
    new Uint8Array(ChromeUtils.base64URLDecode(replyB64, { padding: "reject" }))
  );
}

add_task(async function test_epoca_account_get_and_sign_approved() {
  await SpecialPowers.pushPrefEnv({
    set: [
      ["epoca.useragent.enabled", true],
      ["security.allow_eval_with_system_principal", true],
      ["epoca.useragent.dev-mnemonic", DEV_MNEMONIC],
      ["epoca.useragent.auto-approve", true],
    ],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    const account = await exchange(browser, ACCOUNT_GET_REQUEST);
    is(
      account,
      ACCOUNT_GET_RESPONSE,
      "account-get returns the derived public key (golden frame)"
    );

    const signed = await exchange(browser, SIGN_RAW_REQUEST);
    ok(
      signed.startsWith(SIGN_RAW_OK_PREFIX),
      `sign-raw returns a signed frame (got ${signed})`
    );
    is(signed.length, SIGN_RAW_OK_LENGTH, "signature is 64 bytes");
  });

  await SpecialPowers.popPrefEnv();
});

add_task(async function test_epoca_sign_rejected_by_user() {
  await SpecialPowers.pushPrefEnv({
    set: [
      ["epoca.useragent.enabled", true],
      ["security.allow_eval_with_system_principal", true],
      ["epoca.useragent.dev-mnemonic", DEV_MNEMONIC],
      // auto-approve off: drive the real consent prompt and click Deny.
    ],
  });

  const { PromptTestUtils } = ChromeUtils.importESModule(
    "resource://testing-common/PromptTestUtils.sys.mjs"
  );

  // A distinct origin so the module-level account grant from the previous
  // test doesn't suppress this test's account-access prompt.
  await BrowserTestUtils.withNewTab("https://example.org/", async browser => {
    // account-get must be granted first (Allow) so the sign request reaches
    // its own prompt.
    const grantPromise = PromptTestUtils.handleNextPrompt(
      browser,
      { modalType: Services.prompt.MODAL_TYPE_TAB },
      { buttonNumClick: 0 }
    );
    const accountP = exchange(browser, ACCOUNT_GET_REQUEST);
    await grantPromise;
    const account = await accountP;
    is(account, ACCOUNT_GET_RESPONSE, "account granted via Allow prompt");

    // Now deny the signature.
    const denyPromise = PromptTestUtils.handleNextPrompt(
      browser,
      { modalType: Services.prompt.MODAL_TYPE_TAB },
      { buttonNumClick: 1 }
    );
    const signP = exchange(browser, SIGN_RAW_REQUEST);
    await denyPromise;
    const signed = await signP;
    is(
      b64(hexToBytes(signed)),
      b64(hexToBytes(SIGN_RAW_REJECTED)),
      "denied signature returns the Rejected error frame"
    );
  });

  await SpecialPowers.popPrefEnv();
});
