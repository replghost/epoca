# environments (vendored)

UserAgentKit environment bundles — the canonical network registry (chain
genesis hashes, RPC endpoints, and service endpoints) for a given deployment.

- Upstream: https://github.com/paritytech/useragent-kit
  (`crates/host-chain-core/environments/`)
- Files: `paseo-next-v2.json` (verbatim copy)
- Consumed by `EpocaChainService.ensureEnvironment()`, which derives the
  supported chains (`chains[*].genesis_hash` → `chains[*].rpc_urls`) and the
  statement-store endpoint (`services.statement_store.endpoints[0]`). The
  `epoca.chain.networks` pref merges on top as a per-genesis override.

To update: copy the bundle JSON from useragent-kit
`crates/host-chain-core/environments/<name>.json` here, keeping it verbatim.
Adding a new bundle requires listing it in `moz.build`
(`FINAL_TARGET_FILES.modules.epoca.environments`) and a `./mach build`.
