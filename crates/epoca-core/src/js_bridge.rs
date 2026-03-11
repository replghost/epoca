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
    DataGetPeerId,
    MediaGetUserMedia { audio: bool, video: bool },
    MediaConnect { peer_address: String, track_ids: Vec<u64>, from_address: String },
    MediaAccept,
    MediaClose { session_id: u64 },
    MediaAttachTrack { track_id: u64, element_id: String },
    MediaSignal { session_id: u64, signal_type: String, data: String },
    MediaGetPeerId,
    MediaStartListening { address: String },
    MediaSetTrackEnabled { track_id: u64, enabled: bool },
    RequestWssPermission { url: String },
    RequestHttpPermission { origin: String },
    StorageGet { key: String },
    StorageSet { key: String, value: String },
    StorageRemove { key: String },
}

/// Permission context needed for dispatch decisions.
pub struct BridgePermissions {
    pub wallet_enabled: bool,
    pub chain: bool,
    pub statements: bool,
    pub data: bool,
    /// Granted media capabilities, e.g. ["camera", "audio"].
    pub media: Vec<String>,
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
    /// getUserMedia: resolve the promise with track IDs, then evaluate the
    /// getUserMedia JS on the webview to actually open the camera/mic.
    MediaGetUserMedia {
        call_id: u64,
        webview_ptr: usize,
        audio: bool,
        video: bool,
        audio_track_id: Option<u64>,
        video_track_id: Option<u64>,
    },
    /// attachTrack: evaluate JS that wires a track to a DOM element.
    MediaAttachTrack {
        call_id: u64,
        webview_ptr: usize,
        track_id: u64,
        element_id: String,
    },
    /// connect: set up RTCPeerConnection via evaluateScript + start signaling.
    MediaConnect {
        call_id: u64,
        webview_ptr: usize,
        session_id: u64,
        peer_address: String,
        track_ids: Vec<u64>,
        author: String,
    },
    /// accept: create session for pending incoming call (deferred from ring).
    MediaAccept {
        call_id: u64,
        webview_ptr: usize,
        session_id: u64,
        peer_address: String,
        track_ids: Vec<u64>,
        local_peer_id: String,
        /// JS strings for signals buffered between ring and accept (offer, ICE, etc.).
        buffered_signals: Vec<String>,
    },
    /// close: tear down RTCPeerConnection and clean up session.
    MediaClose {
        call_id: u64,
        webview_ptr: usize,
        session_id: u64,
    },
    WssPermission { url: String },
    HttpPermission { origin: String },
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
        "dataGetPeerId" => Ok(BridgeRequest::DataGetPeerId),
        "mediaAccept" => Ok(BridgeRequest::MediaAccept),
        "mediaGetUserMedia" => {
            let audio = params.get("audio").and_then(|v| v.as_bool()).unwrap_or(false);
            let video = params.get("video").and_then(|v| v.as_bool()).unwrap_or(false);
            Ok(BridgeRequest::MediaGetUserMedia { audio, video })
        }
        "mediaConnect" => {
            let peer_address = params
                .get("peer")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let track_ids = params
                .get("trackIds")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
                .unwrap_or_default();
            let from_address = params
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::MediaConnect { peer_address, track_ids, from_address })
        }
        "mediaClose" => {
            let session_id = params.get("sessionId").and_then(|v| v.as_u64()).unwrap_or(0);
            Ok(BridgeRequest::MediaClose { session_id })
        }
        "mediaAttachTrack" => {
            let track_id = params.get("trackId").and_then(|v| v.as_u64()).unwrap_or(0);
            let element_id = params
                .get("elementId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::MediaAttachTrack { track_id, element_id })
        }
        "requestWssPermission" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::RequestWssPermission { url })
        }
        "requestHttpPermission" => {
            let origin = params.get("origin").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::RequestHttpPermission { origin })
        }
        "mediaGetPeerId" => Ok(BridgeRequest::MediaGetPeerId),
        "mediaStartListening" => {
            let address = params.get("address").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::MediaStartListening { address })
        }
        "mediaSetTrackEnabled" => {
            let track_id = params.get("trackId").and_then(|v| v.as_u64()).unwrap_or(0);
            let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            Ok(BridgeRequest::MediaSetTrackEnabled { track_id, enabled })
        }
        "mediaSignal" => {
            let session_id = params.get("sessionId").and_then(|v| v.as_u64()).unwrap_or(0);
            let signal_type = params
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let data = params
                .get("data")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Ok(BridgeRequest::MediaSignal { session_id, signal_type, data })
        }
        "storageGet" => {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::StorageGet { key })
        }
        "storageSet" => {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let value =
                params.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::StorageSet { key, value })
        }
        "storageRemove" => {
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Ok(BridgeRequest::StorageRemove { key })
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

        BridgeRequest::RequestWssPermission { url } => {
            if perms.chain {
                // Already has chain permission — shouldn't happen but resolve OK
                return BridgeResult::Js(resolve_ok(id, "true"));
            }
            BridgeResult::Async(BridgeAsyncAction::WssPermission { url: url.clone() })
        }

        BridgeRequest::RequestHttpPermission { origin } => {
            BridgeResult::Async(BridgeAsyncAction::HttpPermission { origin: origin.clone() })
        }

        BridgeRequest::StorageGet { key } => {
            match crate::app_storage::get(app_id, key) {
                Some(value) => {
                    // Serialize the string value as a JSON string so the JS side
                    // receives a proper string (not a bare identifier).
                    let json_val = serde_json::to_string(&value).unwrap_or("null".to_string());
                    BridgeResult::Js(resolve_ok(id, &json_val))
                }
                None => BridgeResult::Js(resolve_ok(id, "null")),
            }
        }

        BridgeRequest::StorageSet { key, value } => {
            match crate::app_storage::set(app_id, key, value) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::StorageRemove { key } => {
            match crate::app_storage::remove(app_id, key) {
                Ok(()) => BridgeResult::Js(resolve_ok(id, "true")),
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
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

        BridgeRequest::DataGetPeerId => {
            if !perms.data {
                return BridgeResult::Js(resolve_err(id, "data permission not granted"));
            }
            let peer_id = match &wallet_address {
                Ok(addr) => addr.clone(),
                Err(_) => crate::data_api::local_peer_id(webview_ptr),
            };
            BridgeResult::Js(resolve_ok(id, &format!("'{peer_id}'")))
        }

        BridgeRequest::MediaGetUserMedia { audio, video } => {
            // Require at least one media capability to be granted.
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            if *video && !perms.media.iter().any(|c| c == "camera") {
                return BridgeResult::Js(resolve_err(id, "camera permission not granted"));
            }
            if *audio && !perms.media.iter().any(|c| c == "audio") {
                return BridgeResult::Js(resolve_err(id, "audio permission not granted"));
            }
            // Allocate track IDs now. The workbench will then: (1) resolve the
            // promise with the IDs, (2) evaluate the getUserMedia JS.
            let (audio_tid, video_tid) =
                crate::media_api::request_get_user_media(webview_ptr, *audio, *video);
            // Start ring listener so this peer can receive incoming calls.
            if let Ok(addr) = &wallet_address {
                let _ = crate::media_api::start_ring_listener(app_id, addr, webview_ptr);
            }
            BridgeResult::Async(BridgeAsyncAction::MediaGetUserMedia {
                call_id: id,
                webview_ptr,
                audio: *audio,
                video: *video,
                audio_track_id: audio_tid,
                video_track_id: video_tid,
            })
        }

        BridgeRequest::MediaGetPeerId => {
            let peer_id = match &wallet_address {
                Ok(addr) => addr.clone(),
                Err(_) => crate::media_api::local_peer_id_pub(),
            };
            BridgeResult::Js(resolve_ok(id, &format!("'{peer_id}'")))
        }

        BridgeRequest::MediaSetTrackEnabled { track_id, enabled } => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            let js = crate::media_api::set_track_enabled_js(*track_id, *enabled);
            // Evaluate on the webview, then resolve the promise.
            let resolve = resolve_ok(id, "true");
            BridgeResult::Js(format!("{js}; {resolve}"))
        }

        BridgeRequest::MediaStartListening { address } => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            if address.is_empty() {
                return BridgeResult::Js(resolve_err(id, "address is required"));
            }
            let _ = crate::media_api::start_ring_listener(app_id, address, webview_ptr);
            log::info!("[media] ring listener started via mediaStartListening for {}", address);
            BridgeResult::Js(resolve_ok(id, "true"))
        }

        BridgeRequest::MediaAccept => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            match crate::media_api::accept_incoming_call(webview_ptr) {
                Ok((session_id, peer_address, _app_id, track_ids, local_peer_id, buffered_signals)) => {
                    BridgeResult::Async(BridgeAsyncAction::MediaAccept {
                        call_id: id,
                        webview_ptr,
                        session_id,
                        peer_address,
                        track_ids,
                        local_peer_id,
                        buffered_signals,
                    })
                }
                Err(e) => BridgeResult::Js(resolve_err(id, &e)),
            }
        }

        BridgeRequest::MediaConnect { peer_address, track_ids, from_address: _ } => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            if peer_address.is_empty() {
                return BridgeResult::Js(resolve_err(id, "peer address cannot be empty"));
            }
            // Always use wallet_address for local identity — never trust from_address from the SPA.
            let local_id = match &wallet_address {
                Ok(addr) => addr.as_str(),
                Err(_) => author,
            };
            let session_id = crate::media_api::create_session(
                webview_ptr, app_id, peer_address, track_ids.clone(), local_id,
            );
            // Publish ring to the remote peer so they auto-create a session.
            if let Err(e) = crate::media_api::publish_ring(app_id, local_id, peer_address) {
                log::warn!("[media] publish ring failed: {e}");
            } else {
                crate::media_api::push_signaling_progress(session_id, "ring_sent");
            }
            // Start signaling relay thread.
            match crate::media_api::start_signaling(session_id, app_id, peer_address, local_id) {
                Ok(handle) => {
                    crate::media_api::set_signaling_handle(session_id, handle);
                }
                Err(e) => {
                    return BridgeResult::Js(resolve_err(id, &e));
                }
            }
            BridgeResult::Async(BridgeAsyncAction::MediaConnect {
                call_id: id,
                webview_ptr,
                session_id,
                peer_address: peer_address.clone(),
                track_ids: track_ids.clone(),
                author: local_id.to_string(),
            })
        }

        BridgeRequest::MediaClose { session_id } => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            BridgeResult::Async(BridgeAsyncAction::MediaClose {
                call_id: id,
                webview_ptr,
                session_id: *session_id,
            })
        }

        BridgeRequest::MediaSignal { session_id, signal_type, data } => {
            // Signals from JS are fire-and-forget (id=0), no promise to resolve.
            match signal_type.as_str() {
                "offer" | "answer" | "candidate" => {
                    // Relay to remote peer via Statement Store.
                    if let Err(e) = crate::media_api::publish_signal(*session_id, signal_type, data) {
                        log::warn!("[media] publish signal failed: {e}");
                    }
                }
                "connected" => {
                    crate::media_api::session_connected(*session_id);
                }
                "closed" => {
                    crate::media_api::close_session(*session_id, data);
                }
                "remoteTrack" => {
                    if let Ok(info) = serde_json::from_str::<serde_json::Value>(data) {
                        let track_id = info.get("trackId").and_then(|v| v.as_u64()).unwrap_or(0);
                        let kind = info.get("kind").and_then(|v| v.as_str()).unwrap_or("unknown");
                        crate::media_api::push_remote_track(*session_id, track_id, kind);
                    }
                }
                _ => {}
            }
            // id=0 signals have no pending promise, resolve is a no-op.
            BridgeResult::Js(resolve_ok(id, "true"))
        }

        BridgeRequest::MediaAttachTrack { track_id, element_id } => {
            if perms.media.is_empty() {
                return BridgeResult::Js(resolve_err(id, "media permission not granted"));
            }
            // Reject element IDs with characters that could escape a JS string context.
            if element_id.contains(['\'', '"', '\\', '\n', '\r']) {
                return BridgeResult::Js(resolve_err(id, "invalid elementId"));
            }
            BridgeResult::Async(BridgeAsyncAction::MediaAttachTrack {
                call_id: id,
                webview_ptr,
                track_id: *track_id,
                element_id: element_id.clone(),
            })
        }
    }
}
