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
    // No `matches`: MatchPattern cannot express host-bearing custom schemes
    // (dotapp is not in its HostLocatorSchemes, so patterns parse as path
    // globs and MatchesDomain rejects any URI with a host). The child actor
    // gates on the document scheme instead.
    enablePreference: "epoca.useragent.enabled",
  },
};

const JSPROCESSACTORS = {
  // Asset pipe for the dotapp protocol handler: content-process subresource
  // channels query the parent-side product registry through this actor.
  EpocaDotApp: {
    parent: {
      esModuleURI: "resource:///actors/EpocaDotAppParent.sys.mjs",
    },
    child: {
      esModuleURI: "resource:///actors/EpocaDotAppChild.sys.mjs",
    },
  },
};

export let gEpocaUserAgent = {
  init() {
    ActorManagerParent.addJSWindowActors(JSWINDOWACTORS);
    ActorManagerParent.addJSProcessActors(JSPROCESSACTORS);
  },
};
