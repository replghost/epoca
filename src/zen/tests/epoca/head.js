/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

const { PromiseTestUtils } = ChromeUtils.importESModule(
  "resource://testing-common/PromiseTestUtils.sys.mjs"
);

// Unrelated Zen noise: the unofficial brand is missing this fluent string,
// and the rejection fires for any test that opens a tab.
PromiseTestUtils.allowMatchingRejectionsGlobally(/menu-bookmark-tab/);
