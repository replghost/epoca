/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Validates the dotapp:// product scheme end to end: a registered bundle
// loads in a tab with a host-keyed content principal, subresources resolve
// through the content-process asset pipe, the injected CSP blocks network
// access, and two products get isolated persistent origins.

const { EpocaDotAppRegistry } = ChromeUtils.importESModule(
  "resource:///modules/EpocaDotAppRegistry.sys.mjs"
);

function bytes(text) {
  return new TextEncoder().encode(text);
}

const PRODUCT_A_HTML = `<!DOCTYPE html>
<html>
  <head>
    <title>product-a</title>
    <link rel="icon" type="image/svg+xml" href="/favicon.svg">
  </head>
  <body>
    <h1 id="title">alpha</h1>
    <a id="product-link" href="dot://product-b.dot/">product-b</a>
    <script src="/app.js"></script>
  </body>
</html>`;

const FAVICON_SVG = `<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><rect width="16" height="16" fill="#e6007a"/></svg>`;

add_setup(function () {
  EpocaDotAppRegistry.register("product-a", {
    "/index.html": bytes(PRODUCT_A_HTML),
    "/app.js": bytes(
      `document.getElementById("title").dataset.scripted = "yes";`
    ),
    "/data.json": bytes(`{"ok":true}`),
    "/favicon.svg": bytes(FAVICON_SVG),
  });
  EpocaDotAppRegistry.register("product-b", {
    "/index.html": bytes(
      `<!DOCTYPE html><html><head><title>product-b</title></head><body></body></html>`
    ),
  });
  // SPA with a RELATIVE asset path (like Nuxt/Vite "./assets/x.js"), served
  // under a client sub-route to exercise the injected <base href="/">.
  EpocaDotAppRegistry.register("spa", {
    "/index.html": bytes(
      `<!DOCTYPE html><html><head><title>spa</title>` +
        `<script src="./assets/app.js"></script></head>` +
        `<body><h1 id="t">spa</h1></body></html>`
    ),
    "/assets/app.js": bytes(
      // Head script: set a flag on documentElement (always present) so the
      // test observes that the relative asset loaded, not DOM timing.
      `document.documentElement.dataset.assetLoaded = "yes";`
    ),
  });
  registerCleanupFunction(() => {
    EpocaDotAppRegistry.unregister("product-a");
    EpocaDotAppRegistry.unregister("product-b");
    EpocaDotAppRegistry.unregister("spa");
  });
});

add_task(async function test_dotapp_loads_with_isolated_origin() {
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", async browser => {
    const result = await SpecialPowers.spawn(browser, [], () => {
      return {
        title: content.document.title,
        origin: content.origin,
        scripted:
          content.document.getElementById("title").dataset.scripted === "yes",
        cspInjected: !!content.document.querySelector(
          'meta[http-equiv="Content-Security-Policy"]'
        ),
        baseInjected: !!content.document.querySelector('base[href="/"]'),
        secureContext: content.isSecureContext,
      };
    });
    is(result.title, "product-a", "document served from the registry");
    ok(result.baseInjected, "host <base href='/'> was injected");
    is(
      result.origin,
      "dot://product-a.dot",
      "content principal keys on the product host"
    );
    ok(result.scripted, "subresource script loaded and executed");
    ok(result.cspInjected, "host CSP meta was injected into the document");
    ok(result.secureContext, "dotapp documents are secure contexts");
  });
});

add_task(async function test_dotapp_spa_subroute_relative_assets() {
  // Loading a client sub-route serves index.html (SPA fallback); its relative
  // "./assets/app.js" must resolve against the product root, not the route
  // path, thanks to the injected <base href="/">.
  await BrowserTestUtils.withNewTab(
    "dot://spa.dot/play/deep",
    async browser => {
      const loaded = await SpecialPowers.spawn(browser, [], () => {
        return content.document.documentElement.dataset.assetLoaded;
      });
      is(loaded, "yes", "relative asset resolved to root under a sub-route");
    }
  );
});

add_task(async function test_dotapp_network_locked() {
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", async browser => {
    const result = await SpecialPowers.spawn(browser, [], async () => {
      const own = await content
        .fetch("/data.json")
        .then(r => r.json())
        .catch(e => ({ error: String(e) }));

      let networkBlocked = false;
      try {
        await content.fetch("https://example.com/");
      } catch (e) {
        networkBlocked = true;
      }
      return { own, networkBlocked };
    });
    is(result.own.ok, true, "product can fetch its own bundle assets");
    ok(result.networkBlocked, "fetch to the web is blocked by the CSP");
  });
});

// Products get host-keyed content principals, so host state (TrUAPI storage,
// permissions) keys on a stable per-product origin and products cannot reach
// into each other. DOM localStorage is not asserted here: LSNG additionally
// requires parent-side client registration and QuotaManager persistence
// wiring for custom schemes (follow-up work); products use the TrUAPI
// local-storage host functions instead.
add_task(async function test_dotapp_origin_isolation() {
  // Principal objects stay valid after their tab closes; open the products
  // sequentially so two tab-close animations don't overlap test teardown.
  let principalA;
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", browser => {
    principalA = browser.browsingContext.currentWindowGlobal.documentPrincipal;
  });
  let principalB;
  await BrowserTestUtils.withNewTab("dot://product-b.dot/", browser => {
    principalB = browser.browsingContext.currentWindowGlobal.documentPrincipal;
  });

  is(principalA.origin, "dot://product-a.dot", "product-a keeps its origin");
  is(principalB.origin, "dot://product-b.dot", "product-b keeps its origin");
  ok(
    principalA.isContentPrincipal && principalB.isContentPrincipal,
    "products get real content principals, not opaque null principals"
  );
  ok(
    !principalA.subsumes(principalB) && !principalB.subsumes(principalA),
    "product principals do not subsume each other"
  );
});

add_task(async function test_dotapp_favicon() {
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", async browser => {
    const tab = gBrowser.getTabForBrowser(browser);
    await BrowserTestUtils.waitForCondition(
      () => tab.getAttribute("image"),
      "waiting for the tab favicon"
    );
    // SVG favicons are delivered as a data: URL (wrapped in
    // moz-remote-image: for rendering) — assert our bundle's bytes made it.
    const image = decodeURIComponent(tab.getAttribute("image"));
    ok(
      image.includes("data:image/svg+xml"),
      "tab favicon was served from the product bundle"
    );
  });
});

add_task(async function test_dotapp_gets_truapi_bridge() {
  await SpecialPowers.pushPrefEnv({
    set: [["epoca.useragent.enabled", true]],
  });
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", async browser => {
    const result = await SpecialPowers.spawn(browser, [], () => {
      const page = content.wrappedJSObject;
      return {
        mark: page.__HOST_WEBVIEW_MARK__ === true,
        port: page.__HOST_API_PORT__ !== undefined,
      };
    });
    ok(result.mark, "dotapp product sees the host webview mark");
    ok(result.port, "dotapp product sees the host api MessagePort");
  });
  await SpecialPowers.popPrefEnv();
});

add_task(async function test_dotapp_link_context_menu_has_zen_items() {
  await BrowserTestUtils.withNewTab("dot://product-a.dot/", async browser => {
    const menu = document.getElementById("contentAreaContextMenu");
    const shown = BrowserTestUtils.waitForEvent(menu, "popupshown");

    await BrowserTestUtils.synthesizeMouse(
      "#product-link",
      0,
      0,
      { type: "contextmenu", button: 2, centered: true },
      browser
    );
    await shown;

    const openInTab = document.getElementById("context-openlinkintab");
    ok(!openInTab.hidden, "dot links get the normal browser link menu");
    is(openInTab.label, "Open Link in New Tab", "browser link text is present");

    const splitLink = document.getElementById("context-zenSplitLink");
    ok(!splitLink.hidden, "dot links get the Zen split-link menu item");
    is(splitLink.label, "Split link to new tab", "Zen split-link text is present");

    const glanceLink = document.getElementById("context-zenOpenLinkInGlance");
    ok(!glanceLink.hidden, "dot links get the Zen glance menu item");
    is(
      glanceLink.label,
      "Open Link in Glance",
      "Zen glance-link text is present"
    );

    const hidden = BrowserTestUtils.waitForEvent(menu, "popuphidden");
    menu.hidePopup();
    await hidden;
  });
});

