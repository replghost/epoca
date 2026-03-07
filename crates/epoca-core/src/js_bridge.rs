//! JS bridge dispatch — typed routing for `window.epoca.*` calls.
//!
//! Extracts the untyped string-match dispatch from the workbench render loop
//! into a standalone, testable module. No GPUI dependency.

/// Parsed host API request with typed parameters.
#[derive(Debug)]
pub enum BridgeRequest {
    GetAddress,
    Sign { payload: String },
    ChainQuery { method: String, rpc_params: serde_json::Value },
    ChainSubmit { call_data: String },
    StatementsWrite { channel: String, data: String },
    StatementsSubscribe { channel: String },
    DataConnect { peer_address: String },
    DataSend { conn_id: u64, data: String },
    DataClose { conn_id: u64 },
}

/// Permission context needed for dispatch decisions.
pub struct BridgePermissions {
    pub wallet_enabled: bool,
    pub chain: bool,
    pub statements: bool,
    pub data: bool,
}

/// Result of dispatching a bridge request.
pub enum BridgeResult {
    /// Evaluate this JS string on the source webview.
    Js(String),
    /// Request requires async work — caller handles it.
    Async(BridgeAsyncAction),
    /// Method not recognized.
    UnknownMethod(String),
}

/// Async actions that the workbench must handle (dialogs, background work).
pub enum BridgeAsyncAction {
    Sign { payload: String },
    ChainQuery { method: String, rpc_params: serde_json::Value, chain: String },
    ChainSubmit { call_data: String, chain: String },
    DataConnect { peer_address: String, conn_id: u64 },
}

/// Format a JS resolve call for success.
fn resolve_ok(id: u64, result: &str) -> String {
    format!("window.__epocaResolve({id}, null, {result})")
}

/// Format a JS resolve call for error.
fn resolve_err(id: u64, error: &str) -> String {
    let escaped = error.replace('\'', "\\'");
    format!("window.__epocaResolve({id}, '{escaped}', null)")
}

/// Parse the method + params JSON into a typed request.
pub fn parse_request(method: &str, params_json: &str) -> Result<BridgeRequest, String> {
    let params: serde_json::Value =
        serde_json::from_str(params_json).unwrap_or(serde_json::Value::Object(Default::default()));

    match method {
        "getAddress" => Ok(BridgeRequest::GetAddress),
        "sign" => {
            let payload = params
                .get("payload")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::Sign { payload })
        }
        "chainQuery" => {
            let method = params
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let rpc_params = params
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Array(vec![]));
            Ok(BridgeRequest::ChainQuery { method, rpc_params })
        }
        "chainSubmit" => {
            let call_data = params
                .get("callData")
                .map(|v| serde_json::to_string(v).unwrap_or_default())
                .unwrap_or_default();
            Ok(BridgeRequest::ChainSubmit { call_data })
        }
        "statementsWrite" => {
            let channel = params
                .get("channel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = params
                .get("data")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| params_json.to_string());
            Ok(BridgeRequest::StatementsWrite { channel, data })
        }
        "statementsSubscribe" => {
            let channel = params
                .get("channel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::StatementsSubscribe { channel })
        }
        "dataConnect" => {
            let peer = params
                .get("peerAddress")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::DataConnect { peer_address: peer })
        }
        "dataSend" => {
            let conn_id = params.get("connId").and_then(|v| v.as_u64()).unwrap_or(0);
            let data = params
                .get("data")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::DataSend { conn_id, data })
        }
        "dataClose" => {
            let conn_id = params.get("connId").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(BridgeRequest::DataClose { conn_id })
        }
        other => Err(other.to_string()),
    }
}

/// Dispatch a parsed request. Returns JS to evaluate or an async action.
pub fn dispatch(
    req: &BridgeRequest,
    app_id: &str,
    id: u64,
    webview_ptr: usize,
    chain: &str,
    perms: &BridgePermissions,
    wallet_address: Result<String, String>,
    author: &str,
    has_pending_sign: bool,
    has_pending_submit: bool,
    has_pending_connect: bool,
) -> BridgeResult {
    match req {
        BridgeRequest::GetAddress => {
            if !perms.wallet_enabled {
                return BridgeResult::Js(resolve_err(id, "wallet not enabled"));
            }
            match &wallet_address {
                Ok(addr) => BridgeResult::Js(resolve_ok(id, &format!("'{addr}'"))),
                Err(e) => BridgeResult::Js(resolve_err(id, e)),
            }
        }

        BridgeRequest::Sign { payload } => {
            if !perms.wallet_enabled {
                return BridgeResult::Js(resolve_err(id, "wallet not enabled"));
            }
            if has_pending_sign {
                return BridgeResult::Js(resolve_err(
                    id,
                    "another signing request is pending",
                ));
            }
            BridgeResult::Async(BridgeAsyncAction::Sign {
                payload: payload.clone(),
            })
        }

        BridgeRequest::ChainQuery { method, rpc_params } => {
            if !perms.chain {
                return BridgeResult::Js(resolve_err(id, "chain permission not granted"));
            }
            match crate::chain_api::submit_query(
                crate::chain_api::parse_chain_id(chain)
                    .unwrap_or(epoca_chain::ChainId::PaseoAssetHub),
                webview_ptr,
                id,
                method,
                rpc_params,
            ) {
                Ok(()) => BridgeResult::Async(BridgeAsyncAction::ChainQuery {
                    method: method.clone(),
                    rpc_params: rpc_params.clone(),
                    chain: chain.to_string(),
                }),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::ChainSubmit { call_data } => {
            if !perms.chain {
                return BridgeResult::Js(resolve_err(id, "chain permission not granted"));
            }
            if has_pending_submit {
                return BridgeResult::Js(resolve_err(
                    id,
                    "another submit request is pending",
                ));
            }
            BridgeResult::Async(BridgeAsyncAction::ChainSubmit {
                call_data: call_data.clone(),
                chain: chain.to_string(),
            })
        }

        BridgeRequest::StatementsWrite { channel, data } => {
            if !perms.statements {
                return BridgeResult::Js(resolve_err(
                    id,
                    "statements permission not granted",
                ));
            }
            match crate::statements_api::write(app_id, author, channel, data) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::StatementsSubscribe { channel } => {
            if !perms.statements {
                return BridgeResult::Js(resolve_err(
                    id,
                    "statements permission not granted",
                ));
            }
            match crate::statements_api::subscribe(app_id, webview_ptr, channel) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::DataConnect { peer_address } => {
            if !perms.data {
                return BridgeResult::Js(resolve_err(id, "data permission not granted"));
            }
            if has_pending_connect {
                return BridgeResult::Js(resolve_err(
                    id,
                    "another connect request is pending",
                ));
            }
            match crate::data_api::connect(app_id, webview_ptr, peer_address) {
                Ok(conn_id) => BridgeResult::Async(BridgeAsyncAction::DataConnect {
                    peer_address: peer_address.clone(),
                    conn_id,
                }),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::DataSend { conn_id, data } => {
            if !perms.data {
                return BridgeResult::Js(resolve_err(id, "data permission not granted"));
            }
            match crate::data_api::send(app_id, *conn_id, data) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::DataClose { conn_id } => {
            if !perms.data {
                return BridgeResult::Js(resolve_err(id, "data permission not granted"));
            }
            match crate::data_api::close(app_id, *conn_id) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }
    }
}
