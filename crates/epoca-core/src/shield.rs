use gpui::*;
use epoca_shield::{ShieldConfig, ShieldManager, bootstrap};
use std::sync::{LazyLock, RwLock};

/// Newtype wrapper so `ShieldManager` can be registered as a GPUI global.
/// The orphan rule requires either the trait or the type to be local; this
/// wrapper is local to `epoca-core`.
pub struct ShieldGlobal(pub ShieldManager);
impl Global for ShieldGlobal {}

/// Initialize the shield in a background thread and update the GPUI global
/// when compilation is complete.
/// Call this from Workbench::new or the app entry point.
pub fn init_shield(cx: &mut App) {
    // Register an empty manager immediately so try_global() never panics
    // during the brief startup window before compilation finishes.
    cx.set_global(ShieldGlobal(ShieldManager::default_empty()));

    // Spawn background compilation (blocking I/O + CPU work off the main thread).
    // When done, store the compiled config in COMPILED_CONFIG so newly opened
    // tabs get the full ruleset. Already-open tabs keep their scripts from
    // startup (typically empty at t=0).
    // After the initial compile, the thread loops with a 6-hour sleep to keep
    // EasyList/EasyPrivacy fresh without requiring a restart.
    std::thread::spawn(move || {
        loop {
            log::info!("Shield: starting bootstrap (list fetch + compile)...");
            let config = bootstrap(None);
            log::info!(
                "Shield: compiled {} rule sets, {}b fingerprint script, {}b end script",
                config.rule_sets.len(),
                config.document_start_script.len(),
                config.document_end_script.len(),
            );
            COMPILED_CONFIG
                .write()
                .map(|mut guard| *guard = Some(config))
                .ok();
            // Sleep 6 hours before re-fetching the filter lists.
            std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        }
    });
}

/// Global slot for the compiled ShieldConfig, written by the background thread
/// and read by WebViewTab::new.
static COMPILED_CONFIG: LazyLock<RwLock<Option<ShieldConfig>>> =
    LazyLock::new(|| RwLock::new(None));

/// Retrieve the compiled ShieldConfig if available.
/// Returns an empty default if compilation hasn't finished yet.
pub fn current_config() -> ShieldConfig {
    COMPILED_CONFIG
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Nav channel — cmd-click / new-tab events from page JS
// ---------------------------------------------------------------------------

use std::sync::{mpsc, Mutex, OnceLock};

/// Channel for "open URL in new tab" events posted from page JS.
/// Tuple: (url, focus) where focus=true means switch to the new tab.
static NAV_CHANNEL: OnceLock<(
    mpsc::SyncSender<(String, bool)>,
    Mutex<mpsc::Receiver<(String, bool)>>,
)> = OnceLock::new();

fn nav_channel() -> &'static (
    mpsc::SyncSender<(String, bool)>,
    Mutex<mpsc::Receiver<(String, bool)>>,
) {
    NAV_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(64);
        (tx, Mutex::new(rx))
    })
}

/// Drain all pending nav events (called every render frame from Workbench).
pub fn drain_nav_events() -> Vec<(String, bool)> {
    let ch = nav_channel();
    let rx = ch.1.lock().unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// Register the `epocaNav` WKScriptMessageHandler on the given
/// WKUserContentController. The JS side posts:
///   { type: 'openInNewTab', url: '...' }           → background
///   { type: 'openInNewTabFocus', url: '...' }       → foreground
#[cfg(target_os = "macos")]
pub fn register_nav_handler(uc: *mut objc2::runtime::AnyObject) {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        // Idempotent: re-use existing class if already registered
        if let Some(c) = AnyClass::get("EpocaNavHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaNavHandler", superclass).unwrap();

            // userContentController:didReceiveScriptMessage:
            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                _uc: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                let body: *mut AnyObject = objc2::msg_send![message, body];
                if body.is_null() {
                    return;
                }
                // body is an NSDictionary from JS. Extract "type" and "url".
                let type_key: *mut AnyObject = {
                    let s: *mut AnyObject = objc2::msg_send![
                        AnyClass::get("NSString").unwrap(),
                        stringWithUTF8String: b"type\0".as_ptr() as *const i8
                    ];
                    s
                };
                let url_key: *mut AnyObject = {
                    let s: *mut AnyObject = objc2::msg_send![
                        AnyClass::get("NSString").unwrap(),
                        stringWithUTF8String: b"url\0".as_ptr() as *const i8
                    ];
                    s
                };
                let type_val: *mut AnyObject =
                    objc2::msg_send![body, objectForKey: type_key];
                let url_val: *mut AnyObject =
                    objc2::msg_send![body, objectForKey: url_key];
                if type_val.is_null() || url_val.is_null() {
                    return;
                }

                let type_cstr: *const i8 = objc2::msg_send![type_val, UTF8String];
                let url_cstr: *const i8 = objc2::msg_send![url_val, UTF8String];
                if type_cstr.is_null() || url_cstr.is_null() {
                    return;
                }

                let type_str = std::ffi::CStr::from_ptr(type_cstr).to_string_lossy();
                let url_str = std::ffi::CStr::from_ptr(url_cstr)
                    .to_string_lossy()
                    .to_string();

                let focus = type_str == "openInNewTabFocus";
                let _ = nav_channel().0.try_send((url_str, focus));
            }

            builder.add_method(
                objc2::sel!(userContentController:didReceiveScriptMessage:),
                did_receive as unsafe extern "C" fn(_, _, _, _),
            );

            // Declare WKScriptMessageHandler protocol conformance
            if let Some(proto) = objc2::runtime::AnyProtocol::get("WKScriptMessageHandler") {
                builder.add_protocol(proto);
            }

            builder.register()
        }
    });

    unsafe {
        let handler: *mut AnyObject = objc2::msg_send![*cls, new];
        if handler.is_null() {
            return;
        }
        let name: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaNav\0".as_ptr() as *const i8
        ];
        let _: () = objc2::msg_send![uc, addScriptMessageHandler: handler name: name];
    }
}

// ---------------------------------------------------------------------------
// Title channel — page title changes reported from TITLE_TRACKER_SCRIPT
// ---------------------------------------------------------------------------

/// Channel for page title events.
/// Tuple: (webview_ptr, title) where webview_ptr identifies which tab.
static TITLE_CHANNEL: OnceLock<(
    mpsc::SyncSender<(usize, String)>,
    Mutex<mpsc::Receiver<(usize, String)>>,
)> = OnceLock::new();

fn title_channel() -> &'static (
    mpsc::SyncSender<(usize, String)>,
    Mutex<mpsc::Receiver<(usize, String)>>,
) {
    TITLE_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(128);
        (tx, Mutex::new(rx))
    })
}

/// Drain all pending title events (called every render frame from Workbench).
/// Returns `(webview_ptr, title)` pairs.
pub fn drain_title_events() -> Vec<(usize, String)> {
    let ch = title_channel();
    let rx = ch.1.lock().unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// Register the `epocaMeta` WKScriptMessageHandler on the given
/// WKUserContentController. The JS side posts:
///   { type: 'titleChanged', title: '...' }
///
/// `webview_ptr` is a raw pointer cast to usize used as a stable identity key
/// to route the title to the correct tab in Workbench.
#[cfg(target_os = "macos")]
pub fn register_meta_handler(uc: *mut objc2::runtime::AnyObject, webview_ptr: usize) {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};
    use std::collections::HashMap;

    // Map from WKUserContentController pointer → webview_ptr so the static
    // callback can find the right tab identity without capturing locals.
    static UC_MAP: LazyLock<Mutex<HashMap<usize, usize>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaMetaHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaMetaHandler", superclass).unwrap();

            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                uc: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                let body: *mut AnyObject = objc2::msg_send![message, body];
                if body.is_null() { return; }

                let type_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"type\0".as_ptr() as *const i8
                ];
                let title_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"title\0".as_ptr() as *const i8
                ];
                let type_val: *mut AnyObject = objc2::msg_send![body, objectForKey: type_key];
                let title_val: *mut AnyObject = objc2::msg_send![body, objectForKey: title_key];
                if type_val.is_null() || title_val.is_null() { return; }

                let type_cstr: *const i8 = objc2::msg_send![type_val, UTF8String];
                let title_cstr: *const i8 = objc2::msg_send![title_val, UTF8String];
                if type_cstr.is_null() || title_cstr.is_null() { return; }

                let type_str = std::ffi::CStr::from_ptr(type_cstr).to_string_lossy();
                if type_str != "titleChanged" { return; }

                let title = std::ffi::CStr::from_ptr(title_cstr)
                    .to_string_lossy()
                    .to_string();
                if title.is_empty() { return; }

                // Look up which tab this UC belongs to.
                let uc_key = uc as usize;
                if let Some(wv_ptr) = UC_MAP.lock().unwrap().get(&uc_key).copied() {
                    let _ = title_channel().0.try_send((wv_ptr, title));
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

    // Record the uc → webview_ptr mapping before registering the handler.
    UC_MAP.lock().unwrap().insert(uc as usize, webview_ptr);

    unsafe {
        let handler: *mut AnyObject = objc2::msg_send![*cls, new];
        if handler.is_null() { return; }
        let name: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaMeta\0".as_ptr() as *const i8
        ];
        let _: () = objc2::msg_send![uc, addScriptMessageHandler: handler name: name];
    }
}

// ---------------------------------------------------------------------------
// Shield channel — cosmetic/blocked count events from epocaShield JS
// ---------------------------------------------------------------------------

/// Channel for shield stat events.
/// Tuple: (webview_ptr, count) where count is cosmetic elements hidden.
static SHIELD_CHANNEL: OnceLock<(
    mpsc::SyncSender<(usize, u32)>,
    Mutex<mpsc::Receiver<(usize, u32)>>,
)> = OnceLock::new();

fn shield_channel() -> &'static (
    mpsc::SyncSender<(usize, u32)>,
    Mutex<mpsc::Receiver<(usize, u32)>>,
) {
    SHIELD_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(256);
        (tx, Mutex::new(rx))
    })
}

/// Drain pending shield stat events (called every render frame from Workbench).
/// Returns `(webview_ptr, cosmetic_count)` pairs.
pub fn drain_shield_events() -> Vec<(usize, u32)> {
    let ch = shield_channel();
    let rx = ch.1.lock().unwrap();
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    events
}

/// Register the `epocaShield` WKScriptMessageHandler. Receives:
///   { type: 'cosmeticReady', count: N }
#[cfg(target_os = "macos")]
pub fn register_shield_handler(uc: *mut objc2::runtime::AnyObject, webview_ptr: usize) {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};
    use std::collections::HashMap;

    static UC_MAP: LazyLock<Mutex<HashMap<usize, usize>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaShieldHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder = ClassBuilder::new("EpocaShieldHandler", superclass).unwrap();

            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                uc: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                let body: *mut AnyObject = objc2::msg_send![message, body];
                if body.is_null() { return; }

                let type_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"type\0".as_ptr() as *const i8
                ];
                let count_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"count\0".as_ptr() as *const i8
                ];
                let type_val: *mut AnyObject = objc2::msg_send![body, objectForKey: type_key];
                let count_val: *mut AnyObject = objc2::msg_send![body, objectForKey: count_key];
                if type_val.is_null() { return; }

                let type_cstr: *const i8 = objc2::msg_send![type_val, UTF8String];
                if type_cstr.is_null() { return; }
                let type_str = std::ffi::CStr::from_ptr(type_cstr).to_string_lossy();
                if type_str != "cosmeticReady" { return; }

                let count: u32 = if count_val.is_null() {
                    0
                } else {
                    let n: i64 = objc2::msg_send![count_val, longLongValue];
                    n.max(0) as u32
                };

                let uc_key = uc as usize;
                if let Some(wv_ptr) = UC_MAP.lock().unwrap().get(&uc_key).copied() {
                    let _ = shield_channel().0.try_send((wv_ptr, count));
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

    UC_MAP.lock().unwrap().insert(uc as usize, webview_ptr);

    unsafe {
        let handler: *mut AnyObject = objc2::msg_send![*cls, new];
        if handler.is_null() { return; }
        let name: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaShield\0".as_ptr() as *const i8
        ];
        let _: () = objc2::msg_send![uc, addScriptMessageHandler: handler name: name];
    }
}
