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
    std::thread::spawn(move || {
        log::info!("Shield: starting bootstrap (list fetch + compile)...");
        let config = bootstrap(None);
        log::info!(
            "Shield: compiled {} rule sets, {}b fingerprint script, {}b end script",
            config.rule_sets.len(),
            config.document_start_script.len(),
            config.document_end_script.len(),
        );
        // Store in the static slot so WebViewTab can pull it on next open.
        // We cannot update the GPUI global from a non-GPUI thread directly;
        // the compiled config lives here and is read by current_config().
        COMPILED_CONFIG
            .write()
            .map(|mut guard| *guard = Some(config))
            .ok();
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
