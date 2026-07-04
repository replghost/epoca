// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Defense-in-depth network lockdown for dot products, independent of the
// CSP injected into their documents: any load of a network URL that a
// dot principal initiates (or that loads into a dot document) is
// rejected. Covers navigations too, so a product cannot exfiltrate by
// redirecting its tab to a web URL.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  imageGatewayOrigins: "resource:///modules/EpocaDotAppRegistry.sys.mjs",
});

const NETWORK_SCHEMES = new Set(["http", "https", "ws", "wss", "ftp"]);

const IMAGE_TYPES = new Set([
  Ci.nsIContentPolicy.TYPE_IMAGE,
  Ci.nsIContentPolicy.TYPE_IMAGESET,
]);

export class EpocaDotAppContentPolicy {
  shouldLoad(contentLocation, loadInfo) {
    try {
      if (NETWORK_SCHEMES.has(contentLocation.scheme)) {
        const { loadingPrincipal, triggeringPrincipal } = loadInfo;
        if (
          loadingPrincipal?.schemeIs("dot") ||
          triggeringPrincipal?.schemeIs("dot")
        ) {
          // Single exception, matching the injected CSP: content-addressed
          // images from the configured IPFS gateways.
          if (
            IMAGE_TYPES.has(loadInfo.externalContentPolicyType) &&
            lazy
              .imageGatewayOrigins()
              .includes(`${contentLocation.scheme}://${contentLocation.host}`)
          ) {
            return Ci.nsIContentPolicy.ACCEPT;
          }
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
