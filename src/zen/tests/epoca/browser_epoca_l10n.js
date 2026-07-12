/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

"use strict";

// Regression guard for the fork's total-blank-chrome bug.
//
// browser.xhtml declares <link rel="localization"> for several Zen ftls
// (zen-workspaces, zen-general, zen-split-view, ...). If any of those en-US
// files are missing from the packaged localization, the sync Fluent bundle
// solver fails to build ANY complete bundle for the window, leaving
// document.l10n empty -> every data-l10n-id element in the whole chrome UI
// (settings, menus, context menus) renders blank. The en-US zen ftls must be
// present under browser/locales/en-US/browser/ (packaged via that dir's
// jar.mn glob). This test fails loudly if they go missing again.

add_task(async function test_window_l10n_is_alive() {
  // A core Firefox string must resolve through the window's own document.l10n
  // (not a freshly-constructed Localization, which would mask the bug).
  const core = await document.l10n.formatValue(
    "browser-main-window-default-title"
  );
  ok(core, `core browser string resolves via document.l10n: "${core}"`);
});

add_task(async function test_zen_ftls_are_packaged() {
  // One value-bearing message from each Zen ftl referenced by browser.xhtml's
  // localization links. A missing ftl here is exactly what poisons the whole
  // window, so assert each resolves through document.l10n.
  const zenIds = [
    "zen-panel-ui-current-profile-text", // browser/zen-general.ftl
    "zen-panel-ui-workspaces-text", // browser/zen-workspaces.ftl
    "zen-split-view-modifier-header", // browser/zen-split-view.ftl
  ];
  for (const id of zenIds) {
    const val = await document.l10n.formatValue(id);
    ok(val, `zen l10n string "${id}" resolves via document.l10n: "${val}"`);
  }
});

add_task(async function test_chrome_is_not_wholesale_blank() {
  // Guardrail against the systemic failure: if the window l10n breaks, nearly
  // all data-l10n-id elements go blank. In a healthy window the vast majority
  // carry text/label. Some are legitimately empty (conditionally shown, or
  // attribute-only in ways textContent doesn't capture), so allow a margin.
  const els = Array.from(document.querySelectorAll("[data-l10n-id]"));
  const blank = els.filter(
    e => !(e.textContent || "").trim() && !e.getAttribute("label")
  );
  Assert.greater(els.length, 100, "sanity: many localized elements present");
  Assert.less(
    blank.length,
    els.length / 2,
    `fewer than half of ${els.length} localized elements are blank ` +
      `(blank=${blank.length}); a wholesale-blank chrome means window l10n broke`
  );
});
