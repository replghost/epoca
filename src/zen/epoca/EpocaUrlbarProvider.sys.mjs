// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// URL bar provider for dot names: typing "browse.dot" (optionally with a
// path, or as "dot://browse.dot") offers a heuristic result that navigates
// to dot://browse.dot/, where the protocol handler serves the bundle
// (resolving it via dotNS first if it isn't registered yet).

import {
  UrlbarProvider,
  UrlbarUtils,
} from "moz-src:///browser/components/urlbar/UrlbarUtils.sys.mjs";

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  UrlbarResult: "moz-src:///browser/components/urlbar/UrlbarResult.sys.mjs",
});

const ENABLED_PREF = "epoca.useragent.enabled";

// "browse.dot", "browse.dot/route", "dot://browse/route" — a dotNS label
// followed by an optional path.
const DOT_INPUT_RE =
  /^(?:dot:\/\/)?([a-z0-9][a-z0-9-]{0,63})(?:\.dot)?(\/\S*)?$/i;

/**
 * Map URL bar input to a canonical dot://<label>.dot URL, or null when the
 * input is not a dot name. The bare-label form (no ".dot", no "dot://") is
 * rejected so normal hostnames and search terms are unaffected.
 *
 * @param {string} input
 * @returns {string|null}
 */
export function dotInputToDotAppUrl(input) {
  const match = DOT_INPUT_RE.exec(input.trim());
  if (!match) {
    return null;
  }
  const explicit =
    /^dot:\/\//i.test(input.trim()) || /\.dot(\/|$)/i.test(input.trim());
  if (!explicit) {
    return null;
  }
  const label = match[1].toLowerCase();
  const path = match[2] || "/";
  return `dot://${label}.dot${path}`;
}

export class EpocaUrlbarProviderDotNames extends UrlbarProvider {
  get name() {
    return "EpocaUrlbarProviderDotNames";
  }

  get type() {
    return UrlbarUtils.PROVIDER_TYPE.HEURISTIC;
  }

  async isActive(queryContext) {
    return (
      Services.prefs.getBoolPref(ENABLED_PREF, false) &&
      !queryContext.searchMode &&
      !!dotInputToDotAppUrl(queryContext.searchString)
    );
  }

  // Own the whole result set for dot-name input: providers with the highest
  // priority run exclusively, so no search/visit heuristic competes.
  getPriority() {
    return 10;
  }

  startQuery(queryContext, addCallback) {
    const url = dotInputToDotAppUrl(queryContext.searchString);
    if (!url) {
      return;
    }
    const result = new lazy.UrlbarResult({
      type: UrlbarUtils.RESULT_TYPE.URL,
      source: UrlbarUtils.RESULT_SOURCE.OTHER_LOCAL,
      heuristic: true,
      payload: {
        url,
        title: `Open ${new URL(url).host}`,
        icon: "chrome://global/skin/icons/defaultFavicon.svg",
      },
    });
    addCallback(this, result);
  }
}
