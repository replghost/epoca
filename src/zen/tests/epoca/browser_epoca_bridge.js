/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Validates the product bridge end to end against the real TrUAPI engine:
// a page loaded while epoca.useragent.enabled is set must see the host mark
// and the MessagePort, and a SCALE handshake frame posted on the port must
// round-trip through the UserAgentKit WASM engine in the parent process.
//
// Frames from the TrUAPI v0.2 golden vectors (useragent-kit conformance):
// request  = SCALE str "handshake" + TAG_HANDSHAKE_REQ + version v1 + 1
// response = SCALE str "handshake" + TAG_HANDSHAKE_RESP + v1 + Ok(())
const HANDSHAKE_REQUEST_HEX = "2468616e647368616b65000001";
const HANDSHAKE_RESPONSE_HEX = "2468616e647368616b65010000";

function hexToBytes(hex) {
  return Uint8Array.from(
    hex.match(/../g).map(byte => parseInt(byte, 16))
  );
}

function b64(bytes) {
  return ChromeUtils.base64URLEncode(bytes, { pad: false });
}

async function postFrameAndAwaitReply(browser, frameHex) {
  // Returns the reply frame base64url-encoded (byte-safe across the
  // compartment boundary, where JS-level TypedArray access is forbidden).
  return SpecialPowers.spawn(browser, [frameHex], async hex => {
    const page = content.wrappedJSObject;
    const port = page.__HOST_API_PORT__;
    Assert.ok(port, "host api MessagePort is published");

    // Build the frame in the page compartment: a privileged-compartment
    // typed array fails the content port's structured clone.
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
  });
}

add_task(async function test_epoca_bridge_disabled_by_default() {
  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    const exposed = await SpecialPowers.spawn(browser, [], () => {
      const page = content.wrappedJSObject;
      return {
        mark: page.__HOST_WEBVIEW_MARK__ !== undefined,
        port: page.__HOST_API_PORT__ !== undefined,
      };
    });
    ok(!exposed.mark, "host mark is not exposed while the pref is off");
    ok(!exposed.port, "host port is not exposed while the pref is off");
  });
});

add_task(async function test_epoca_bridge_truapi_handshake() {
  await SpecialPowers.pushPrefEnv({
    set: [
      ["epoca.useragent.enabled", true],
      // The engine wasm instantiates in the parent process, which shares
      // Firefox's hardened eval gate. Shipping builds will allowlist the
      // vendored glue in nsContentSecurityUtils instead of this pref.
      ["security.allow_eval_with_system_principal", true],
    ],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    const markSet = await SpecialPowers.spawn(
      browser,
      [],
      () => content.wrappedJSObject.__HOST_WEBVIEW_MARK__ === true
    );
    ok(markSet, "host webview mark is set");

    const reply = await postFrameAndAwaitReply(browser, HANDSHAKE_REQUEST_HEX);
    is(
      reply,
      b64(hexToBytes(HANDSHAKE_RESPONSE_HEX)),
      "engine answers the TrUAPI handshake with the golden response frame"
    );
  });

  await SpecialPowers.popPrefEnv();
});

add_task(async function test_epoca_bridge_survives_garbage_frame() {
  await SpecialPowers.pushPrefEnv({
    set: [
      ["epoca.useragent.enabled", true],
      // The engine wasm instantiates in the parent process, which shares
      // Firefox's hardened eval gate. Shipping builds will allowlist the
      // vendored glue in nsContentSecurityUtils instead of this pref.
      ["security.allow_eval_with_system_principal", true],
    ],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    // A malformed frame must not produce a reply or wedge the bridge...
    await SpecialPowers.spawn(browser, [], () => {
      const port = content.wrappedJSObject.__HOST_API_PORT__;
      port.start();
      port.postMessage(
        Cu.cloneInto(Uint8Array.from([0xde, 0xad, 0xbe, 0xef]), content)
      );
    });

    // ...so a handshake sent right after must still round-trip.
    const reply = await postFrameAndAwaitReply(browser, HANDSHAKE_REQUEST_HEX);
    is(
      reply,
      b64(hexToBytes(HANDSHAKE_RESPONSE_HEX)),
      "bridge still answers the handshake after a malformed frame"
    );
  });

  await SpecialPowers.popPrefEnv();
});
