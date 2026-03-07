/// Polkadot app host-api bridge — connects the JS shim in WKWebView to
/// the Rust HostApi via WKScriptMessageHandler.
///
/// The JS shim (`HOST_API_BRIDGE_SCRIPT` from `epoca-hostapi`) intercepts
/// binary SCALE messages on a MessagePort and forwards them to native as
/// base64 strings. This module registers the `epocaHostApi` message handler,
/// decodes the base64, runs it through `HostApi::handle_message()`, and
/// sends the response back via `evaluateJavaScript`.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use epoca_hostapi::protocol::Account;
use epoca_hostapi::{HostApi, HostApiOutcome};

// ---------------------------------------------------------------------------
// GPUI Global — shared HostApi instance
// ---------------------------------------------------------------------------

pub struct HostApiGlobal {
    pub api: Mutex<HostApi>,
}

impl gpui::Global for HostApiGlobal {}

/// Initialize the HostApi global. Call once at startup.
/// Accounts are synced dynamically from the wallet in the drain loop.
pub fn init_hostapi(cx: &mut gpui::App) {
    let api = HostApi::new();
    cx.set_global(HostApiGlobal {
        api: Mutex::new(api),
    });
}

/// Sync the host-api accounts list from the wallet.
/// Called from the workbench drain loop before processing events.
pub fn sync_accounts_from_wallet(
    api: &mut HostApi,
    wallet: &crate::wallet::WalletGlobal,
) {
    let accounts = match wallet.manager.root_public_key() {
        Some(pubkey) => vec![Account {
            public_key: pubkey.to_vec(),
            name: Some("Wallet".into()),
        }],
        None => vec![],
    };
    api.set_accounts(accounts);
}

// ---------------------------------------------------------------------------
// Message channel — WKScriptMessageHandler → Rust → evaluateJavaScript
// ---------------------------------------------------------------------------

/// Incoming message from JS: (webview_ptr, base64_encoded_scale_bytes)
static HOSTAPI_CHANNEL: OnceLock<(
    std::sync::mpsc::SyncSender<(usize, String)>,
    Mutex<std::sync::mpsc::Receiver<(usize, String)>>,
)> = OnceLock::new();

fn hostapi_channel() -> &'static (
    std::sync::mpsc::SyncSender<(usize, String)>,
    Mutex<std::sync::mpsc::Receiver<(usize, String)>>,
) {
    HOSTAPI_CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(256);
        (tx, Mutex::new(rx))
    })
}

/// Drain pending host-api messages. Called from Workbench render loop.
pub fn drain_hostapi_events() -> Vec<(usize, String)> {
    let ch = hostapi_channel();
    let rx = ch.1.lock().unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

// ---------------------------------------------------------------------------
// UC → webview_ptr mapping
// ---------------------------------------------------------------------------

static HOSTAPI_UC_MAP: std::sync::LazyLock<Mutex<HashMap<usize, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn unregister_hostapi_handler(webview_ptr: usize) {
    let mut map = HOSTAPI_UC_MAP.lock().unwrap();
    map.retain(|_uc, wv| *wv != webview_ptr);
}

// ---------------------------------------------------------------------------
// Register WKScriptMessageHandler
// ---------------------------------------------------------------------------

/// Register the `epocaHostApi` WKScriptMessageHandler on a WKUserContentController.
/// The handler receives base64-encoded SCALE binary from the JS bridge.
#[cfg(target_os = "macos")]
pub fn register_hostapi_handler(uc: *mut objc2::runtime::AnyObject, webview_ptr: usize) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaHostApiHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaHostApiHandler", superclass).unwrap();

            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                uc: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                // body is an NSString (base64-encoded SCALE binary)
                let body: *mut AnyObject = msg_send![message, body];
                if body.is_null() {
                    return;
                }

                let cstr: *const i8 = msg_send![body, UTF8String];
                if cstr.is_null() {
                    return;
                }
                let b64 = std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .to_string();

                let uc_addr = uc as usize;
                let webview_ptr = HOSTAPI_UC_MAP
                    .lock()
                    .unwrap()
                    .get(&uc_addr)
                    .copied()
                    .unwrap_or(0);

                log::info!("[sign-debug] ObjC handler: received b64 ({} chars) from webview {webview_ptr:#x}", b64.len());
                if hostapi_channel().0.try_send((webview_ptr, b64)).is_err() {
                    log::warn!("hostapi: channel full, dropping message from webview {webview_ptr:#x}");
                }
            }

            builder.add_method(
                objc2::sel!(userContentController:didReceiveScriptMessage:),
                did_receive as unsafe extern "C" fn(_, _, _, _),
            );

            if let Some(proto) = objc2::runtime::AnyProtocol::get("WKScriptMessageHandler") {
                builder.add_protocol(proto);
            }

            builder.register()
        }
    });

    HOSTAPI_UC_MAP
        .lock()
        .unwrap()
        .insert(uc as usize, webview_ptr);

    unsafe {
        let handler: *mut objc2::runtime::AnyObject = msg_send![*cls, new];
        if handler.is_null() {
            return;
        }
        let name: *mut objc2::runtime::AnyObject = msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaHostApi\0".as_ptr() as *const i8
        ];
        let _: () = msg_send![uc, addScriptMessageHandler: handler name: name];
        // Balance the +1 retain from [cls new] — addScriptMessageHandler retains its own ref.
        let _: () = msg_send![handler, release];
        log::info!(
            "hostapi: registered epocaHostApi handler (webview_ptr={webview_ptr:#x})"
        );
    }
}

// ---------------------------------------------------------------------------
// Base64 helpers
// ---------------------------------------------------------------------------

const B64_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let input = input.as_bytes();
    let mut buf = Vec::with_capacity(input.len() * 3 / 4);
    let mut accum = 0u32;
    let mut bits = 0u32;

    for &c in input {
        let val = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        accum = (accum << 6) | val as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            buf.push((accum >> bits) as u8);
            accum &= (1 << bits) - 1;
        }
    }
    Some(buf)
}

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(B64_CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(B64_CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(B64_CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Send a binary response back to the JS bridge in the specified webview.
/// Evaluates `window.__epocaHostApiReply("base64...")` on the webview.
pub fn send_response(webview_ptr: usize, response_bytes: &[u8]) {
    let b64 = base64_encode(response_bytes);
    let js = format!("window.__epocaHostApiReply('{b64}')");

    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};

        unsafe {
            let wv = webview_ptr as *mut AnyObject;
            if wv.is_null() {
                return;
            }
            let js_cstr = std::ffi::CString::new(js).unwrap();
            let ns_js: *mut AnyObject = msg_send![
                AnyClass::get("NSString").unwrap(),
                stringWithUTF8String: js_cstr.as_ptr()
            ];
            let _: () = msg_send![wv,
                evaluateJavaScript: ns_js
                completionHandler: std::ptr::null::<AnyObject>()
            ];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip() {
        let cases: &[&[u8]] = &[
            b"",
            b"\x00",
            b"\xff",
            b"hello",
            b"hello world!",
            b"\x00\x01\x02\x03\x04\x05",
            &[0xAA; 32],  // typical public key
            &[0xFF; 100], // longer payload
        ];
        for data in cases {
            let encoded = base64_encode(data);
            let decoded = base64_decode(&encoded).expect("decode should succeed");
            assert_eq!(*data, decoded.as_slice(), "round-trip failed for {} bytes", data.len());
        }
    }

    #[test]
    fn base64_decode_invalid_char() {
        assert!(base64_decode("abc!def").is_none());
    }

    #[test]
    fn base64_decode_empty() {
        assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn base64_decode_padding() {
        // "YQ==" encodes "a"
        assert_eq!(base64_decode("YQ==").unwrap(), b"a");
        // "YWI=" encodes "ab"
        assert_eq!(base64_decode("YWI=").unwrap(), b"ab");
        // "YWJj" encodes "abc"
        assert_eq!(base64_decode("YWJj").unwrap(), b"abc");
    }

    #[test]
    fn base64_encode_output_is_js_safe() {
        // Verify the encoder only produces [A-Za-z0-9+/=] — no quotes or backslashes
        let data: Vec<u8> = (0..=255).collect();
        let encoded = base64_encode(&data);
        for c in encoded.chars() {
            assert!(
                c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=',
                "unexpected char in base64 output: {c:?}"
            );
        }
    }
}
