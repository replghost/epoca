/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Exercises the TrUAPI local-storage host functions end to end through the
// bridge: write, read back, clear, and read-miss, each validated against
// byte-exact frames (request vectors from the TrUAPI v0.2 golden set; the
// read/clear requests and all responses verified against the engine).
const FRAMES = {
  write: {
    request: "3473746f726167652d77726974650e002073657474696e67730c010203",
    response: "3473746f726167652d77726974650f0000",
  },
  read_hit: {
    request: "3073746f726167652d726561640c002073657474696e6773",
    response: "3073746f726167652d726561640d0000010c010203",
  },
  clear: {
    request: "3473746f726167652d636c65617210002073657474696e6773",
    response: "3473746f726167652d636c656172110000",
  },
  read_miss: {
    request: "3073746f726167652d726561640c002073657474696e6773",
    response: "3073746f726167652d726561640d000000",
  },
};

function hexToBytes(hex) {
  return Uint8Array.from(hex.match(/../g).map(byte => parseInt(byte, 16)));
}

function b64(bytes) {
  return ChromeUtils.base64URLEncode(bytes, { pad: false });
}

add_task(async function test_epoca_storage_roundtrip() {
  await SpecialPowers.pushPrefEnv({
    set: [
      ["epoca.useragent.enabled", true],
      // See browser_epoca_bridge.js: parent-process wasm gate, allowlisted
      // in shipping builds via the nsContentSecurityUtils patch.
      ["security.allow_eval_with_system_principal", true],
    ],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    for (const [step, frames] of Object.entries(FRAMES)) {
      const reply = await SpecialPowers.spawn(
        browser,
        [frames.request],
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
                resolve(
                  ChromeUtils.base64URLEncode(event.data, { pad: false })
                ),
              { once: true }
            );
            port.start();
            port.postMessage(frame);
          });
        }
      );
      is(
        reply,
        b64(hexToBytes(frames.response)),
        `storage step '${step}' returns the expected frame`
      );
    }
  });

  await SpecialPowers.popPrefEnv();
});
