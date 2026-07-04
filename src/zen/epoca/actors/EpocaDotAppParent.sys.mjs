// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent half of the dotapp asset pipe: answers content-process protocol
// handler queries from the in-memory product registry. The input stream in
// the reply crosses process boundaries via structured clone.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaDotAppRegistry: "resource:///modules/EpocaDotAppRegistry.sys.mjs",
});

export class EpocaDotAppParent extends JSProcessActorParent {
  receiveMessage(message) {
    if (message.name === "EpocaDotApp:GetAsset") {
      return lazy.EpocaDotAppRegistry.resolve(
        message.data.spec,
        message.data.browsingContextId
      );
    }
    return null;
  }
}
