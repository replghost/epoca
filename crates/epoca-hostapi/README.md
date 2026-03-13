# epoca-hostapi

Transport-agnostic host API engine for Polkadot app runtimes. Processes binary
SCALE-encoded messages from apps and returns binary responses. No framework
dependencies — only `log`.

## Architecture

```
App (JS/Wasm)                   Host (Rust native)
--------------                  ------------------
window.__HOST_API_PORT__  --->  [Transport layer]
  MessageChannel binary           WKScriptMessageHandler (macOS)
  or PolkaVM host call            or PolkaVM import

                                    |
                                    v
                                HostApi::handle_message(raw_bytes, app_id)
                                    |
                                    v
                              HostApiOutcome
                                Response(bytes)    -> send back to app
                                NeedsSign{..}      -> wallet approval, then encode_sign_response()
                                Silent             -> no reply needed
```

The crate handles decode/encode only. Transport (WKWebView, PolkaVM, WebSocket,
etc.) is the caller's responsibility.

## Wire Format

All messages use a minimal SCALE codec (see `codec.rs`):

```
Message = requestId: str, payload: Enum { ...variants... }

str       = compact_u32(byte_len) ++ utf8_bytes
compact   = SCALE compact integer (1/2/4/5 bytes)
Enum      = u8 tag ++ variant_data
Option<T> = 0x00 (None) | 0x01 ++ T (Some)
Result    = 0x00 ++ T (Ok) | 0x01 ++ E (Err)
Vec<T>    = compact_u32(len) ++ T*len
Bytes     = compact_u32(len) ++ raw_bytes
```

### Request flow

```
[compact_len "req-1"] [tag: u8] [version: 0x00] [method-specific fields...]
 ^-- requestId          ^-- method   ^-- v1
```

### Response flow

```
[compact_len "req-1"] [resp_tag: u8] [version: 0x00] [Result: 0x00=Ok|0x01=Err] [data...]
 ^-- same requestId     ^-- tag+1      ^-- v1
```

## Protocol Tags

Request/response pairs use adjacent tags (N=request, N+1=response).
Subscriptions use 4 tags: start, stop, interrupt, receive.

| Tag | Method |
|-----|--------|
| 0/1 | Handshake |
| 2/3 | FeatureSupported |
| 4/5 | PushNotification |
| 6/7 | NavigateTo |
| 12/13 | LocalStorageRead |
| 14/15 | LocalStorageWrite |
| 16/17 | LocalStorageClear |
| 18-21 | AccountConnectionStatus (sub) |
| 28/29 | GetNonProductAccounts |
| 30/31 | CreateTransaction |
| 34/35 | SignRaw |
| 36/37 | SignPayload |
| 70/71 | JsonRpcSend |
| 72-75 | JsonRpcSubscribe (sub) |

Full tag list in `protocol.rs`.

## Usage

```rust
use epoca_hostapi::{HostApi, HostApiOutcome, protocol::Account};

let mut api = HostApi::new();

// Provide wallet accounts to the app.
api.set_accounts(vec![Account {
    public_key: pubkey_bytes.to_vec(),
    name: Some("Alice".into()),
}]);

// Process an incoming binary message from the app.
match api.handle_message(&raw_bytes, "my-app-id") {
    HostApiOutcome::Response(reply) => {
        // Send `reply` bytes back to the app.
        transport.send(reply);
    }
    HostApiOutcome::NeedsSign { request_id, request_tag, public_key, payload } => {
        // Show wallet approval UI, then:
        let reply = if approved {
            epoca_hostapi::protocol::encode_sign_response(
                &request_id,
                request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ,
                &signature_bytes,
            )
        } else {
            epoca_hostapi::protocol::encode_sign_error(
                &request_id,
                request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ,
            )
        };
        transport.send(reply);
    }
    HostApiOutcome::Silent => {
        // No response needed (subscription stop/interrupt, malformed input).
    }
}
```

## JS Bridge

The crate includes `HOST_API_BRIDGE_SCRIPT` — a JavaScript snippet that sets up
a `MessageChannel` between the app SDK and the native host. It:

1. Sets `window.__HOST_WEBVIEW_MARK__ = true` (SDK detection)
2. Creates a `MessageChannel`, exposes `port2` as `window.__HOST_API_PORT__`
3. Forwards binary messages from `port1` to native via
   `webkit.messageHandlers.epocaHostApi.postMessage(base64)`
4. Receives native replies via `window.__epocaHostApiReply(base64)`

This is the low-level binary transport bridge. The higher-level `window.host`
JS API (with `sign()`, `getAddress()`, `chain.query()`, etc.) is
application-specific and lives outside this crate.

## Implemented Methods

| Method | Status |
|--------|--------|
| Handshake | Done |
| GetNonProductAccounts | Done |
| FeatureSupported | Done (always false) |
| AccountConnectionStatus | Done (always connected) |
| LocalStorage Read/Write/Clear | Done (app-scoped) |
| SignPayload / SignRaw | Done (returns NeedsSign) |
| NavigateTo | Done |
| CreateTransaction | Stub (returns error) |
| JsonRpc Send/Subscribe | Stub (returns error) |

## Local Storage Scoping

Local storage keys are namespaced by `app_id` — app A cannot read app B's data.
Internally: `"{app_id}\0{key}"`.

## Modules

- **`codec`** — SCALE primitives: `Reader`, `encode_compact_u32`, `encode_string`, etc.
- **`protocol`** — Wire tags, `HostRequest`/`HostResponse` enums, encode/decode functions.
- **`lib`** — `HostApi` engine, `HostApiOutcome`, JS bridge script.
