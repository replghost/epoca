//! Epoca Host API — transport-agnostic engine for the Polkadot app host-api protocol.
//!
//! Processes binary SCALE-encoded messages from Polkadot apps and returns binary
//! responses. Works over any transport: WKWebView MessagePort, PolkaVM host calls,
//! WebSocket, etc.
//!
//! See [`HostApi::handle_message`] for the main entry point.

pub mod codec;
pub mod protocol;

use protocol::{
    decode_message, encode_account_status, encode_feature_response, encode_navigate_response,
    encode_response, encode_storage_read_response, encode_storage_write_response, Account,
    HostRequest, HostResponse, PROTOCOL_VERSION, TAG_ACCOUNT_STATUS_INTERRUPT,
    TAG_ACCOUNT_STATUS_STOP, TAG_CHAT_ACTION_INTERRUPT, TAG_CHAT_ACTION_STOP,
    TAG_CHAT_CUSTOM_MSG_INTERRUPT, TAG_CHAT_CUSTOM_MSG_STOP, TAG_CHAT_LIST_INTERRUPT,
    TAG_CHAT_LIST_STOP, TAG_JSONRPC_SUB_INTERRUPT, TAG_JSONRPC_SUB_STOP,
    TAG_PREIMAGE_LOOKUP_INTERRUPT, TAG_PREIMAGE_LOOKUP_STOP, TAG_STATEMENT_STORE_INTERRUPT,
    TAG_STATEMENT_STORE_STOP,
};

/// Maximum number of storage keys per app.
const MAX_STORAGE_KEYS_PER_APP: usize = 1024;
/// Maximum size of a single storage value (64 KB).
const MAX_STORAGE_VALUE_SIZE: usize = 64 * 1024;
/// Maximum storage key length (512 bytes).
const MAX_STORAGE_KEY_LENGTH: usize = 512;

/// Outcome of processing a host-api message.
pub enum HostApiOutcome {
    /// Send this response directly back to the app.
    Response(Vec<u8>),
    /// Sign request — needs wallet to produce a signature before responding.
    NeedsSign {
        request_id: String,
        request_tag: u8,
        public_key: Vec<u8>,
        payload: Vec<u8>,
    },
    /// JSON-RPC query — needs routing through the chain API allowlist + RPC bridge.
    NeedsChainQuery {
        request_id: String,
        method: String,
        params: serde_json::Value,
    },
    /// Navigation request — the workbench should open this URL (may be a .dot address).
    NeedsNavigate {
        request_id: String,
        url: String,
    },
    /// No response needed (fire-and-forget).
    Silent,
}

/// Shared host implementation. Handles decoded requests, returns encoded responses.
pub struct HostApi {
    accounts: Vec<Account>,
    local_storage: std::collections::HashMap<String, Vec<u8>>,
}

impl Default for HostApi {
    fn default() -> Self {
        Self::new()
    }
}

impl HostApi {
    pub fn new() -> Self {
        Self {
            accounts: Vec::new(),
            local_storage: std::collections::HashMap::new(),
        }
    }

    /// Set the accounts that will be returned by host_get_non_product_accounts.
    pub fn set_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
    }

    /// Process a raw binary message from the app.
    ///
    /// Returns `HostApiOutcome::Response` for immediate replies,
    /// `HostApiOutcome::NeedsSign` for sign requests that need wallet approval,
    /// or `HostApiOutcome::Silent` for fire-and-forget messages.
    ///
    /// `app_id` scopes local storage — each app gets its own namespace.
    pub fn handle_message(&mut self, raw: &[u8], app_id: &str) -> HostApiOutcome {
        let (request_id, request_tag, req) = match decode_message(raw) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[hostapi] failed to decode message: {e}");
                return HostApiOutcome::Silent;
            }
        };

        log::info!("[hostapi] request: {req:?} (id={request_id}, tag={request_tag})");

        match req {
            HostRequest::Handshake { version } => {
                if version == PROTOCOL_VERSION {
                    HostApiOutcome::Response(encode_response(
                        &request_id,
                        request_tag,
                        &HostResponse::HandshakeOk,
                    ))
                } else {
                    log::warn!("[hostapi] unsupported protocol version: {version}");
                    HostApiOutcome::Response(encode_response(
                        &request_id,
                        request_tag,
                        &HostResponse::Error("unsupported protocol version".into()),
                    ))
                }
            }

            HostRequest::GetNonProductAccounts => {
                log::info!(
                    "[hostapi] returning {} accounts",
                    self.accounts.len()
                );
                HostApiOutcome::Response(encode_response(
                    &request_id,
                    request_tag,
                    &HostResponse::AccountList(self.accounts.clone()),
                ))
            }

            HostRequest::FeatureSupported { feature_data } => {
                // Check if the feature matches known supported features.
                let feature_str = std::str::from_utf8(&feature_data).unwrap_or("");
                let supported = matches!(feature_str, "signing" | "sign" | "navigate");
                log::info!("[hostapi] feature_supported: '{feature_str}' → {supported}");
                HostApiOutcome::Response(encode_feature_response(&request_id, supported))
            }

            HostRequest::AccountConnectionStatusStart => {
                HostApiOutcome::Response(encode_account_status(&request_id, true))
            }

            HostRequest::LocalStorageRead { key } => {
                let scoped = format!("{app_id}\0{key}");
                let value = self.local_storage.get(&scoped).map(|v| v.as_slice());
                HostApiOutcome::Response(encode_storage_read_response(&request_id, value))
            }

            HostRequest::LocalStorageWrite { key, value } => {
                if key.len() > MAX_STORAGE_KEY_LENGTH {
                    return HostApiOutcome::Response(encode_response(
                        &request_id,
                        request_tag,
                        &HostResponse::Error("storage key too long".into()),
                    ));
                }
                if value.len() > MAX_STORAGE_VALUE_SIZE {
                    return HostApiOutcome::Response(encode_response(
                        &request_id,
                        request_tag,
                        &HostResponse::Error("storage value too large".into()),
                    ));
                }
                let scoped = format!("{app_id}\0{key}");
                // Check per-app key count (count keys with this app's prefix).
                if !self.local_storage.contains_key(&scoped) {
                    let prefix = format!("{app_id}\0");
                    let app_key_count = self
                        .local_storage
                        .keys()
                        .filter(|k| k.starts_with(&prefix))
                        .count();
                    if app_key_count >= MAX_STORAGE_KEYS_PER_APP {
                        return HostApiOutcome::Response(encode_response(
                            &request_id,
                            request_tag,
                            &HostResponse::Error("storage key limit reached".into()),
                        ));
                    }
                }
                self.local_storage.insert(scoped, value);
                HostApiOutcome::Response(encode_storage_write_response(&request_id, false))
            }

            HostRequest::LocalStorageClear { key } => {
                let scoped = format!("{app_id}\0{key}");
                self.local_storage.remove(&scoped);
                HostApiOutcome::Response(encode_storage_write_response(&request_id, true))
            }

            HostRequest::NavigateTo { url } => {
                log::info!("[hostapi] navigate_to: {url}");
                HostApiOutcome::NeedsNavigate {
                    request_id,
                    url,
                }
            }

            HostRequest::SignPayload { public_key, payload } => {
                log::info!("[hostapi] sign_payload request (pubkey={} bytes)", public_key.len());
                HostApiOutcome::NeedsSign {
                    request_id,
                    request_tag,
                    public_key,
                    payload,
                }
            }

            HostRequest::SignRaw { public_key, data } => {
                log::info!("[hostapi] sign_raw request (pubkey={} bytes)", public_key.len());
                HostApiOutcome::NeedsSign {
                    request_id,
                    request_tag,
                    public_key,
                    payload: data,
                }
            }

            HostRequest::CreateTransaction { .. } => {
                log::info!("[hostapi] create_transaction (not yet implemented)");
                HostApiOutcome::Response(encode_response(
                    &request_id,
                    request_tag,
                    &HostResponse::Error("create_transaction not yet implemented".into()),
                ))
            }

            HostRequest::JsonRpcSend { data } => {
                // The data is a SCALE string containing a JSON-RPC request body,
                // or raw bytes we try to interpret as UTF-8 JSON.
                let json_str = parse_jsonrpc_data(&data);
                match json_str {
                    Some((method, params)) => {
                        log::info!("[hostapi] JSON-RPC send: method={method}");
                        HostApiOutcome::NeedsChainQuery {
                            request_id,
                            method,
                            params,
                        }
                    }
                    None => {
                        log::warn!("[hostapi] failed to parse JSON-RPC send data");
                        HostApiOutcome::Response(encode_response(
                            &request_id,
                            request_tag,
                            &HostResponse::Error("invalid json-rpc request".into()),
                        ))
                    }
                }
            }

            HostRequest::JsonRpcSubscribeStart { .. } => {
                log::info!("[hostapi] JSON-RPC subscribe (not yet implemented)");
                HostApiOutcome::Response(encode_response(
                    &request_id,
                    request_tag,
                    &HostResponse::Error("json-rpc subscriptions not yet implemented".into()),
                ))
            }

            HostRequest::Unimplemented { tag } => {
                log::info!("[hostapi] unimplemented method (tag={tag})");
                if is_subscription_control(tag) {
                    HostApiOutcome::Silent
                } else {
                    HostApiOutcome::Response(encode_response(
                        &request_id,
                        request_tag,
                        &HostResponse::Error("not implemented".into()),
                    ))
                }
            }

            HostRequest::Unknown { tag } => {
                log::warn!("[hostapi] unknown tag: {tag}");
                HostApiOutcome::Silent
            }
        }
    }
}

/// Parse the `data` field from a `JsonRpcSend` request.
///
/// The Product SDK encodes this as a SCALE string containing the full JSON-RPC
/// request (e.g. `{"jsonrpc":"2.0","id":1,"method":"state_getMetadata","params":[]}`).
/// We try SCALE string first, then fall back to raw UTF-8.
fn parse_jsonrpc_data(data: &[u8]) -> Option<(String, serde_json::Value)> {
    // Try SCALE string (compact length + UTF-8 bytes).
    let json_str = codec::Reader::new(data)
        .read_string()
        .ok()
        .or_else(|| std::str::from_utf8(data).ok().map(|s| s.to_string()))?;

    let v: serde_json::Value = serde_json::from_str(&json_str).ok()?;
    let method = v.get("method")?.as_str()?.to_string();
    let params = v.get("params").cloned().unwrap_or(serde_json::Value::Array(vec![]));
    Some((method, params))
}

/// Check if a tag is a subscription control message (stop/interrupt).
fn is_subscription_control(tag: u8) -> bool {
    matches!(
        tag,
        TAG_ACCOUNT_STATUS_STOP
            | TAG_ACCOUNT_STATUS_INTERRUPT
            | TAG_CHAT_LIST_STOP
            | TAG_CHAT_LIST_INTERRUPT
            | TAG_CHAT_ACTION_STOP
            | TAG_CHAT_ACTION_INTERRUPT
            | TAG_CHAT_CUSTOM_MSG_STOP
            | TAG_CHAT_CUSTOM_MSG_INTERRUPT
            | TAG_STATEMENT_STORE_STOP
            | TAG_STATEMENT_STORE_INTERRUPT
            | TAG_PREIMAGE_LOOKUP_STOP
            | TAG_PREIMAGE_LOOKUP_INTERRUPT
            | TAG_JSONRPC_SUB_STOP
            | TAG_JSONRPC_SUB_INTERRUPT
    )
}

// ---------------------------------------------------------------------------
// JS bridge script — injected into WKWebView at document_start
// ---------------------------------------------------------------------------

/// JavaScript injected before the Polkadot app loads. Sets up:
/// 1. `window.__HOST_WEBVIEW_MARK__ = true` — SDK webview detection
/// 2. `MessageChannel` with port2 as `window.__HOST_API_PORT__`
/// 3. Binary message forwarding between port1 and native (base64)
pub const HOST_API_BRIDGE_SCRIPT: &str = r#"
(function() {
    'use strict';
    console.log('[epoca-bridge] STARTING, guard=' + !!window.__epocaHostApiBridge);
    if (window.__epocaHostApiBridge) return;
    window.__epocaHostApiBridge = true;

    // Signal to the app SDK that we're a webview host.
    window.__HOST_WEBVIEW_MARK__ = true;
    console.log('[epoca-bridge] set __HOST_WEBVIEW_MARK__=true');

    // Create the MessageChannel.
    var ch = new MessageChannel();
    window.__HOST_API_PORT__ = ch.port2;
    ch.port2.start();
    console.log('[epoca-bridge] set __HOST_API_PORT__, port2.start() done');

    // Host-side port — messages from the app arrive here.
    var port1 = ch.port1;
    port1.start();

    // Forward binary messages from app to native as base64.
    port1.onmessage = function(ev) {
        var data = ev.data;
        console.log('[epoca-bridge] port1.onmessage fired, data type=' + (data ? data.constructor.name : 'null'));
        if (!data) return;
        var bytes;
        if (data instanceof Uint8Array) {
            bytes = data;
        } else if (data instanceof ArrayBuffer) {
            bytes = new Uint8Array(data);
        } else if (ArrayBuffer.isView(data)) {
            bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
        } else {
            console.log('[epoca-bridge] port1.onmessage: unrecognized data type, ignoring');
            return;
        }
        var binary = '';
        for (var i = 0; i < bytes.length; i++) {
            binary += String.fromCharCode(bytes[i]);
        }
        console.log('[epoca-bridge] forwarding ' + bytes.length + ' bytes to native');
        window.webkit.messageHandlers.epocaHostApi.postMessage(btoa(binary));
    };

    // Native sends responses back by calling this function with base64.
    window.__epocaHostApiReply = function(b64) {
        console.log('[epoca-bridge] __epocaHostApiReply called, b64 len=' + (b64 ? b64.length : 0));
        try {
            var binary = atob(b64);
            var bytes = new Uint8Array(binary.length);
            for (var i = 0; i < binary.length; i++) {
                bytes[i] = binary.charCodeAt(i);
            }
            port1.postMessage(bytes, [bytes.buffer]);
        } catch(e) {
            console.error('[epoca-bridge] reply delivery failed:', e.message);
        }
    };
    console.log('[epoca-bridge] SETUP COMPLETE, __HOST_API_PORT__=' + !!window.__HOST_API_PORT__ + ' __HOST_WEBVIEW_MARK__=' + !!window.__HOST_WEBVIEW_MARK__);
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::*;

    const TEST_APP: &str = "test-app";

    /// Extract a Response from HostApiOutcome, panicking on other variants.
    fn expect_response(outcome: HostApiOutcome) -> Vec<u8> {
        match outcome {
            HostApiOutcome::Response(v) => v,
            other => panic!("expected Response, got {}", outcome_name(&other)),
        }
    }

    fn expect_silent(outcome: HostApiOutcome) {
        match outcome {
            HostApiOutcome::Silent => {}
            other => panic!("expected Silent, got {}", outcome_name(&other)),
        }
    }

    fn outcome_name(o: &HostApiOutcome) -> &'static str {
        match o {
            HostApiOutcome::Response(_) => "Response",
            HostApiOutcome::NeedsSign { .. } => "NeedsSign",
            HostApiOutcome::NeedsChainQuery { .. } => "NeedsChainQuery",
            HostApiOutcome::NeedsNavigate { .. } => "NeedsNavigate",
            HostApiOutcome::Silent => "Silent",
        }
    }

    fn make_handshake_request(request_id: &str) -> Vec<u8> {
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, request_id);
        msg.push(TAG_HANDSHAKE_REQ);
        msg.push(0); // v1
        msg.push(PROTOCOL_VERSION);
        msg
    }

    fn make_get_accounts_request(request_id: &str) -> Vec<u8> {
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, request_id);
        msg.push(TAG_GET_NON_PRODUCT_ACCOUNTS_REQ);
        msg.push(0); // v1
        msg
    }

    fn make_storage_write(request_id: &str, key: &str, value: &[u8]) -> Vec<u8> {
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, request_id);
        msg.push(TAG_LOCAL_STORAGE_WRITE_REQ);
        msg.push(0); // v1
        codec::encode_string(&mut msg, key);
        codec::encode_var_bytes(&mut msg, value);
        msg
    }

    fn make_storage_read(request_id: &str, key: &str) -> Vec<u8> {
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, request_id);
        msg.push(TAG_LOCAL_STORAGE_READ_REQ);
        msg.push(0); // v1
        codec::encode_string(&mut msg, key);
        msg
    }

    fn make_storage_clear(request_id: &str, key: &str) -> Vec<u8> {
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, request_id);
        msg.push(TAG_LOCAL_STORAGE_CLEAR_REQ);
        msg.push(0); // v1
        codec::encode_string(&mut msg, key);
        msg
    }

    #[test]
    fn handshake_flow() {
        let mut api = HostApi::new();
        let req = make_handshake_request("hs-1");
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "hs-1");
        assert_eq!(r.read_u8().unwrap(), TAG_HANDSHAKE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
    }

    #[test]
    fn handshake_wrong_version() {
        let mut api = HostApi::new();
        let mut req = Vec::new();
        codec::encode_string(&mut req, "hs-bad");
        req.push(TAG_HANDSHAKE_REQ);
        req.push(0); // v1
        req.push(255); // wrong version
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "hs-bad");
        assert_eq!(r.read_u8().unwrap(), TAG_HANDSHAKE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 1); // Result::Err
    }

    #[test]
    fn get_accounts_empty() {
        let mut api = HostApi::new();
        let req = make_get_accounts_request("acc-1");
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "acc-1");
        assert_eq!(r.read_u8().unwrap(), TAG_GET_NON_PRODUCT_ACCOUNTS_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        assert_eq!(r.read_compact_u32().unwrap(), 0); // empty vector
    }

    #[test]
    fn get_accounts_with_data() {
        let mut api = HostApi::new();
        api.set_accounts(vec![Account {
            public_key: vec![0xAA; 32],
            name: Some("Test Account".into()),
        }]);

        let req = make_get_accounts_request("acc-2");
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "acc-2");
        assert_eq!(r.read_u8().unwrap(), TAG_GET_NON_PRODUCT_ACCOUNTS_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        assert_eq!(r.read_compact_u32().unwrap(), 1); // 1 account

        let pk = r.read_var_bytes().unwrap();
        assert_eq!(pk, vec![0xAA; 32]);
        let name = r.read_option(|r| r.read_string()).unwrap();
        assert_eq!(name.as_deref(), Some("Test Account"));
    }

    #[test]
    fn local_storage_round_trip() {
        let mut api = HostApi::new();

        expect_response(api.handle_message(&make_storage_write("w-1", "mykey", b"myvalue"), TEST_APP));

        let resp = expect_response(api.handle_message(&make_storage_read("r-1", "mykey"), TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "r-1");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_READ_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        let val = r.read_option(|r| r.read_var_bytes()).unwrap();
        assert_eq!(val.as_deref(), Some(b"myvalue".as_ref()));
    }

    #[test]
    fn local_storage_read_missing_key() {
        let mut api = HostApi::new();
        let resp = expect_response(api.handle_message(&make_storage_read("r-miss", "nonexistent"), TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "r-miss");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_READ_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        let val = r.read_option(|r| r.read_var_bytes()).unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn local_storage_clear() {
        let mut api = HostApi::new();

        // Write then clear
        api.handle_message(&make_storage_write("w-2", "clearme", b"data"), TEST_APP);
        let resp = expect_response(api.handle_message(&make_storage_clear("c-1", "clearme"), TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "c-1");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_CLEAR_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok

        // Verify key is gone
        let resp2 = expect_response(api.handle_message(&make_storage_read("r-2", "clearme"), TEST_APP));
        let mut r2 = codec::Reader::new(&resp2);
        r2.read_string().unwrap();
        r2.read_u8().unwrap();
        r2.read_u8().unwrap();
        r2.read_u8().unwrap();
        let val = r2.read_option(|r| r.read_var_bytes()).unwrap();
        assert!(val.is_none());
    }

    #[test]
    fn local_storage_isolation_between_apps() {
        let mut api = HostApi::new();

        // App A writes a key
        api.handle_message(&make_storage_write("w-a", "shared", b"from_a"), "app-a");

        // App B reads the same key name — should get None
        let resp = expect_response(api.handle_message(&make_storage_read("r-b", "shared"), "app-b"));
        let mut r = codec::Reader::new(&resp);
        r.read_string().unwrap();
        r.read_u8().unwrap();
        r.read_u8().unwrap();
        r.read_u8().unwrap();
        let val = r.read_option(|r| r.read_var_bytes()).unwrap();
        assert!(val.is_none(), "app-b should not see app-a's data");

        // App A reads its own key — should get the value
        let resp2 = expect_response(api.handle_message(&make_storage_read("r-a", "shared"), "app-a"));
        let mut r2 = codec::Reader::new(&resp2);
        r2.read_string().unwrap();
        r2.read_u8().unwrap();
        r2.read_u8().unwrap();
        r2.read_u8().unwrap();
        let val2 = r2.read_option(|r| r.read_var_bytes()).unwrap();
        assert_eq!(val2.as_deref(), Some(b"from_a".as_ref()));
    }

    #[test]
    fn unimplemented_request_returns_error() {
        let mut api = HostApi::new();
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "unimp-1");
        msg.push(TAG_PUSH_NOTIFICATION_REQ);

        let resp = expect_response(api.handle_message(&msg, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "unimp-1");
        assert_eq!(r.read_u8().unwrap(), TAG_PUSH_NOTIFICATION_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 1); // Result::Err
    }

    #[test]
    fn subscription_stop_returns_silent() {
        let mut api = HostApi::new();
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "stop-1");
        msg.push(TAG_ACCOUNT_STATUS_STOP);

        expect_silent(api.handle_message(&msg, TEST_APP));
    }

    #[test]
    fn malformed_input_returns_silent() {
        let mut api = HostApi::new();

        // Empty input
        expect_silent(api.handle_message(&[], TEST_APP));

        // Truncated after request_id
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "trunc");
        expect_silent(api.handle_message(&msg, TEST_APP));
    }

    #[test]
    fn unknown_tag_returns_silent() {
        let mut api = HostApi::new();
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "unk-1");
        msg.push(0xFF); // unknown tag
        expect_silent(api.handle_message(&msg, TEST_APP));
    }

    #[test]
    fn storage_write_rejects_oversized_value() {
        let mut api = HostApi::new();
        let big_value = vec![0xAA; super::MAX_STORAGE_VALUE_SIZE + 1];
        let req = make_storage_write("w-big", "key", &big_value);
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "w-big");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_WRITE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 1); // Result::Err
    }

    #[test]
    fn storage_write_rejects_long_key() {
        let mut api = HostApi::new();
        let long_key = "k".repeat(super::MAX_STORAGE_KEY_LENGTH + 1);
        let req = make_storage_write("w-longkey", &long_key, b"v");
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "w-longkey");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_WRITE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 1); // Result::Err
    }

    #[test]
    fn storage_write_enforces_key_limit() {
        let mut api = HostApi::new();
        // Fill up to the limit.
        for i in 0..super::MAX_STORAGE_KEYS_PER_APP {
            let key = format!("key-{i}");
            let req = make_storage_write(&format!("w-{i}"), &key, b"v");
            expect_response(api.handle_message(&req, TEST_APP));
        }
        // The next key should be rejected.
        let req = make_storage_write("w-over", "one-too-many", b"v");
        let resp = expect_response(api.handle_message(&req, TEST_APP));

        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "w-over");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_WRITE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 1); // Result::Err

        // Overwriting an existing key should still work.
        let req = make_storage_write("w-update", "key-0", b"new-value");
        let resp = expect_response(api.handle_message(&req, TEST_APP));
        let mut r = codec::Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "w-update");
        assert_eq!(r.read_u8().unwrap(), TAG_LOCAL_STORAGE_WRITE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
    }

    #[test]
    fn sign_payload_returns_needs_sign() {
        let mut api = HostApi::new();
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "sign-1");
        msg.push(TAG_SIGN_PAYLOAD_REQ);
        msg.push(0); // v1
        codec::encode_var_bytes(&mut msg, &[0xAA; 32]); // publicKey
        msg.extend_from_slice(b"payload-data");

        match api.handle_message(&msg, TEST_APP) {
            HostApiOutcome::NeedsSign { request_id, request_tag, public_key, payload } => {
                assert_eq!(request_id, "sign-1");
                assert_eq!(request_tag, TAG_SIGN_PAYLOAD_REQ);
                assert_eq!(public_key, vec![0xAA; 32]);
                assert_eq!(payload, b"payload-data");
            }
            _ => panic!("expected NeedsSign"),
        }
    }

    #[test]
    fn sign_raw_returns_needs_sign() {
        let mut api = HostApi::new();
        let mut msg = Vec::new();
        codec::encode_string(&mut msg, "sign-2");
        msg.push(TAG_SIGN_RAW_REQ);
        msg.push(0); // v1
        codec::encode_var_bytes(&mut msg, &[0xBB; 32]); // publicKey
        msg.extend_from_slice(b"raw-bytes");

        match api.handle_message(&msg, TEST_APP) {
            HostApiOutcome::NeedsSign { request_id, request_tag, public_key, payload } => {
                assert_eq!(request_id, "sign-2");
                assert_eq!(request_tag, TAG_SIGN_RAW_REQ);
                assert_eq!(public_key, vec![0xBB; 32]);
                assert_eq!(payload, b"raw-bytes");
            }
            _ => panic!("expected NeedsSign"),
        }
    }
}
