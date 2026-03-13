//! SPA runtime — WKURLSchemeHandler, host API injection, and block-all rules
//! for sandboxed single-page application tabs.
//!
//! Assets are served from the in-memory `.prod` bundle via the `epocaapp://` scheme.
//! The page origin is `epocaapp://<app_id>/` — no HTTP origin to escape from.
//! A `block-all` WKContentRuleList prevents any outbound HTTP(S) requests.
//! The host API (`window.host`) is injected at document start.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// How assets are served for a given SPA.
enum AssetSource {
    /// All assets loaded in memory (local .prod files).
    Eager(HashMap<String, Vec<u8>>),
    /// Assets fetched on-demand from IPFS gateway, cached locally.
    Lazy {
        /// IPFS CID of the bundle root directory.
        cid: String,
        /// IPFS gateway URL (e.g. "https://ipfs.dotspark.app").
        gateway: String,
        /// Cache of already-fetched assets.
        cache: Mutex<HashMap<String, Vec<u8>>>,
    },
}

/// Global registry of loaded SPA bundles, keyed by app_id.
/// The WKURLSchemeHandler callback looks up assets here.
/// Each entry is reference-counted so multiple tabs with the same app_id
/// share the asset source and it's only freed when the last tab closes.
static SPA_ASSETS: OnceLock<Mutex<HashMap<String, (u32, Arc<AssetSource>)>>> = OnceLock::new();

fn spa_assets() -> &'static Mutex<HashMap<String, (u32, Arc<AssetSource>)>> {
    SPA_ASSETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a bundle's assets (eager, all in memory).
/// If the app_id already exists, increments the reference count.
pub fn register_spa_assets(app_id: &str, assets: HashMap<String, Vec<u8>>) {
    let mut map = spa_assets().lock().unwrap();
    map.entry(app_id.to_string())
        .and_modify(|(rc, _)| *rc += 1)
        .or_insert((1, Arc::new(AssetSource::Eager(assets))));
}

/// Register an IPFS-backed lazy asset source.
/// Assets are fetched from the gateway on first access and cached.
/// `initial_assets` may contain pre-fetched files (e.g. manifest.toml).
pub fn register_spa_assets_lazy(
    app_id: &str,
    cid: &str,
    gateway: &str,
    initial_assets: HashMap<String, Vec<u8>>,
) {
    let mut map = spa_assets().lock().unwrap();
    map.entry(app_id.to_string())
        .and_modify(|(rc, _)| *rc += 1)
        .or_insert((
            1,
            Arc::new(AssetSource::Lazy {
                cid: cid.to_string(),
                gateway: gateway.to_string(),
                cache: Mutex::new(initial_assets),
            }),
        ));
}

/// Unregister a bundle's assets when the tab is closed.
/// Decrements the reference count; only removes assets when it reaches zero.
pub fn unregister_spa_assets(app_id: &str) {
    let mut map = spa_assets().lock().unwrap();
    if let Some((rc, _)) = map.get_mut(app_id) {
        *rc = rc.saturating_sub(1);
        if *rc == 0 {
            map.remove(app_id);
        }
    }
}

/// Look up a single asset by app_id and path.
/// For lazy sources, fetches from IPFS on cache miss (blocks the calling thread).
pub fn lookup_spa_asset(app_id: &str, path: &str) -> Option<Vec<u8>> {
    // Reject path traversal attempts.
    if path.contains("..") || path.starts_with('/') {
        log::warn!("[spa] rejected path traversal attempt: {path}");
        return None;
    }
    let source = {
        let map = spa_assets().lock().unwrap();
        map.get(app_id)?.1.clone()
    };

    match &*source {
        AssetSource::Eager(assets) => assets.get(path).cloned(),
        AssetSource::Lazy {
            cid,
            gateway,
            cache,
        } => {
            // Check cache first.
            {
                let c = cache.lock().unwrap();
                if let Some(data) = c.get(path) {
                    return Some(data.clone());
                }
            }
            // Fetch from IPFS gateway.
            let url = format!("{gateway}/ipfs/{cid}/{path}");
            log::info!("[spa-lazy] fetching: {path} from {url}");
            match fetch_ipfs_asset(&url) {
                Ok(data) => {
                    log::info!("[spa-lazy] cached: {path} ({} bytes)", data.len());
                    let mut c = cache.lock().unwrap();
                    c.insert(path.to_string(), data.clone());
                    Some(data)
                }
                Err(e) => {
                    log::warn!("[spa-lazy] failed to fetch {path}: {e}");
                    None
                }
            }
        }
    }
}

/// Fetch a single file from an IPFS gateway URL.
fn fetch_ipfs_asset(url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build(),
    );
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("fetch failed: {e}"))?;
    resp.into_body()
        .with_config()
        .limit(10 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("read failed: {e}"))
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
const BLOCK_ALL_RULE_JSON: &str = r#"[{"trigger":{"url-filter":"^(https?|wss?)://"},"action":{"type":"block"}}]"#;

/// Install a block-all content rule on the given WKUserContentController.
#[cfg(target_os = "macos")]
/// Install a block-all content rule on the given WKUserContentController.
///
/// The UC pointer is **retained** before the async compilation callback so it
/// stays alive until the main-thread dispatch completes. `[WKWebView configuration]`
/// returns a copy, and the UC from that copy is autoreleased — without an explicit
/// retain it becomes dangling by the time the callback fires.
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

    // Retain the UC so it survives until the async callback fires.
    unsafe { let _: *mut AnyObject = msg_send![uc, retain]; }
    let uc_ptr = uc as usize;

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
                // Release the retained UC even on error.
                let uc = uc_ptr as *mut AnyObject;
                let _: () = msg_send![uc, release];
                return;
            }
            if list.is_null() {
                let uc = uc_ptr as *mut AnyObject;
                let _: () = msg_send![uc, release];
                return;
            }
            // Dispatch to main thread — the completion handler fires on a
            // WebKit background thread, but WKUserContentController must be
            // mutated on main.
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
                    // Balance the retain from install_block_all_rule.
                    let _: () = objc2::msg_send![uc, release];
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
// Host API injection script — window.host
// ---------------------------------------------------------------------------

/// JavaScript injected at document start into every SPA WebView.
/// Provides `window.host` with methods that communicate back to the Rust host
/// via WKScriptMessageHandler.
pub const HOST_API_SCRIPT: &str = r#"
(function() {
    'use strict';
    console.log('[epoca-host-api] running, guard=' + !!window.__epocaHostApi + ' bridge=' + !!window.__epocaHostApiBridge);

    // Remove WebRTC constructors from app-visible scope entirely.
    // The host recovers them via a hidden iframe in evaluateScript when needed.
    // getUserMedia is intentionally left available (harmless without RTCPeerConnection).
    //
    // SECURITY: Do NOT stash constructors anywhere on window. Any property
    // (including Symbol-keyed, non-enumerable) is discoverable via
    // Object.getOwnPropertyNames/Symbols. Instead, the host's evaluateScript
    // creates a temporary about:blank iframe to obtain fresh native constructors
    // on demand (see media_api::get_rtc_from_iframe_js).
    window.RTCPeerConnection = undefined;
    window.RTCSessionDescription = undefined;
    window.RTCIceCandidate = undefined;
    if (!window.__epocaMediaSessions) {
        Object.defineProperty(window, '__epocaMediaSessions', {
            value: {}, writable: false, enumerable: false, configurable: false
        });
    }

    if (window.__epocaHostApi) return;
    window.__epocaHostApi = true;
    console.log('[epoca-host-api] window.host being defined');

    // Correlation ID counter for request/response matching.
    let _nextId = 1;
    const _pending = new Map();

    // Internal: send a message to the host and return a Promise.
    function _call(channel, method, params) {
        const id = _nextId++;
        return new Promise((resolve, reject) => {
            var timer = setTimeout(function() {
                if (_pending.delete(id)) reject(new Error('host timeout'));
            }, 30000);
            _pending.set(id, {
                resolve: function(v) { clearTimeout(timer); resolve(v); },
                reject:  function(e) { clearTimeout(timer); reject(e); }
            });
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

    window.host = Object.freeze({
        // Request the host to sign a payload. Returns the signature.
        // The user will see a confirmation dialog before signing proceeds.
        sign: function(payload) {
            return _call('epocaHost', 'sign', { payload: payload });
        },

        // Get the app's public address (derived from the injected identity).
        getAddress: function() {
            return _call('epocaHost', 'getAddress', {});
        },

        // Statements — publish/subscribe messaging via the host.
        statements: Object.freeze({
            write: function(channel, data) {
                return _call('epocaHost', 'statementsWrite', { channel: channel, data: data });
            },
            subscribe: function(channel) {
                return _call('epocaHost', 'statementsSubscribe', { channel: channel });
            }
        }),

        // Per-app key-value storage — persisted to ~/.epoca/apps/{app_id}/storage.json.
        storage: Object.freeze({
            get: function(key) {
                return _call('epocaHost', 'storageGet', { key: key });
            },
            set: function(key, value) {
                return _call('epocaHost', 'storageSet', { key: key, value: value });
            },
            remove: function(key) {
                return _call('epocaHost', 'storageRemove', { key: key });
            }
        }),

        // Data connections — P2P communication via the host.
        data: Object.freeze({
            getPeerId: function() {
                return _call('epocaHost', 'dataGetPeerId', {});
            },
            connect: function(peerAddress) {
                return _call('epocaHost', 'dataConnect', { peerAddress: peerAddress });
            },
            send: function(connId, data) {
                return _call('epocaHost', 'dataSend', { connId: connId, data: data });
            },
            close: function(connId) {
                return _call('epocaHost', 'dataClose', { connId: connId });
            }
        }),

        // Chain interaction — queries and extrinsics via the host's light client.
        chain: Object.freeze({
            query: function(method, params) {
                return _call('epocaHost', 'chainQuery', { method: method, params: params || {} });
            },
            submit: function(callData) {
                return _call('epocaHost', 'chainSubmit', { callData: callData });
            }
        }),

        // Media — camera/audio capture and peer-to-peer media sessions via the host.
        // getUserMedia and connect resolve immediately with opaque IDs; actual readiness
        // arrives via mediaTrackReady and mediaConnected push events.
        media: Object.freeze({
            getUserMedia: function(constraints) {
                return _call('epocaHost', 'mediaGetUserMedia', {
                    audio: !!(constraints && constraints.audio),
                    video: !!(constraints && constraints.video)
                });
            },
            connect: function(peer, trackIds) {
                return _call('epocaHost', 'mediaConnect', { peer: peer, trackIds: trackIds || [] });
            },
            accept: function() {
                return _call('epocaHost', 'mediaAccept', {});
            },
            close: function(sessionId) {
                return _call('epocaHost', 'mediaClose', { sessionId: sessionId });
            },
            attachTrack: function(trackId, elementId) {
                return _call('epocaHost', 'mediaAttachTrack', { trackId: trackId, elementId: elementId });
            },
            getPeerId: function() {
                return _call('epocaHost', 'mediaGetPeerId', {});
            },
            startListening: function(address) {
                return _call('epocaHost', 'mediaStartListening', { address: address });
            },
            setTrackEnabled: function(trackId, enabled) {
                return _call('epocaHost', 'mediaSetTrackEnabled', { trackId: trackId, enabled: enabled });
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

    // Block window.open — sandboxed apps must not open new browsing contexts.
    Object.defineProperty(window, 'open', {
        value: function() { console.warn('epoca: window.open is blocked in sandboxed apps'); return null; },
        writable: false,
        configurable: false
    });

    // Host pushes events to the app via this non-writable global.
    // Supported event names:
    //   statement, dataConnected, dataMessage, dataClosed, dataError
    //   mediaTrackReady, mediaConnected, mediaRemoteTrack, mediaClosed, mediaError
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
// epocaHost WKScriptMessageHandler — receives calls from window.host
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
                let webview_ptr = match HOST_UC_MAP.lock().unwrap().get(&uc_addr).copied() {
                    Some(p) if p != 0 => p,
                    _ => return, // UC already unregistered, drop the message
                };

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
