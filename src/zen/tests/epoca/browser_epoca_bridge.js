/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Validates the product bridge end to end: a page loaded while
// epoca.useragent.enabled is set must see the host mark and the MessagePort,
// and a byte frame posted on the port must round-trip through the parent
// host engine and come back correlated to the request.

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

add_task(async function test_epoca_bridge_handshake_roundtrip() {
  await SpecialPowers.pushPrefEnv({
    set: [["epoca.useragent.enabled", true]],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    const response = await SpecialPowers.spawn(browser, [], async () => {
      const page = content.wrappedJSObject;

      Assert.strictEqual(
        page.__HOST_WEBVIEW_MARK__,
        true,
        "host webview mark is set"
      );
      const port = page.__HOST_API_PORT__;
      Assert.ok(port, "host api MessagePort is published");

      const request = { id: "poc-1", method: "epoca.handshake", params: {} };
      const frame = new content.TextEncoder().encode(JSON.stringify(request));

      const replyText = await new Promise(resolve => {
        port.addEventListener(
          "message",
          // Decode inside the listener: WebIDL unwraps the cross-compartment
          // buffer natively, where JS-level TypedArray access is forbidden.
          event => resolve(new TextDecoder().decode(event.data)),
          { once: true }
        );
        port.start();
        port.postMessage(frame);
      });

      return JSON.parse(replyText);
    });

    is(response.id, "poc-1", "response correlates to the request id");
    is(response.result.host, "epoca", "handshake identifies the epoca host");
    is(
      response.result.protocol,
      "poc-json-v0",
      "handshake reports the PoC protocol version"
    );
  });

  await SpecialPowers.popPrefEnv();
});

add_task(async function test_epoca_bridge_unknown_method() {
  await SpecialPowers.pushPrefEnv({
    set: [["epoca.useragent.enabled", true]],
  });

  await BrowserTestUtils.withNewTab("https://example.com/", async browser => {
    const response = await SpecialPowers.spawn(browser, [], async () => {
      const page = content.wrappedJSObject;
      const port = page.__HOST_API_PORT__;

      const request = { id: "poc-2", method: "epoca.no-such-method" };
      const frame = new content.TextEncoder().encode(JSON.stringify(request));

      const replyText = await new Promise(resolve => {
        port.addEventListener(
          "message",
          // Decode inside the listener: WebIDL unwraps the cross-compartment
          // buffer natively, where JS-level TypedArray access is forbidden.
          event => resolve(new TextDecoder().decode(event.data)),
          { once: true }
        );
        port.start();
        port.postMessage(frame);
      });

      return JSON.parse(replyText);
    });

    is(response.id, "poc-2", "error response correlates to the request id");
    is(
      response.error.code,
      "unknown-method",
      "unknown methods return a typed error"
    );
  });

  await SpecialPowers.popPrefEnv();
});
