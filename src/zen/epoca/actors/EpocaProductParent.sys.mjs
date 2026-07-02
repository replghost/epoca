// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Parent-process half of the product bridge. Receives byte frames from the
// content actor and dispatches them to the host engine.
//
// PoC placeholder: speaks a trivial JSON-over-UTF-8 protocol so the
// round-trip can be validated end to end. The real implementation replaces
// #dispatch with the SCALE-frame TrUAPI engine (useragent-kit host-wasm).

export class EpocaProductParent extends JSWindowActorParent {
  receiveMessage(message) {
    if (message.name !== "EpocaProduct:Frame") {
      return;
    }

    let response;
    try {
      const request = JSON.parse(new TextDecoder().decode(message.data));
      response = this.#dispatch(request);
    } catch (e) {
      response = { error: { code: "malformed-frame", message: e.message } };
    }

    this.sendAsyncMessage(
      "EpocaProduct:HostFrame",
      new TextEncoder().encode(JSON.stringify(response))
    );
  }

  #dispatch(request) {
    switch (request.method) {
      case "epoca.handshake":
        return {
          id: request.id,
          result: {
            host: "epoca",
            hostVersion: "0.1.0",
            protocol: "poc-json-v0",
          },
        };
      default:
        return {
          id: request.id,
          error: { code: "unknown-method", method: request.method },
        };
    }
  }
}
