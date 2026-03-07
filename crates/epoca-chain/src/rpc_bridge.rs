//! Shared RPC bridge between SPA apps and chain client threads.
//!
//! This module provides the channel infrastructure for routing JSON-RPC
//! requests from sandboxed apps to the smoldot/RPC thread, and responses back.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::ChainId;

/// Pending RPC request from an SPA app → chain client thread.
#[derive(Debug)]
pub struct RpcRequest {
    pub chain: ChainId,
    pub corr_id: u64,
    pub json_rpc: String,
}

/// Global request queue. Chain threads poll this for their chain.
static RPC_REQUESTS: OnceLock<Mutex<Vec<RpcRequest>>> = OnceLock::new();

fn rpc_requests() -> &'static Mutex<Vec<RpcRequest>> {
    RPC_REQUESTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// RPC response from chain thread → workbench drain loop.
#[derive(Debug)]
pub struct RpcResponse {
    pub corr_id: u64,
    pub result: String,
}

static RPC_RESPONSE_CHANNEL: OnceLock<(
    std::sync::mpsc::SyncSender<RpcResponse>,
    Mutex<std::sync::mpsc::Receiver<RpcResponse>>,
)> = OnceLock::new();

fn response_channel() -> &'static (
    std::sync::mpsc::SyncSender<RpcResponse>,
    Mutex<std::sync::mpsc::Receiver<RpcResponse>>,
) {
    RPC_RESPONSE_CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1024);
        (tx, Mutex::new(rx))
    })
}

/// Correlation map: corr_id → (webview_ptr, js_id).
static CORRELATION: OnceLock<Mutex<HashMap<u64, (usize, u64)>>> = OnceLock::new();

fn correlation() -> &'static Mutex<HashMap<u64, (usize, u64)>> {
    CORRELATION.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// App-side (called from epoca-core workbench)
// ---------------------------------------------------------------------------

/// Enqueue an RPC request for a specific chain.
pub fn enqueue_request(chain: ChainId, corr_id: u64, json_rpc: String) {
    rpc_requests()
        .lock()
        .unwrap()
        .push(RpcRequest { chain, corr_id, json_rpc });
}

/// Register a correlation between a corr_id and the originating webview/JS id.
pub fn register_correlation(corr_id: u64, webview_ptr: usize, js_id: u64) {
    correlation()
        .lock()
        .unwrap()
        .insert(corr_id, (webview_ptr, js_id));
}

/// Drain pending RPC responses.
/// Returns Vec of (webview_ptr, js_id, raw_json_rpc_response).
pub fn drain_responses() -> Vec<(usize, u64, String)> {
    let ch = response_channel();
    let rx = ch.1.lock().unwrap();
    let mut results = Vec::new();

    while let Ok(resp) = rx.try_recv() {
        if let Some((webview_ptr, js_id)) = correlation().lock().unwrap().remove(&resp.corr_id) {
            results.push((webview_ptr, js_id, resp.result));
        }
    }

    results
}

/// Clean up correlations for a closed webview.
pub fn cleanup_for_webview(webview_ptr: usize) {
    correlation()
        .lock()
        .unwrap()
        .retain(|_, (ptr, _)| *ptr != webview_ptr);
}

// ---------------------------------------------------------------------------
// Chain-thread-side (called from smoldot/RPC event loops)
// ---------------------------------------------------------------------------

/// Take all pending requests for a specific chain.
pub fn take_requests_for_chain(chain: ChainId) -> Vec<RpcRequest> {
    let mut queue = rpc_requests().lock().unwrap();
    let mut taken = Vec::new();
    let mut remaining = Vec::new();
    for req in queue.drain(..) {
        if req.chain == chain {
            taken.push(req);
        } else {
            remaining.push(req);
        }
    }
    *queue = remaining;
    taken
}

/// Push a response from the chain thread.
pub fn push_response(corr_id: u64, result: String) {
    let _ = response_channel()
        .0
        .try_send(RpcResponse { corr_id, result });
}

/// Check if a JSON-RPC response ID belongs to an app request.
pub fn is_app_request(json_rpc_id: u64) -> bool {
    correlation().lock().unwrap().contains_key(&json_rpc_id)
}
