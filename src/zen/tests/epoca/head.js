/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

const { PromiseTestUtils } = ChromeUtils.importESModule(
  "resource://testing-common/PromiseTestUtils.sys.mjs"
);

// Unrelated Zen noise: the unofficial brand is missing these fluent strings.
// menu-bookmark-tab fires for any test that opens a tab; the urlbar action
// labels fire for any test that opens the urlbar view.
PromiseTestUtils.allowMatchingRejectionsGlobally(/menu-bookmark-tab/);
PromiseTestUtils.allowMatchingRejectionsGlobally(/urlbar-result-action/);
