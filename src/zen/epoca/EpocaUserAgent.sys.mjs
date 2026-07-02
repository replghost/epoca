// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Entry point for epoca's user-agent (host) functionality: registers the
// actors that bridge product (SPA) pages to the host-side TrUAPI engine.

import { ActorManagerParent } from "resource://gre/modules/ActorManagerParent.sys.mjs";

const JSWINDOWACTORS = {
  EpocaProduct: {
    parent: {
      esModuleURI: "resource:///actors/EpocaProductParent.sys.mjs",
    },
    child: {
      esModuleURI: "resource:///actors/EpocaProductChild.sys.mjs",
      events: {
        // Fires when the document element is inserted, before any page
        // script runs — the document-start equivalent for JSWindowActors.
        DOMDocElementInserted: {},
      },
    },
    // PoC scope: any http(s) page while the pref is flipped. Once the
    // dotapp:// scheme lands, this narrows to product origins only.
    matches: ["https://*/*", "http://*/*"],
    enablePreference: "epoca.useragent.enabled",
  },
};

export let gEpocaUserAgent = {
  init() {
    ActorManagerParent.addJSWindowActors(JSWINDOWACTORS);
  },
};
