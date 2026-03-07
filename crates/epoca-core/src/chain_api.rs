//! Chain API for sandboxed SPA tabs.
//!
//! Apps call `window.epoca.chain.query(method, params)` which routes through
//! the host's light client. This module handles permission checks, method
//! allowlisting, and routing between the workbench drain loop and the
//! chain client threads via `epoca_chain::rpc_bridge`.

use std::sync::atomic::{AtomicU64, Ordering};

use epoca_chain::ChainId;

/// Monotonic correlation ID for tracking requests across threads.
static NEXT_CORR_ID: AtomicU64 = AtomicU64::new(1);

/// Read-only RPC methods that apps are allowed to call.
const ALLOWED_QUERY_METHODS: &[&str] = &[
    "state_getStorage",
    "state_getStorageHash",
    "state_getStorageSize",
    "state_call",
    "state_getRuntimeVersion",
    "state_getMetadata",
    "state_getKeys",
    "state_getKeysPaged",
    "chain_getBlock",
    "chain_getBlockHash",
    "chain_getFinalizedHead",
    "chain_getHead",
    "chain_getHeader",
    "system_chain",
    "system_name",
    "system_version",
    "system_properties",
    "system_health",
    "rpc_methods",
    "chainSpec_v1_chainName",
    "chainSpec_v1_genesisHash",
    "chainSpec_v1_properties",
    "system_accountNextIndex",
    "payment_queryInfo",
];

/// Parse a chain name string from manifest into a ChainId.
pub fn parse_chain_id(s: &str) -> Option<ChainId> {
    match s {
        "PolkadotAssetHub" | "polkadot-asset-hub" => Some(ChainId::PolkadotAssetHub),
        "PaseoAssetHub" | "paseo-asset-hub" => Some(ChainId::PaseoAssetHub),
        "Previewnet" | "previewnet" => Some(ChainId::Previewnet),
        "Ethereum" | "ethereum" => Some(ChainId::Ethereum),
        "Bitcoin" | "bitcoin" => Some(ChainId::Bitcoin),
        _ => None,
    }
}

/// Submit a chain query from an SPA app.
/// Returns an error string if the method is not allowed.
pub fn submit_query(
    chain: ChainId,
    webview_ptr: usize,
    js_id: u64,
    method: &str,
    params: &serde_json::Value,
) -> Result<(), String> {
    if !ALLOWED_QUERY_METHODS.contains(&method) {
        return Err(format!("method '{method}' not permitted"));
    }

    let corr_id = NEXT_CORR_ID.fetch_add(1, Ordering::Relaxed);

    let json_rpc = serde_json::json!({
        "jsonrpc": "2.0",
        "id": corr_id,
        "method": method,
        "params": params,
    })
    .to_string();

    epoca_chain::rpc_bridge::register_correlation(corr_id, webview_ptr, js_id);
    epoca_chain::rpc_bridge::enqueue_request(chain, corr_id, json_rpc);

    log::info!("[chain-api] query {method} corr={corr_id} for webview {webview_ptr:#x}");
    Ok(())
}

/// Submit a pre-approved extrinsic to the chain.
/// Called only after the user approves via the approval dialog.
/// Bypasses the query allowlist since the user explicitly approved.
pub fn submit_extrinsic(
    chain: ChainId,
    webview_ptr: usize,
    js_id: u64,
    call_data: &str,
) -> Result<(), String> {
    let corr_id = NEXT_CORR_ID.fetch_add(1, Ordering::Relaxed);

    let json_rpc = serde_json::json!({
        "jsonrpc": "2.0",
        "id": corr_id,
        "method": "author_submitExtrinsic",
        "params": [call_data],
    })
    .to_string();

    epoca_chain::rpc_bridge::register_correlation(corr_id, webview_ptr, js_id);
    epoca_chain::rpc_bridge::enqueue_request(chain, corr_id, json_rpc);

    log::info!("[chain-api] submit extrinsic corr={corr_id} for webview {webview_ptr:#x}");
    Ok(())
}

/// Drain pending RPC responses (called from workbench render loop).
/// Returns Vec of (webview_ptr, js_id, result_or_error).
pub fn drain_responses() -> Vec<(usize, u64, Result<serde_json::Value, String>)> {
    let raw = epoca_chain::rpc_bridge::drain_responses();
    raw.into_iter()
        .map(|(webview_ptr, js_id, raw_json)| {
            let parsed = match serde_json::from_str::<serde_json::Value>(&raw_json) {
                Ok(v) => {
                    if let Some(error) = v.get("error") {
                        Err(error.to_string())
                    } else if let Some(result) = v.get("result") {
                        Ok(result.clone())
                    } else {
                        Err("unexpected response format".into())
                    }
                }
                Err(e) => Err(format!("parse error: {e}")),
            };
            (webview_ptr, js_id, parsed)
        })
        .collect()
}

/// Clean up any pending correlations for a closed webview.
pub fn cleanup_for_webview(webview_ptr: usize) {
    epoca_chain::rpc_bridge::cleanup_for_webview(webview_ptr);
}
