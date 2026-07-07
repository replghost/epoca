/* Any copyright is dedicated to the Public Domain.
   https://creativecommons.org/publicdomain/zero/1.0/ */

"use strict";

// Guards the configuration products (e.g. browse.dot) depend on to load the
// on-chain app registry: a chain network must be configured with a valid
// genesis and a wss endpoint, and EpocaChainService must expose it. The live
// end-to-end reachability + chainHead-follow check runs against the real
// network in `npm run test:contract` (scripts/test-contract.mjs), which lives
// outside this harness because mochitest disables non-local connections.

const { EpocaChainService } = ChromeUtils.importESModule(
  "resource:///modules/EpocaChainService.sys.mjs"
);

add_task(function test_chain_network_configured() {
  const networks = JSON.parse(
    Services.prefs.getStringPref("epoca.chain.networks", "{}")
  );
  const genesisHashes = Object.keys(networks);
  ok(genesisHashes.length, "at least one chain network is configured");

  const genesis = genesisHashes[0];
  ok(/^0x[0-9a-f]{64}$/i.test(genesis), "configured genesis is a 32-byte hash");

  const urls = networks[genesis];
  ok(
    Array.isArray(urls) && urls.some(u => /^wss?:\/\//.test(u)),
    "network has a websocket RPC endpoint"
  );

  // The engine advertises this network so chain queries route to it.
  const supported = EpocaChainService.supportedGenesisHashes().map(bytes =>
    "0x" + Array.from(bytes, b => b.toString(16).padStart(2, "0")).join("")
  );
  ok(
    supported.some(h => h.toLowerCase() === genesis.toLowerCase()),
    "EpocaChainService advertises the configured genesis"
  );
});
