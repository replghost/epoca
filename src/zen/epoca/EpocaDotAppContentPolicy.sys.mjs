// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Defense-in-depth network lockdown for dotapp products, independent of the
// CSP injected into their documents: any load of a network URL that a
// dotapp principal initiates (or that loads into a dotapp document) is
// rejected. Covers navigations too, so a product cannot exfiltrate by
// redirecting its tab to a web URL.

const NETWORK_SCHEMES = new Set(["http", "https", "ws", "wss", "ftp"]);

export class EpocaDotAppContentPolicy {
  shouldLoad(contentLocation, loadInfo) {
    try {
      if (NETWORK_SCHEMES.has(contentLocation.scheme)) {
        const { loadingPrincipal, triggeringPrincipal } = loadInfo;
        if (
          loadingPrincipal?.schemeIs("dotapp") ||
          triggeringPrincipal?.schemeIs("dotapp")
        ) {
          return Ci.nsIContentPolicy.REJECT_REQUEST;
        }
      }
    } catch {
      // Never let a policy failure take down unrelated loads.
    }
    return Ci.nsIContentPolicy.ACCEPT;
  }

  shouldProcess() {
    return Ci.nsIContentPolicy.ACCEPT;
  }

  QueryInterface = ChromeUtils.generateQI(["nsIContentPolicy"]);
}
