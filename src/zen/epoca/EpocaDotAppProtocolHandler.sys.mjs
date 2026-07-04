// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

// Protocol handler for dot://<name>.dot/<path>. Serves product bundles
// from the parent-process EpocaDotAppRegistry. Under Fission, document loads
// open the channel in the parent while subresource loads open it in the
// content process, so both paths are handled: a direct registry lookup in
// the parent, an EpocaDotApp process-actor query from content.
//
// The channel owner is deliberately left null so Gecko derives a content
// principal from the (standard, host-bearing) URI — that is what gives each
// product its own isolated, persistent origin.

const lazy = {};

ChromeUtils.defineESModuleGetters(lazy, {
  EpocaDotAppRegistry: "resource:///modules/EpocaDotAppRegistry.sys.mjs",
});

export class EpocaDotAppProtocolHandler {
  scheme = "dot";

  allowPort() {
    return false;
  }

  newChannel(uri, loadInfo) {
    const channel = Cc["@mozilla.org/network/input-stream-channel;1"]
      .createInstance(Ci.nsIInputStreamChannel)
      .QueryInterface(Ci.nsIChannel);
    channel.setURI(uri);
    channel.originalURI = uri;
    channel.loadInfo = loadInfo;

    const wrapper = Services.io.newSuspendableChannelWrapper(channel);
    wrapper.suspend();

    this.#resolve(uri, loadInfo)
      .then(asset => {
        if (asset?.inputStream) {
          channel.contentStream = asset.inputStream;
          channel.contentType = asset.contentType;
          channel.contentCharset = "utf-8";
          wrapper.resume();
        } else {
          this.#fail(channel, wrapper, Cr.NS_ERROR_FILE_NOT_FOUND);
        }
      })
      .catch(e => {
        console.error(`dotapp: failed to resolve ${uri.spec}`, e);
        this.#fail(channel, wrapper, Cr.NS_ERROR_FAILURE);
      });

    return wrapper;
  }

  // The inner channel cannot open without a content stream
  // (nsInputStreamChannel requires one), and a wrapper that never opens
  // leaves its consumer waiting forever. Feed an empty stream, let the
  // resume open the channel, then cancel so the listener is notified of
  // the failure.
  #fail(channel, wrapper, status) {
    try {
      const empty = Cc[
        "@mozilla.org/io/arraybuffer-input-stream;1"
      ].createInstance(Ci.nsIArrayBufferInputStream);
      empty.setData(new ArrayBuffer(0), 0, 0);
      channel.contentStream = empty;
      channel.contentType = "text/plain";
      wrapper.resume();
    } finally {
      channel.cancel(status);
    }
  }

  async #resolve(uri, loadInfo) {
    const browsingContextId = loadInfo?.browsingContext?.id;
    if (
      Services.appinfo.processType === Ci.nsIXULRuntime.PROCESS_TYPE_DEFAULT
    ) {
      return lazy.EpocaDotAppRegistry.resolve(uri, browsingContextId);
    }
    const actor = ChromeUtils.domProcessChild.getActor("EpocaDotApp");
    return actor.sendQuery("EpocaDotApp:GetAsset", {
      spec: uri.spec,
      browsingContextId,
    });
  }

  QueryInterface = ChromeUtils.generateQI(["nsIProtocolHandler"]);
}
