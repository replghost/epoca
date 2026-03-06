use epoca_wallet::WalletManager;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};

pub struct WalletGlobal {
    pub manager: WalletManager,
}

impl gpui::Global for WalletGlobal {}

// ---------------------------------------------------------------------------
// Polkadot wallet injection script for regular WebView tabs
// ---------------------------------------------------------------------------

/// Injected at document_start into every WebView tab (when wallet is enabled).
/// Implements the standard `window.injectedWeb3['epoca']` interface that
/// Substrate dapps (polkadot.js.org, etc.) use to discover wallet extensions.
///
/// The dapp calls `injectedWeb3['epoca'].enable()` which returns accounts
/// and a signer. All signing requests route through `epocaWallet`
/// WKScriptMessageHandler → Rust host → confirmation dialog → WalletManager.
pub const WALLET_INJECT_SCRIPT: &str = r#"(function(){
    'use strict';
    if (window.__epocaWalletInjected) return;
    window.__epocaWalletInjected = true;

    // Pending promise callbacks, keyed by numeric ID.
    let _nextId = 1;
    const _pending = new Map();

    // Accounts cache — populated after enable().
    let _accounts = [];
    let _accountSubs = [];

    // Send a message to the Rust host and return a Promise.
    function _call(method, params) {
        const id = _nextId++;
        console.log('[epoca-wallet] _call', method, 'id=', id);
        return new Promise((resolve, reject) => {
            _pending.set(id, { resolve, reject });
            if (window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.epocaWallet) {
                window.webkit.messageHandlers.epocaWallet.postMessage({
                    id: id,
                    method: method,
                    params: params || {}
                });
            } else {
                _pending.delete(id);
                reject(new Error('Epoca wallet handler not available'));
            }
        });
    }

    // Host resolves/rejects pending calls via this callback.
    Object.defineProperty(window, '__epocaWalletResolve', {
        value: function(id, error, result) {
            console.log('[epoca-wallet] resolve id=', id, 'error=', error, 'result=', result);
            const p = _pending.get(id);
            if (!p) { console.warn('[epoca-wallet] no pending promise for id=', id); return; }
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

    // Host pushes account updates via this callback.
    Object.defineProperty(window, '__epocaWalletAccounts', {
        value: function(accounts) {
            _accounts = accounts;
            for (const cb of _accountSubs) {
                try { cb(accounts); } catch(e) { console.error('epoca wallet account sub error:', e); }
            }
        },
        writable: false,
        configurable: false
    });

    // Standard Polkadot wallet extension interface.
    if (!window.injectedWeb3) window.injectedWeb3 = {};

    window.injectedWeb3['epoca'] = Object.freeze({
        version: '0.1.0',

        enable: async function(appName) {
            console.log('[epoca-wallet] enable() called by', appName);
            // Request accounts from the host.
            const result = await _call('enable', { appName: appName || '' });
            console.log('[epoca-wallet] enable() got result:', JSON.stringify(result));
            _accounts = result.accounts || [];

            const ext = Object.freeze({
                accounts: Object.freeze({
                    get: async function() {
                        return _accounts.map(function(a) {
                            return { address: a.address, name: a.name || 'Epoca', type: 'sr25519' };
                        });
                    },
                    subscribe: function(cb) {
                        _accountSubs.push(cb);
                        // Immediately call with current accounts
                        try { cb(_accounts.map(function(a) {
                            return { address: a.address, name: a.name || 'Epoca', type: 'sr25519' };
                        })); } catch(e) {}
                        return function() {
                            const i = _accountSubs.indexOf(cb);
                            if (i >= 0) _accountSubs.splice(i, 1);
                        };
                    }
                }),

                signer: Object.freeze({
                    signPayload: async function(payload) {
                        // payload: { address, blockHash, blockNumber, era, genesisHash,
                        //            method, nonce, specVersion, tip, transactionVersion, signedExtensions, version }
                        const result = await _call('signPayload', { payload: payload });
                        return { id: result.id || 0, signature: result.signature };
                    },

                    signRaw: async function(raw) {
                        // raw: { address, data, type: 'bytes' }
                        const result = await _call('signRaw', { raw: raw });
                        return { id: result.id || 0, signature: result.signature };
                    }
                }),

                metadata: Object.freeze({
                    get: async function() { return []; },
                    provide: async function() { return true; }
                })
            });
            console.log('[epoca-wallet] enable() returning ext:', ext);
            console.log('[epoca-wallet] signer:', ext.signer);
            console.log('[epoca-wallet] signPayload type:', typeof ext.signer.signPayload);
            console.log('[epoca-wallet] signRaw type:', typeof ext.signer.signRaw);
            return ext;
        }
    });
})();
"#;

// ---------------------------------------------------------------------------
// epocaWallet WKScriptMessageHandler — receives calls from injectedWeb3
// ---------------------------------------------------------------------------

/// Event from a regular WebView tab's injectedWeb3 wallet calls.
pub struct WalletEvent {
    pub webview_ptr: usize,
    pub id: u64,
    pub method: String,
    pub params_json: String,
}

static WALLET_CHANNEL: OnceLock<(
    mpsc::SyncSender<WalletEvent>,
    Mutex<mpsc::Receiver<WalletEvent>>,
)> = OnceLock::new();

fn wallet_channel() -> &'static (
    mpsc::SyncSender<WalletEvent>,
    Mutex<mpsc::Receiver<WalletEvent>>,
) {
    WALLET_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(64);
        (tx, Mutex::new(rx))
    })
}

/// Drain pending wallet events from regular WebView tabs.
pub fn drain_wallet_events() -> Vec<WalletEvent> {
    let ch = wallet_channel();
    let rx = ch.1.lock().unwrap();
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

/// UC → webview_ptr mapping for the wallet handler.
static WALLET_UC_MAP: std::sync::LazyLock<Mutex<HashMap<usize, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register the `epocaWallet` WKScriptMessageHandler on a WebViewTab's
/// WKUserContentController.
#[cfg(target_os = "macos")]
pub fn register_wallet_handler(uc: *mut objc2::runtime::AnyObject, webview_ptr: usize) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaWalletHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaWalletHandler", superclass).unwrap();

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
                        let alloc: *mut AnyObject =
                            msg_send![AnyClass::get("NSString").unwrap(), alloc];
                        let ns_str: *mut AnyObject =
                            msg_send![alloc, initWithData: json_data encoding: 4u64];
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
                let webview_ptr =
                    WALLET_UC_MAP.lock().unwrap().get(&uc_addr).copied().unwrap_or(0);

                let _ = wallet_channel().0.try_send(WalletEvent {
                    webview_ptr,
                    id: id_num,
                    method,
                    params_json,
                });
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

    WALLET_UC_MAP
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
            stringWithUTF8String: b"epocaWallet\0".as_ptr() as *const i8
        ];
        let _: () = msg_send![uc, addScriptMessageHandler: handler name: name];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn register_wallet_handler(_uc: *mut std::ffi::c_void, _webview_ptr: usize) {}
