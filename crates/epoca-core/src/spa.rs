//! SPA runtime — WKURLSchemeHandler, host API injection, and block-all rules
//! for sandboxed single-page application tabs.
//!
//! Assets are served from the in-memory `.prod` bundle via the `epocaapp://` scheme.
//! The page origin is `epocaapp://<app_id>/` — no HTTP origin to escape from.
//! A `block-all` WKContentRuleList prevents any outbound HTTP(S) requests.
//! The host API (`window.epoca`) is injected at document start.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Global registry of loaded SPA bundles, keyed by app_id.
/// The WKURLSchemeHandler callback looks up assets here.
static SPA_ASSETS: OnceLock<Mutex<HashMap<String, HashMap<String, Vec<u8>>>>> = OnceLock::new();

fn spa_assets() -> &'static Mutex<HashMap<String, HashMap<String, Vec<u8>>>> {
    SPA_ASSETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a bundle's assets so the scheme handler can serve them.
pub fn register_spa_assets(app_id: &str, assets: HashMap<String, Vec<u8>>) {
    let mut map = spa_assets().lock().unwrap();
    map.insert(app_id.to_string(), assets);
}

/// Unregister a bundle's assets when the tab is closed.
pub fn unregister_spa_assets(app_id: &str) {
    let mut map = spa_assets().lock().unwrap();
    map.remove(app_id);
}

/// Look up a single asset by app_id and path.
pub fn lookup_spa_asset(app_id: &str, path: &str) -> Option<Vec<u8>> {
    let map = spa_assets().lock().unwrap();
    map.get(app_id)?.get(path).cloned()
}

/// Guess MIME type from file extension.
pub fn mime_for_path(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "application/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Block-all WKContentRuleList — prevents outbound HTTP(S) from SPA WebViews
// ---------------------------------------------------------------------------

/// JSON rule that blocks all HTTP/HTTPS loads. Installed on SPA WebViews
/// separately from the shield (which is for browsing tabs).
const BLOCK_ALL_RULE_JSON: &str = r#"[{"trigger":{"url-filter":"^https?://"},"action":{"type":"block"}}]"#;

/// Install a block-all content rule on the given WKUserContentController.
#[cfg(target_os = "macos")]
pub fn install_block_all_rule(uc: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let store_cls = match AnyClass::get("WKContentRuleListStore") {
        Some(c) => c,
        None => return,
    };
    let store: *mut AnyObject = unsafe { msg_send![store_cls, defaultStore] };
    if store.is_null() {
        return;
    }

    let ns_id: *mut AnyObject = unsafe {
        msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epoca-spa-block-all\0".as_ptr() as *const i8
        ]
    };
    let json_cstr = std::ffi::CString::new(BLOCK_ALL_RULE_JSON).unwrap();
    let ns_json: *mut AnyObject = unsafe {
        msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: json_cstr.as_ptr()
        ]
    };

    let uc_ptr = uc as usize;
    let block = block2::RcBlock::new(
        move |list: *mut AnyObject, error: *mut AnyObject| unsafe {
            if !error.is_null() {
                let desc: *mut AnyObject = msg_send![error, localizedDescription];
                if !desc.is_null() {
                    let cstr: *const i8 = msg_send![desc, UTF8String];
                    if !cstr.is_null() {
                        let msg = std::ffi::CStr::from_ptr(cstr).to_string_lossy();
                        log::warn!("SPA block-all rule compilation failed: {msg}");
                    }
                }
                return;
            }
            if list.is_null() {
                return;
            }
            // Dispatch addContentRuleList: to the main thread — the completion
            // handler fires on an arbitrary WebKit background thread, but
            // WKUserContentController must be mutated on main.
            // dispatch_get_main_queue() is a C macro — the real symbol is
            // _dispatch_main_q. Use dispatch_async_f to bounce to main thread.
            extern "C" {
                static _dispatch_main_q: std::ffi::c_void;
                fn dispatch_async_f(
                    queue: *const std::ffi::c_void,
                    context: *mut std::ffi::c_void,
                    work: extern "C" fn(*mut std::ffi::c_void),
                );
            }
            extern "C" fn add_rule_on_main(ctx: *mut std::ffi::c_void) {
                unsafe {
                    let pair = Box::from_raw(ctx as *mut [usize; 2]);
                    let uc = pair[0] as *mut objc2::runtime::AnyObject;
                    let list = pair[1] as *mut objc2::runtime::AnyObject;
                    let _: () = objc2::msg_send![uc, addContentRuleList: list];
                    log::info!("SPA: installed block-all content rule");
                }
            }
            let pair = Box::new([uc_ptr, list as usize]);
            dispatch_async_f(
                std::ptr::addr_of!(_dispatch_main_q) as *const std::ffi::c_void,
                Box::into_raw(pair) as *mut std::ffi::c_void,
                add_rule_on_main,
            );
        },
    );

    unsafe {
        let _: () = msg_send![store,
            compileContentRuleListForIdentifier: ns_id
            encodedContentRuleList: ns_json
            completionHandler: &*block
        ];
    }
}

// ---------------------------------------------------------------------------
// Host API injection script — window.epoca
// ---------------------------------------------------------------------------

/// JavaScript injected at document start into every SPA WebView.
/// Provides `window.epoca` with methods that communicate back to the Rust host
/// via WKScriptMessageHandler.
pub const HOST_API_SCRIPT: &str = r#"
(function() {
    'use strict';
    if (window.__epocaHostApi) return;
    window.__epocaHostApi = true;

    // Correlation ID counter for request/response matching.
    let _nextId = 1;
    const _pending = new Map();

    // Internal: send a message to the host and return a Promise.
    function _call(channel, method, params) {
        const id = _nextId++;
        return new Promise((resolve, reject) => {
            _pending.set(id, { resolve, reject });
            window.webkit.messageHandlers[channel].postMessage({
                id: id,
                method: method,
                params: params || {}
            });
        });
    }

    // Event listeners — kept outside the frozen object so on()/off() can mutate.
    const _listeners = {};

    // Host resolves/rejects pending calls via this non-writable global callback.
    Object.defineProperty(window, '__epocaResolve', {
        value: function(id, error, result) {
            const p = _pending.get(id);
            if (!p) return;
            _pending.delete(id);
            if (error) {
                p.reject(new Error(error));
            } else {
                p.resolve(result);
            }
        },
        writable: false,
        configurable: false
    });

    window.epoca = Object.freeze({
        // Request the host to sign a payload. Returns the signature.
        // The user will see a confirmation dialog before signing proceeds.
        sign: function(payload) {
            return _call('epocaHost', 'sign', { payload: payload });
        },

        // Get the app's public address (derived from the injected identity).
        getAddress: function() {
            return _call('epocaHost', 'getAddress', {});
        },

        // Statement Store API
        statementStore: Object.freeze({
            write: function(channel, data) {
                return _call('epocaHost', 'storeWrite', { channel: channel, data: data });
            },
            subscribe: function(channel) {
                return _call('epocaHost', 'storeSubscribe', { channel: channel });
            }
        }),

        // WebSocket proxy — host mediates the connection.
        ws: Object.freeze({
            connect: function(url) {
                return _call('epocaHost', 'wsConnect', { url: url });
            },
            send: function(connId, data) {
                return _call('epocaHost', 'wsSend', { connId: connId, data: data });
            },
            close: function(connId) {
                return _call('epocaHost', 'wsClose', { connId: connId });
            }
        }),

        // Event subscription — listeners stored in closure scope (not frozen).
        on: function(event, callback) {
            if (!_listeners[event]) {
                _listeners[event] = [];
            }
            _listeners[event].push(callback);
        },
        off: function(event, callback) {
            const list = _listeners[event];
            if (list) {
                const i = list.indexOf(callback);
                if (i >= 0) list.splice(i, 1);
            }
        }
    });

    // Host pushes events to the app via this non-writable global.
    Object.defineProperty(window, '__epocaPush', {
        value: function(event, data) {
            const list = _listeners[event];
            if (list) {
                for (const cb of list) {
                    try { cb(data); } catch(e) { console.error('epoca event error:', e); }
                }
            }
        },
        writable: false,
        configurable: false
    });
})();
"#;

// ---------------------------------------------------------------------------
// epocaHost WKScriptMessageHandler — receives calls from window.epoca
// ---------------------------------------------------------------------------

/// Channel for host API calls from SPA WebViews.
/// Tuple: (webview_ptr, id, method, params_json)
static SPA_HOST_CHANNEL: OnceLock<(
    std::sync::mpsc::SyncSender<(usize, u64, String, String)>,
    Mutex<std::sync::mpsc::Receiver<(usize, u64, String, String)>>,
)> = OnceLock::new();

fn spa_host_channel() -> &'static (
    std::sync::mpsc::SyncSender<(usize, u64, String, String)>,
    Mutex<std::sync::mpsc::Receiver<(usize, u64, String, String)>>,
) {
    SPA_HOST_CHANNEL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(256);
        (tx, Mutex::new(rx))
    })
}

/// Drain pending host API calls (called from Workbench render loop).
pub fn drain_spa_host_events() -> Vec<(usize, u64, String, String)> {
    let ch = spa_host_channel();
    let rx = ch.1.lock().unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// UC → webview_ptr mapping for the SPA host handler.
static HOST_UC_MAP: std::sync::LazyLock<Mutex<HashMap<usize, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Remove a webview_ptr from the HOST_UC_MAP when an SPA tab is closed.
/// Prevents dangling pointer accumulation.
pub fn unregister_host_handler(webview_ptr: usize) {
    let mut map = HOST_UC_MAP.lock().unwrap();
    map.retain(|_uc, wv| *wv != webview_ptr);
}

/// Register the `epocaHost` WKScriptMessageHandler on the given
/// WKUserContentController for SPA tabs.
#[cfg(target_os = "macos")]
pub fn register_host_handler(uc: *mut objc2::runtime::AnyObject, webview_ptr: usize) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaHostHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaHostHandler", superclass).unwrap();

            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                uc: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                let body: *mut AnyObject = msg_send![message, body];
                if body.is_null() {
                    return;
                }

                let id_key: *mut AnyObject = msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"id\0".as_ptr() as *const i8
                ];
                let method_key: *mut AnyObject = msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"method\0".as_ptr() as *const i8
                ];
                let params_key: *mut AnyObject = msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"params\0".as_ptr() as *const i8
                ];

                let id_val: *mut AnyObject = msg_send![body, objectForKey: id_key];
                let method_val: *mut AnyObject = msg_send![body, objectForKey: method_key];
                let params_val: *mut AnyObject = msg_send![body, objectForKey: params_key];

                if id_val.is_null() || method_val.is_null() {
                    return;
                }

                let id_num: u64 = msg_send![id_val, unsignedLongLongValue];
                let method_cstr: *const i8 = msg_send![method_val, UTF8String];
                if method_cstr.is_null() {
                    return;
                }
                let method = std::ffi::CStr::from_ptr(method_cstr)
                    .to_string_lossy()
                    .to_string();

                // Serialize params dict to JSON string
                let params_json = if params_val.is_null() {
                    "{}".to_string()
                } else {
                    let json_data: *mut AnyObject = msg_send![
                        AnyClass::get("NSJSONSerialization").unwrap(),
                        dataWithJSONObject: params_val
                        options: 0u64
                        error: std::ptr::null_mut::<*mut AnyObject>()
                    ];
                    if json_data.is_null() {
                        "{}".to_string()
                    } else {
                        let alloc: *mut AnyObject = msg_send![
                            AnyClass::get("NSString").unwrap(),
                            alloc
                        ];
                        let ns_str: *mut AnyObject = msg_send![alloc,
                            initWithData: json_data
                            encoding: 4u64
                        ];
                        if ns_str.is_null() {
                            "{}".to_string()
                        } else {
                            let cstr: *const i8 = msg_send![ns_str, UTF8String];
                            if cstr.is_null() {
                                "{}".to_string()
                            } else {
                                std::ffi::CStr::from_ptr(cstr)
                                    .to_string_lossy()
                                    .to_string()
                            }
                        }
                    }
                };

                let uc_addr = uc as usize;
                let webview_ptr = HOST_UC_MAP.lock().unwrap().get(&uc_addr).copied().unwrap_or(0);

                let _ = spa_host_channel().0.try_send((webview_ptr, id_num, method, params_json));
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

    HOST_UC_MAP.lock().unwrap().insert(uc as usize, webview_ptr);

    unsafe {
        let handler: *mut objc2::runtime::AnyObject = msg_send![*cls, new];
        if handler.is_null() {
            return;
        }
        let name: *mut objc2::runtime::AnyObject = msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaHost\0".as_ptr() as *const i8
        ];
        let _: () = msg_send![uc, addScriptMessageHandler: handler name: name];
        log::info!("SPA: registered epocaHost handler (webview_ptr={webview_ptr:#x})");
    }
}
