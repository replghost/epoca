/// Wire protocol for the Polkadot app host-api.
///
/// Message = Struct { requestId: str, payload: Enum { ...76+ variants... } }
///
/// Each payload variant is itself a versioned enum (currently only "v1" = tag 0).
/// Inside the version wrapper, the actual data is method-specific.
///
/// Tag indices are determined by the insertion order of methods in the protocol
/// definition, expanded as: request methods → _request, _response;
/// subscription methods → _start, _stop, _interrupt, _receive.

use crate::codec::*;

// ---------------------------------------------------------------------------
// Payload tag indices (determined by protocol method insertion order)
// ---------------------------------------------------------------------------

// request/response pairs: tag N = request, tag N+1 = response
pub const TAG_HANDSHAKE_REQ: u8 = 0;
pub const TAG_HANDSHAKE_RESP: u8 = 1;
pub const TAG_FEATURE_SUPPORTED_REQ: u8 = 2;
pub const TAG_FEATURE_SUPPORTED_RESP: u8 = 3;
pub const TAG_PUSH_NOTIFICATION_REQ: u8 = 4;
pub const TAG_PUSH_NOTIFICATION_RESP: u8 = 5;
pub const TAG_NAVIGATE_TO_REQ: u8 = 6;
pub const TAG_NAVIGATE_TO_RESP: u8 = 7;
pub const TAG_DEVICE_PERMISSION_REQ: u8 = 8;
pub const TAG_DEVICE_PERMISSION_RESP: u8 = 9;
pub const TAG_REMOTE_PERMISSION_REQ: u8 = 10;
pub const TAG_REMOTE_PERMISSION_RESP: u8 = 11;
pub const TAG_LOCAL_STORAGE_READ_REQ: u8 = 12;
pub const TAG_LOCAL_STORAGE_READ_RESP: u8 = 13;
pub const TAG_LOCAL_STORAGE_WRITE_REQ: u8 = 14;
pub const TAG_LOCAL_STORAGE_WRITE_RESP: u8 = 15;
pub const TAG_LOCAL_STORAGE_CLEAR_REQ: u8 = 16;
pub const TAG_LOCAL_STORAGE_CLEAR_RESP: u8 = 17;
// subscription: _start, _stop, _interrupt, _receive
pub const TAG_ACCOUNT_STATUS_START: u8 = 18;
pub const TAG_ACCOUNT_STATUS_STOP: u8 = 19;
pub const TAG_ACCOUNT_STATUS_INTERRUPT: u8 = 20;
pub const TAG_ACCOUNT_STATUS_RECEIVE: u8 = 21;
pub const TAG_ACCOUNT_GET_REQ: u8 = 22;
pub const TAG_ACCOUNT_GET_RESP: u8 = 23;
pub const TAG_ACCOUNT_GET_ALIAS_REQ: u8 = 24;
pub const TAG_ACCOUNT_GET_ALIAS_RESP: u8 = 25;
pub const TAG_ACCOUNT_CREATE_PROOF_REQ: u8 = 26;
pub const TAG_ACCOUNT_CREATE_PROOF_RESP: u8 = 27;
pub const TAG_GET_NON_PRODUCT_ACCOUNTS_REQ: u8 = 28;
pub const TAG_GET_NON_PRODUCT_ACCOUNTS_RESP: u8 = 29;
pub const TAG_CREATE_TRANSACTION_REQ: u8 = 30;
pub const TAG_CREATE_TRANSACTION_RESP: u8 = 31;
pub const TAG_CREATE_TX_NON_PRODUCT_REQ: u8 = 32;
pub const TAG_CREATE_TX_NON_PRODUCT_RESP: u8 = 33;
pub const TAG_SIGN_RAW_REQ: u8 = 34;
pub const TAG_SIGN_RAW_RESP: u8 = 35;
pub const TAG_SIGN_PAYLOAD_REQ: u8 = 36;
pub const TAG_SIGN_PAYLOAD_RESP: u8 = 37;
pub const TAG_CHAT_CREATE_ROOM_REQ: u8 = 38;
pub const TAG_CHAT_CREATE_ROOM_RESP: u8 = 39;
pub const TAG_CHAT_REGISTER_BOT_REQ: u8 = 40;
pub const TAG_CHAT_REGISTER_BOT_RESP: u8 = 41;
pub const TAG_CHAT_LIST_START: u8 = 42;
pub const TAG_CHAT_LIST_STOP: u8 = 43;
pub const TAG_CHAT_LIST_INTERRUPT: u8 = 44;
pub const TAG_CHAT_LIST_RECEIVE: u8 = 45;
pub const TAG_CHAT_POST_MSG_REQ: u8 = 46;
pub const TAG_CHAT_POST_MSG_RESP: u8 = 47;
pub const TAG_CHAT_ACTION_START: u8 = 48;
pub const TAG_CHAT_ACTION_STOP: u8 = 49;
pub const TAG_CHAT_ACTION_INTERRUPT: u8 = 50;
pub const TAG_CHAT_ACTION_RECEIVE: u8 = 51;
pub const TAG_CHAT_CUSTOM_MSG_START: u8 = 52;
pub const TAG_CHAT_CUSTOM_MSG_STOP: u8 = 53;
pub const TAG_CHAT_CUSTOM_MSG_INTERRUPT: u8 = 54;
pub const TAG_CHAT_CUSTOM_MSG_RECEIVE: u8 = 55;
pub const TAG_STATEMENT_STORE_START: u8 = 56;
pub const TAG_STATEMENT_STORE_STOP: u8 = 57;
pub const TAG_STATEMENT_STORE_INTERRUPT: u8 = 58;
pub const TAG_STATEMENT_STORE_RECEIVE: u8 = 59;
pub const TAG_STATEMENT_PROOF_REQ: u8 = 60;
pub const TAG_STATEMENT_PROOF_RESP: u8 = 61;
pub const TAG_STATEMENT_SUBMIT_REQ: u8 = 62;
pub const TAG_STATEMENT_SUBMIT_RESP: u8 = 63;
pub const TAG_PREIMAGE_LOOKUP_START: u8 = 64;
pub const TAG_PREIMAGE_LOOKUP_STOP: u8 = 65;
pub const TAG_PREIMAGE_LOOKUP_INTERRUPT: u8 = 66;
pub const TAG_PREIMAGE_LOOKUP_RECEIVE: u8 = 67;
pub const TAG_PREIMAGE_SUBMIT_REQ: u8 = 68;
pub const TAG_PREIMAGE_SUBMIT_RESP: u8 = 69;
pub const TAG_JSONRPC_SEND_REQ: u8 = 70;
pub const TAG_JSONRPC_SEND_RESP: u8 = 71;
pub const TAG_JSONRPC_SUB_START: u8 = 72;
pub const TAG_JSONRPC_SUB_STOP: u8 = 73;
pub const TAG_JSONRPC_SUB_INTERRUPT: u8 = 74;
pub const TAG_JSONRPC_SUB_RECEIVE: u8 = 75;

/// Protocol version (JAM_CODEC_PROTOCOL_ID).
pub const PROTOCOL_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// High-level types
// ---------------------------------------------------------------------------

/// An account returned by host_get_non_product_accounts.
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    /// Raw public key bytes (typically 32 bytes for sr25519/ed25519).
    pub public_key: Vec<u8>,
    /// Optional display name.
    pub name: Option<String>,
}

/// Decoded incoming request from the app.
#[derive(Debug)]
pub enum HostRequest {
    Handshake { version: u8 },
    GetNonProductAccounts,
    FeatureSupported { feature_data: Vec<u8> },
    LocalStorageRead { key: String },
    LocalStorageWrite { key: String, value: Vec<u8> },
    LocalStorageClear { key: String },
    SignPayload { public_key: Vec<u8>, payload: Vec<u8> },
    SignRaw { public_key: Vec<u8>, data: Vec<u8> },
    CreateTransaction { payload: Vec<u8> },
    NavigateTo { url: String },
    AccountConnectionStatusStart,
    JsonRpcSend { data: Vec<u8> },
    JsonRpcSubscribeStart { data: Vec<u8> },
    /// A request we recognize by tag but don't handle yet.
    Unimplemented { tag: u8 },
    /// A tag we don't recognize at all.
    Unknown { tag: u8 },
}

/// Outgoing response to the app.
#[derive(Debug)]
pub enum HostResponse {
    HandshakeOk,
    AccountList(Vec<Account>),
    Error(String),
}

// ---------------------------------------------------------------------------
// Wire message decode / encode
// ---------------------------------------------------------------------------

/// Decode a raw binary message into (request_id, HostRequest).
pub fn decode_message(data: &[u8]) -> Result<(String, u8, HostRequest), DecodeErr> {
    let mut r = Reader::new(data);

    // requestId: SCALE string
    let request_id = r.read_string()?;

    // payload: enum tag (u8) + inner bytes
    let tag = r.read_u8()?;

    let req = match tag {
        TAG_HANDSHAKE_REQ => {
            // inner: Enum { v1: u8 }
            let _version_tag = r.read_u8()?; // 0 = "v1"
            let version = r.read_u8()?;
            HostRequest::Handshake { version }
        }
        TAG_GET_NON_PRODUCT_ACCOUNTS_REQ => {
            // inner: Enum { v1: void }
            let _version_tag = r.read_u8()?;
            // void = no more bytes
            HostRequest::GetNonProductAccounts
        }
        TAG_FEATURE_SUPPORTED_REQ => {
            let _version_tag = r.read_u8()?;
            let rest = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::FeatureSupported { feature_data: rest }
        }
        TAG_LOCAL_STORAGE_READ_REQ => {
            let _version_tag = r.read_u8()?;
            let key = r.read_string()?;
            HostRequest::LocalStorageRead { key }
        }
        TAG_LOCAL_STORAGE_WRITE_REQ => {
            let _version_tag = r.read_u8()?;
            let key = r.read_string()?;
            let value = r.read_var_bytes()?;
            HostRequest::LocalStorageWrite { key, value }
        }
        TAG_LOCAL_STORAGE_CLEAR_REQ => {
            let _version_tag = r.read_u8()?;
            let key = r.read_string()?;
            HostRequest::LocalStorageClear { key }
        }
        TAG_SIGN_PAYLOAD_REQ => {
            let _version_tag = r.read_u8()?;
            let public_key = r.read_var_bytes()?;
            let payload = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::SignPayload { public_key, payload }
        }
        TAG_SIGN_RAW_REQ => {
            let _version_tag = r.read_u8()?;
            let public_key = r.read_var_bytes()?;
            let data = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::SignRaw { public_key, data }
        }
        TAG_CREATE_TRANSACTION_REQ => {
            let _version_tag = r.read_u8()?;
            let rest = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::CreateTransaction { payload: rest }
        }
        TAG_NAVIGATE_TO_REQ => {
            let _version_tag = r.read_u8()?;
            let url = r.read_string()?;
            HostRequest::NavigateTo { url }
        }
        TAG_ACCOUNT_STATUS_START => {
            let _version_tag = r.read_u8()?;
            HostRequest::AccountConnectionStatusStart
        }
        TAG_JSONRPC_SEND_REQ => {
            let _version_tag = r.read_u8()?;
            let rest = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::JsonRpcSend { data: rest }
        }
        TAG_JSONRPC_SUB_START => {
            let _version_tag = r.read_u8()?;
            let rest = r.remaining().to_vec();
            r.skip_rest();
            HostRequest::JsonRpcSubscribeStart { data: rest }
        }
        // Known tags we don't handle yet
        TAG_PUSH_NOTIFICATION_REQ
        | TAG_DEVICE_PERMISSION_REQ
        | TAG_REMOTE_PERMISSION_REQ
        | TAG_ACCOUNT_GET_REQ
        | TAG_ACCOUNT_GET_ALIAS_REQ
        | TAG_ACCOUNT_CREATE_PROOF_REQ
        | TAG_CREATE_TX_NON_PRODUCT_REQ
        | TAG_CHAT_CREATE_ROOM_REQ
        | TAG_CHAT_REGISTER_BOT_REQ
        | TAG_CHAT_POST_MSG_REQ
        | TAG_STATEMENT_PROOF_REQ
        | TAG_STATEMENT_SUBMIT_REQ
        | TAG_PREIMAGE_SUBMIT_REQ => {
            HostRequest::Unimplemented { tag }
        }
        // Subscription stop/interrupt — fire-and-forget, no response needed
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
        | TAG_JSONRPC_SUB_INTERRUPT => {
            HostRequest::Unimplemented { tag }
        }
        _ => HostRequest::Unknown { tag },
    };

    Ok((request_id, tag, req))
}

/// Encode a response into a wire message.
pub fn encode_response(request_id: &str, request_tag: u8, response: &HostResponse) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);

    // requestId: SCALE string
    encode_string(&mut buf, request_id);

    match response {
        HostResponse::HandshakeOk => {
            // payload tag: host_handshake_response
            encode_tag(&mut buf, TAG_HANDSHAKE_RESP);
            // inner version tag: v1 = 0
            encode_tag(&mut buf, 0);
            // Result::Ok(void)
            encode_result_ok_void(&mut buf);
        }

        HostResponse::AccountList(accounts) => {
            // payload tag: host_get_non_product_accounts_response
            encode_tag(&mut buf, TAG_GET_NON_PRODUCT_ACCOUNTS_RESP);
            // inner version tag: v1 = 0
            encode_tag(&mut buf, 0);
            // Result::Ok(Vector(Account))
            encode_result_ok(&mut buf);
            // Vector: compact count + items
            encode_vector_len(&mut buf, accounts.len() as u32);
            for account in accounts {
                // Account = Struct { publicKey: Bytes(), name: Option(str) }
                // publicKey: dynamic bytes (compact len + raw)
                encode_var_bytes(&mut buf, &account.public_key);
                // name: Option(str)
                match &account.name {
                    None => encode_option_none(&mut buf),
                    Some(name) => {
                        encode_option_some(&mut buf);
                        encode_string(&mut buf, name);
                    }
                }
            }
        }

        HostResponse::Error(_reason) => {
            let resp_tag = response_tag_for(request_tag);
            encode_tag(&mut buf, resp_tag);
            encode_tag(&mut buf, 0); // v1
            encode_result_err(&mut buf);
            // GenericError / Unknown error variant (last in the enum)
            // Error = Struct { reason: str }
            // The exact error enum index depends on the method.
            // Use variant 0 (first error kind) as a generic rejection.
            encode_tag(&mut buf, 0);
        }
    }

    buf
}

/// Given a request tag, return the corresponding response tag.
/// Only valid for request/response pairs (even tags where tag+1 is the response).
/// Panics for subscription tags — those use dedicated encode functions.
fn response_tag_for(request_tag: u8) -> u8 {
    assert!(
        !matches!(
            request_tag,
            TAG_ACCOUNT_STATUS_START
                | TAG_CHAT_LIST_START
                | TAG_CHAT_ACTION_START
                | TAG_CHAT_CUSTOM_MSG_START
                | TAG_STATEMENT_STORE_START
                | TAG_PREIMAGE_LOOKUP_START
                | TAG_JSONRPC_SUB_START
        ),
        "response_tag_for called with subscription start tag {request_tag}"
    );
    request_tag + 1
}

/// Encode a feature_supported response (Result::Ok(bool)).
pub fn encode_feature_response(request_id: &str, supported: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_FEATURE_SUPPORTED_RESP);
    encode_tag(&mut buf, 0); // v1
    encode_result_ok(&mut buf);
    buf.push(if supported { 1 } else { 0 }); // bool as u8
    buf
}

/// Encode an account_connection_status_receive message.
pub fn encode_account_status(request_id: &str, connected: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_ACCOUNT_STATUS_RECEIVE);
    encode_tag(&mut buf, 0); // v1
    // Status enum: 0 = disconnected, 1 = connected
    encode_tag(&mut buf, if connected { 1 } else { 0 });
    buf
}

/// Encode a local_storage_read response.
pub fn encode_storage_read_response(request_id: &str, value: Option<&[u8]>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_LOCAL_STORAGE_READ_RESP);
    encode_tag(&mut buf, 0); // v1
    encode_result_ok(&mut buf);
    match value {
        None => encode_option_none(&mut buf),
        Some(v) => {
            encode_option_some(&mut buf);
            encode_var_bytes(&mut buf, v);
        }
    }
    buf
}

/// Encode a local_storage_write/clear response (Result::Ok(void)).
pub fn encode_storage_write_response(request_id: &str, is_clear: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    let tag = if is_clear {
        TAG_LOCAL_STORAGE_CLEAR_RESP
    } else {
        TAG_LOCAL_STORAGE_WRITE_RESP
    };
    encode_tag(&mut buf, tag);
    encode_tag(&mut buf, 0); // v1
    encode_result_ok_void(&mut buf);
    buf
}

/// Encode a navigate_to response (Result::Ok(void)).
pub fn encode_navigate_response(request_id: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_NAVIGATE_TO_RESP);
    encode_tag(&mut buf, 0); // v1
    encode_result_ok_void(&mut buf);
    buf
}

/// Encode a sign_payload or sign_raw success response.
/// Result::Ok { id: u32, signature: Bytes }
pub fn encode_sign_response(request_id: &str, is_raw: bool, signature: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(128);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, if is_raw { TAG_SIGN_RAW_RESP } else { TAG_SIGN_PAYLOAD_RESP });
    encode_tag(&mut buf, 0); // v1
    encode_result_ok(&mut buf);
    encode_compact_u32(&mut buf, 0); // id = 0
    encode_var_bytes(&mut buf, signature);
    buf
}

/// Encode a `host_jsonrpc_send` response.
///
/// The response wraps the JSON-RPC result (or error) as a SCALE string inside
/// the standard `Result<String, Error>` envelope.
pub fn encode_jsonrpc_send_response(request_id: &str, json_rpc_result: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64 + json_rpc_result.len());
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_JSONRPC_SEND_RESP);
    encode_tag(&mut buf, 0); // v1
    encode_result_ok(&mut buf);
    encode_string(&mut buf, json_rpc_result);
    buf
}

/// Encode a `host_jsonrpc_send` error response.
pub fn encode_jsonrpc_send_error(request_id: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, TAG_JSONRPC_SEND_RESP);
    encode_tag(&mut buf, 0); // v1
    encode_result_err(&mut buf);
    encode_tag(&mut buf, 0); // error variant 0
    buf
}

/// Encode a sign_payload or sign_raw error response (user rejected, wallet locked, etc).
pub fn encode_sign_error(request_id: &str, is_raw: bool) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    encode_string(&mut buf, request_id);
    encode_tag(&mut buf, if is_raw { TAG_SIGN_RAW_RESP } else { TAG_SIGN_PAYLOAD_RESP });
    encode_tag(&mut buf, 0); // v1
    encode_result_err(&mut buf);
    encode_tag(&mut buf, 0); // error variant 0 = Rejected
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_handshake_request() {
        // Manually encode: requestId="test", tag=0, v1=0, version=1
        let mut msg = Vec::new();
        encode_string(&mut msg, "test");
        msg.push(TAG_HANDSHAKE_REQ); // payload tag
        msg.push(0); // v1 tag
        msg.push(PROTOCOL_VERSION); // version value

        let (id, tag, req) = decode_message(&msg).unwrap();
        assert_eq!(id, "test");
        assert_eq!(tag, TAG_HANDSHAKE_REQ);
        match req {
            HostRequest::Handshake { version } => assert_eq!(version, 1),
            _ => panic!("expected Handshake"),
        }
    }

    #[test]
    fn encode_handshake_response() {
        let resp = encode_response("test", TAG_HANDSHAKE_REQ, &HostResponse::HandshakeOk);

        // Decode and verify structure
        let mut r = Reader::new(&resp);
        let id = r.read_string().unwrap();
        assert_eq!(id, "test");
        let tag = r.read_u8().unwrap();
        assert_eq!(tag, TAG_HANDSHAKE_RESP);
        let v1_tag = r.read_u8().unwrap();
        assert_eq!(v1_tag, 0);
        let result_ok = r.read_u8().unwrap();
        assert_eq!(result_ok, 0x00); // Ok
        assert_eq!(r.pos, resp.len()); // no trailing bytes
    }

    #[test]
    fn decode_get_non_product_accounts() {
        let mut msg = Vec::new();
        encode_string(&mut msg, "req-42");
        msg.push(TAG_GET_NON_PRODUCT_ACCOUNTS_REQ);
        msg.push(0); // v1

        let (id, tag, req) = decode_message(&msg).unwrap();
        assert_eq!(id, "req-42");
        assert_eq!(tag, TAG_GET_NON_PRODUCT_ACCOUNTS_REQ);
        assert!(matches!(req, HostRequest::GetNonProductAccounts));
    }

    #[test]
    fn encode_account_list_response() {
        let accounts = vec![
            Account {
                public_key: vec![0xd4; 32],
                name: Some("Alice".into()),
            },
            Account {
                public_key: vec![0x8e; 32],
                name: None,
            },
        ];
        let resp = encode_response(
            "req-42",
            TAG_GET_NON_PRODUCT_ACCOUNTS_REQ,
            &HostResponse::AccountList(accounts),
        );

        // Decode and verify structure
        let mut r = Reader::new(&resp);
        let id = r.read_string().unwrap();
        assert_eq!(id, "req-42");
        let tag = r.read_u8().unwrap();
        assert_eq!(tag, TAG_GET_NON_PRODUCT_ACCOUNTS_RESP);
        let v1 = r.read_u8().unwrap();
        assert_eq!(v1, 0); // v1
        let result = r.read_u8().unwrap();
        assert_eq!(result, 0x00); // Ok
        let count = r.read_compact_u32().unwrap();
        assert_eq!(count, 2);

        // Account 1: pubkey + Some("Alice")
        let pk1 = r.read_var_bytes().unwrap();
        assert_eq!(pk1.len(), 32);
        assert_eq!(pk1[0], 0xd4);
        let name1 = r.read_option(|r| r.read_string()).unwrap();
        assert_eq!(name1.as_deref(), Some("Alice"));

        // Account 2: pubkey + None
        let pk2 = r.read_var_bytes().unwrap();
        assert_eq!(pk2.len(), 32);
        assert_eq!(pk2[0], 0x8e);
        let name2 = r.read_option(|r| r.read_string()).unwrap();
        assert!(name2.is_none());

        assert_eq!(r.pos, resp.len());
    }

    #[test]
    fn handshake_round_trip() {
        // Simulate: app sends handshake request, host responds
        let mut req_msg = Vec::new();
        encode_string(&mut req_msg, "hsk-1");
        req_msg.push(TAG_HANDSHAKE_REQ);
        req_msg.push(0); // v1
        req_msg.push(PROTOCOL_VERSION);

        let (id, tag, req) = decode_message(&req_msg).unwrap();
        assert!(matches!(req, HostRequest::Handshake { version: 1 }));

        let resp_bytes = encode_response(&id, tag, &HostResponse::HandshakeOk);

        // Verify the response can be decoded
        let mut r = Reader::new(&resp_bytes);
        assert_eq!(r.read_string().unwrap(), "hsk-1");
        assert_eq!(r.read_u8().unwrap(), TAG_HANDSHAKE_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
    }

    // -------------------------------------------------------------------
    // Golden byte vectors — hand-verified against SCALE spec.
    //
    // Format: requestId (compact_len + UTF-8), tag (u8), version (u8), payload.
    // These catch accidental encoding drift.
    // -------------------------------------------------------------------

    #[test]
    fn golden_handshake_request() {
        // requestId = "t1" (compact_len=8, bytes "t1"), tag=0, v1=0, version=1
        let expected: &[u8] = &[
            0x08, b't', b'1', // compact(2) + "t1"
            0x00, // TAG_HANDSHAKE_REQ
            0x00, // v1
            0x01, // version = 1
        ];
        let mut built = Vec::new();
        encode_string(&mut built, "t1");
        built.push(TAG_HANDSHAKE_REQ);
        built.push(0);
        built.push(PROTOCOL_VERSION);
        assert_eq!(built, expected);
    }

    #[test]
    fn golden_handshake_response_ok() {
        let resp = encode_response("t1", TAG_HANDSHAKE_REQ, &HostResponse::HandshakeOk);
        let expected: &[u8] = &[
            0x08, b't', b'1', // compact(2) + "t1"
            0x01, // TAG_HANDSHAKE_RESP
            0x00, // v1
            0x00, // Result::Ok
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_get_accounts_request() {
        let expected: &[u8] = &[
            0x08, b'a', b'1', // compact(2) + "a1"
            28,   // TAG_GET_NON_PRODUCT_ACCOUNTS_REQ
            0x00, // v1
        ];
        let mut built = Vec::new();
        encode_string(&mut built, "a1");
        built.push(TAG_GET_NON_PRODUCT_ACCOUNTS_REQ);
        built.push(0);
        assert_eq!(built, expected);
    }

    #[test]
    fn golden_get_accounts_response_empty() {
        let resp = encode_response(
            "a1",
            TAG_GET_NON_PRODUCT_ACCOUNTS_REQ,
            &HostResponse::AccountList(vec![]),
        );
        let expected: &[u8] = &[
            0x08, b'a', b'1', // compact(2) + "a1"
            29,   // TAG_GET_NON_PRODUCT_ACCOUNTS_RESP
            0x00, // v1
            0x00, // Result::Ok
            0x00, // Vector len = 0
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_storage_write_response() {
        let resp = encode_storage_write_response("s1", false);
        let expected: &[u8] = &[
            0x08, b's', b'1', // compact(2) + "s1"
            15,   // TAG_LOCAL_STORAGE_WRITE_RESP
            0x00, // v1
            0x00, // Result::Ok(void)
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_storage_clear_response() {
        let resp = encode_storage_write_response("s1", true);
        let expected: &[u8] = &[
            0x08, b's', b'1', // compact(2) + "s1"
            17,   // TAG_LOCAL_STORAGE_CLEAR_RESP
            0x00, // v1
            0x00, // Result::Ok(void)
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_feature_supported_response() {
        let resp = encode_feature_response("f1", false);
        let expected: &[u8] = &[
            0x08, b'f', b'1', // compact(2) + "f1"
            3,    // TAG_FEATURE_SUPPORTED_RESP
            0x00, // v1
            0x00, // Result::Ok
            0x00, // false
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_account_status_receive() {
        let resp = encode_account_status("c1", true);
        let expected: &[u8] = &[
            0x08, b'c', b'1', // compact(2) + "c1"
            21,   // TAG_ACCOUNT_STATUS_RECEIVE
            0x00, // v1
            0x01, // connected = true
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn golden_sign_payload_response_ok() {
        let sig = [0xAB; 64];
        let resp = encode_sign_response("s1", false, &sig);
        let mut r = Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "s1");
        assert_eq!(r.read_u8().unwrap(), TAG_SIGN_PAYLOAD_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        assert_eq!(r.read_compact_u32().unwrap(), 0); // id
        let sig_bytes = r.read_var_bytes().unwrap();
        assert_eq!(sig_bytes, vec![0xAB; 64]);
        assert_eq!(r.pos, resp.len());
    }

    #[test]
    fn golden_sign_raw_response_ok() {
        let sig = [0xCD; 64];
        let resp = encode_sign_response("s2", true, &sig);
        let mut r = Reader::new(&resp);
        assert_eq!(r.read_string().unwrap(), "s2");
        assert_eq!(r.read_u8().unwrap(), TAG_SIGN_RAW_RESP);
        assert_eq!(r.read_u8().unwrap(), 0); // v1
        assert_eq!(r.read_u8().unwrap(), 0); // Result::Ok
        assert_eq!(r.read_compact_u32().unwrap(), 0); // id
        let sig_bytes = r.read_var_bytes().unwrap();
        assert_eq!(sig_bytes, vec![0xCD; 64]);
    }

    #[test]
    fn golden_sign_error_response() {
        let resp = encode_sign_error("s3", false);
        let expected: &[u8] = &[
            0x08, b's', b'3', // compact(2) + "s3"
            37,   // TAG_SIGN_PAYLOAD_RESP
            0x00, // v1
            0x01, // Result::Err
            0x00, // Rejected variant
        ];
        assert_eq!(resp, expected);
    }

    #[test]
    fn decode_sign_payload_request() {
        let mut msg = Vec::new();
        encode_string(&mut msg, "sign-1");
        msg.push(TAG_SIGN_PAYLOAD_REQ);
        msg.push(0); // v1
        encode_var_bytes(&mut msg, &[0xAA; 32]); // publicKey
        msg.extend_from_slice(b"extrinsic-payload"); // payload
        let (id, tag, req) = decode_message(&msg).unwrap();
        assert_eq!(id, "sign-1");
        assert_eq!(tag, TAG_SIGN_PAYLOAD_REQ);
        match req {
            HostRequest::SignPayload { public_key, payload } => {
                assert_eq!(public_key, vec![0xAA; 32]);
                assert_eq!(payload, b"extrinsic-payload");
            }
            _ => panic!("expected SignPayload"),
        }
    }

    #[test]
    fn decode_sign_raw_request() {
        let mut msg = Vec::new();
        encode_string(&mut msg, "sign-2");
        msg.push(TAG_SIGN_RAW_REQ);
        msg.push(0); // v1
        encode_var_bytes(&mut msg, &[0xBB; 32]); // publicKey
        msg.extend_from_slice(b"raw-data"); // data
        let (id, tag, req) = decode_message(&msg).unwrap();
        assert_eq!(id, "sign-2");
        assert_eq!(tag, TAG_SIGN_RAW_REQ);
        match req {
            HostRequest::SignRaw { public_key, data } => {
                assert_eq!(public_key, vec![0xBB; 32]);
                assert_eq!(data, b"raw-data");
            }
            _ => panic!("expected SignRaw"),
        }
    }
}
