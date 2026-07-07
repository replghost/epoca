// Integration test: verify epoca can reach the configured product chain and
// drive the transport that products (browse.dot) use to load the on-chain app
// registry. Talks to the live network, so it lives outside the mochitest
// harness (which disables non-local connections). Run via `npm run test:contract`.
//
// Uses the exact endpoint configured in prefs/zen/epoca.yaml so the test and the
// browser stay in sync. Node 22+ provides a global WebSocket (no dependency).

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

function fail(msg) {
  console.error(`FAIL: ${msg}`);
  process.exit(1);
}

// Pull epoca.chain.networks ({genesisHex: [wssUrl,...]}) out of the pref yaml.
async function readNetworks() {
  const yaml = await readFile(
    join(repoRoot, "prefs/zen/epoca.yaml"),
    "utf8"
  );
  const m = yaml.match(
    /epoca\.chain\.networks[\s\S]*?value:\s*'([^']+)'/
  );
  if (!m) {
    fail("could not find epoca.chain.networks in prefs/zen/epoca.yaml");
  }
  return JSON.parse(m[1]);
}

const networks = await readNetworks();
const genesis = Object.keys(networks)[0];
const url = networks[genesis][0];
console.log(`configured chain: ${genesis}\n  endpoint: ${url}`);

const ws = new WebSocket(url);
let nextId = 1;
const pending = new Map();
const subs = new Map();

ws.addEventListener("message", ev => {
  const m = JSON.parse(ev.data.toString());
  if (m.id !== undefined && pending.has(m.id)) {
    const r = pending.get(m.id);
    pending.delete(m.id);
    r(m);
    return;
  }
  // Match notifications by method so the handler is ready before the follow
  // response is awaited — the initialized event can arrive first.
  if (m.method) {
    for (const handler of subs.values()) {
      handler(m);
    }
  }
});

function rpc(method, params = []) {
  const id = nextId++;
  ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
  return new Promise(resolve => pending.set(id, resolve));
}

const connectTimer = setTimeout(() => fail(`timed out connecting to ${url}`), 20000);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve, { once: true });
  ws.addEventListener("error", () => reject(new Error("websocket error")), {
    once: true,
  });
}).catch(e => fail(e.message));
clearTimeout(connectTimer);
console.log("PASS: connected");

// 1. Genesis matches the configured network.
const genResp = await rpc("chain_getBlockHash", [0]);
if (genResp.result?.toLowerCase() !== genesis.toLowerCase()) {
  fail(`genesis mismatch: chain=${genResp.result} configured=${genesis}`);
}
console.log("PASS: live genesis matches configured network");

// 2. Runtime reachable.
const rv = await rpc("state_getRuntimeVersion");
if (!rv.result?.specName) {
  fail(`state_getRuntimeVersion returned no specName: ${JSON.stringify(rv)}`);
}
console.log(`PASS: runtime reachable (${rv.result.specName} ${rv.result.specVersion})`);

// 3. chainHead follow reaches a finalized block — the subscription products
//    open to read the app-registry contract state. Register the initialized
//    waiter BEFORE subscribing, since the event can arrive before the follow
//    response resolves.
const initializedPromise = new Promise((resolve, reject) => {
  const t = setTimeout(
    () => reject(new Error("no chainHead initialized event within 30s")),
    30000
  );
  subs.set("chainHead", m => {
    if (
      m.method === "chainHead_v1_followEvent" &&
      m.params?.result?.event === "initialized"
    ) {
      clearTimeout(t);
      resolve(m.params.result);
    }
  });
});
const follow = await rpc("chainHead_v1_follow", [true]);
if (typeof follow.result !== "string") {
  fail(`chainHead_v1_follow did not return a subscription id: ${JSON.stringify(follow)}`);
}
const initialized = await initializedPromise.catch(e => fail(e.message));
const blocks =
  initialized.finalizedBlockHashes ??
  (initialized.finalizedBlockHash ? [initialized.finalizedBlockHash] : []);
if (!blocks.length) {
  fail(`chainHead initialized without a finalized block: ${JSON.stringify(initialized)}`);
}
console.log(`PASS: chainHead follow reached finalized block ${blocks[0]}`);

ws.close();
console.log("\nALL CONTRACT-LOAD CHECKS PASSED");
process.exit(0);
