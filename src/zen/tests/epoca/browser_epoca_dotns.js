/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Validates the dot-name URL bar flow: input mapping, the heuristic urlbar
// result for "<name>.dot", and the registry's dotNS fallback (stubbed — no
// network in CI) that resolves unknown products on first navigation.

const { UrlbarTestUtils } = ChromeUtils.importESModule(
  "resource://testing-common/UrlbarTestUtils.sys.mjs"
);
const { UrlbarUtils } = ChromeUtils.importESModule(
  "moz-src:///browser/components/urlbar/UrlbarUtils.sys.mjs"
);
const { dotInputToDotAppUrl } = ChromeUtils.importESModule(
  "resource:///modules/EpocaUrlbarProvider.sys.mjs"
);
const { EpocaDotNs } = ChromeUtils.importESModule(
  "resource:///modules/EpocaDotNs.sys.mjs"
);
const { EpocaDotAppRegistry } = ChromeUtils.importESModule(
  "resource:///modules/EpocaDotAppRegistry.sys.mjs"
);

add_task(function test_dot_input_mapping() {
  const cases = [
    ["browse.dot", "dot://browse.dot/"],
    ["browse.dot/settings", "dot://browse.dot/settings"],
    ["dot://browse", "dot://browse.dot/"],
    ["dot://browse/a/b", "dot://browse.dot/a/b"],
    ["Browse.DOT", "dot://browse.dot/"],
    ["browse", null],
    ["example.com", null],
    ["browse.dots", null],
    ["https://browse.dot/", null],
    ["hello world.dot", null],
  ];
  for (const [input, expected] of cases) {
    is(dotInputToDotAppUrl(input), expected, `mapping of "${input}"`);
  }
});

add_task(async function test_dot_name_urlbar_result() {
  await SpecialPowers.pushPrefEnv({
    set: [["epoca.useragent.enabled", true]],
  });

  // Assert on the query context rather than the rendered row:
  // getDetailsOfResultAt translates the row's fluent fragment, which throws
  // on the unofficial brand's missing urlbar-result-action-* strings.
  const context = await UrlbarTestUtils.promiseAutocompleteResultPopup({
    window,
    waitForFocus: SimpleTest.waitForFocus,
    value: "browse.dot",
  });

  const result = context.results[0];
  is(
    result.type,
    UrlbarUtils.RESULT_TYPE.URL,
    "dot-name input yields a URL result"
  );
  ok(result.heuristic, "the dot-name result is the heuristic result");
  is(
    result.payload.url,
    "dot://browse.dot/",
    "result navigates to the dotapp URL"
  );

  await UrlbarTestUtils.promisePopupClose(window);
  await SpecialPowers.popPrefEnv();
});

add_task(async function test_dotns_fallback_resolves_unknown_product() {
  await SpecialPowers.pushPrefEnv({
    set: [["epoca.dotapp.dotns.enabled", true]],
  });

  // Stub the resolver: the module is a parent-process singleton, so the
  // registry sees this patched method.
  const originalResolve = EpocaDotNs.resolve;
  let resolvedNames = [];
  EpocaDotNs.resolve = async name => {
    resolvedNames.push(name);
    return {
      "index.html": new TextEncoder().encode(
        `<!DOCTYPE html><html><head><title>via-dotns</title></head><body></body></html>`
      ),
    };
  };
  registerCleanupFunction(() => {
    EpocaDotNs.resolve = originalResolve;
    EpocaDotAppRegistry.unregister("dotns-product");
  });

  await BrowserTestUtils.withNewTab("dot://dotns-product.dot/", async browser => {
    const title = await SpecialPowers.spawn(
      browser,
      [],
      () => content.document.title
    );
    is(title, "via-dotns", "unknown product was resolved and served via dotNS");
  });

  Assert.deepEqual(
    resolvedNames,
    ["dotns-product"],
    "resolver was invoked exactly once with the product id"
  );

  // Second load must come from the registry cache, not another resolution.
  await BrowserTestUtils.withNewTab("dot://dotns-product.dot/", async browser => {
    const title = await SpecialPowers.spawn(
      browser,
      [],
      () => content.document.title
    );
    is(title, "via-dotns", "cached product serves without re-resolving");
  });
  is(resolvedNames.length, 1, "no second dotNS resolution");

  await SpecialPowers.popPrefEnv();
});
