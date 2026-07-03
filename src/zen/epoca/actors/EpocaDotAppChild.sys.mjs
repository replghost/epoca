// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Content half of the dotapp asset pipe. The protocol handler obtains this
// actor via ChromeUtils.domProcessChild.getActor and sendQuery()s the
// parent; no child-side logic is needed.

export class EpocaDotAppChild extends JSProcessActorChild {}
