//! WebAuthn / FIDO2 support for WKWebView.
//!
//! WKWebView in third-party browsers does NOT automatically handle
//! `navigator.credentials.create()` / `.get()`. We intercept these calls
//! via a JS polyfill and relay the ceremony to macOS AuthenticationServices
//! (`ASAuthorizationController`) using raw objc2 `msg_send!`.
//!
//! Supports:
//! - USB hardware security keys (YubiKey, SoloKeys) — macOS 14.0+
//! - Platform passkeys (iCloud Keychain, Touch ID) — macOS 13.0+
//!
//! Architecture: JS polyfill → epocaWebAuthn WKScriptMessageHandler →
//! ASAuthorizationController ceremony → result channel → evaluate_script
//! to resolve the original JS promise.

use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};

// ── JS Polyfill ──────────────────────────────────────────────────────

/// Injected at document_start. Overrides `navigator.credentials.create()`
/// and `.get()` to relay WebAuthn ceremonies to the native layer via
/// `window.webkit.messageHandlers.epocaWebAuthn.postMessage(...)`.
///
/// The native layer resolves/rejects by calling
/// `window.__epocaWebAuthnResolve(callbackId, ok, responseJson, errorMsg)`.
pub const WEBAUTHN_POLYFILL: &str = r#"(function(){
if(window.__epocaWebAuthn)return;
window.__epocaWebAuthn=true;
var _cbs=new Map(),_id=0;

function b64(buf){
  var u=new Uint8Array(buf),s='';
  for(var i=0;i<u.length;i+=8192)s+=String.fromCharCode.apply(null,u.subarray(i,i+8192));
  return btoa(s).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
}
function unb64(s){
  s=s.replace(/-/g,'+').replace(/_/g,'/');
  while(s.length%4)s+='=';
  var r=atob(s),a=new Uint8Array(r.length);
  for(var i=0;i<r.length;i++)a[i]=r.charCodeAt(i);
  return a.buffer;
}

// Ensure PublicKeyCredential exists and advertises capability.
// Google, GitHub, etc. check these static methods before offering WebAuthn options.
if(!window.PublicKeyCredential){
  window.PublicKeyCredential=function PublicKeyCredential(){};
  window.PublicKeyCredential.prototype.type='public-key';
  window.PublicKeyCredential.prototype.getClientExtensionResults=function(){return {};};
}
// "Is a platform authenticator (Touch ID / passkey) available?"
window.PublicKeyCredential.isUserVerifyingPlatformAuthenticatorAvailable=function(){
  return Promise.resolve(true);
};
// "Is conditional mediation (autofill passkey) available?"
window.PublicKeyCredential.isConditionalMediationAvailable=function(){
  return Promise.resolve(true);
};
// "Is the external authenticator (security key) available?" — macOS 14+
window.PublicKeyCredential.isExternalCTAP2SecurityKeySupported=function(){
  return Promise.resolve(true);
};

window.__epocaWebAuthnResolve=function(cbId,ok,json,err){
  var p=_cbs.get(cbId);
  if(!p)return;
  _cbs.delete(cbId);
  if(!ok){p.reject(new DOMException(err||'WebAuthn failed','NotAllowedError'));return;}
  try{
    var r=JSON.parse(json);
    // Reconstruct ArrayBuffer fields from base64url
    if(r.rawId)r.rawId=unb64(r.rawId);
    if(r.response){
      if(r.response.clientDataJSON)r.response.clientDataJSON=unb64(r.response.clientDataJSON);
      if(r.response.attestationObject)r.response.attestationObject=unb64(r.response.attestationObject);
      if(r.response.authenticatorData)r.response.authenticatorData=unb64(r.response.authenticatorData);
      if(r.response.signature)r.response.signature=unb64(r.response.signature);
      if(r.response.userHandle)r.response.userHandle=unb64(r.response.userHandle);
    }
    r.getClientExtensionResults=function(){return {};};
    r.type='public-key';
    p.resolve(r);
  }catch(e){p.reject(new DOMException('Failed to parse response','OperationError'));}
};

// Ensure navigator.credentials exists
if(!navigator.credentials){
  navigator.credentials={};
}
var origCreate=navigator.credentials.create?navigator.credentials.create.bind(navigator.credentials):null;
var origGet=navigator.credentials.get?navigator.credentials.get.bind(navigator.credentials):null;

navigator.credentials.create=function(opts){
  if(!opts||!opts.publicKey)return origCreate?origCreate(opts):Promise.reject(new DOMException('Not supported','NotSupportedError'));
  if(!window.webkit||!window.webkit.messageHandlers||!window.webkit.messageHandlers.epocaWebAuthn)
    return origCreate?origCreate(opts):Promise.reject(new DOMException('Not supported','NotSupportedError'));
  return new Promise(function(resolve,reject){
    var cbId=String(++_id);
    _cbs.set(cbId,{resolve:resolve,reject:reject});
    var pk=opts.publicKey;
    var sel=pk.authenticatorSelection||{};
    window.webkit.messageHandlers.epocaWebAuthn.postMessage({
      type:'create',
      rpId:pk.rp.id||window.location.hostname,
      rpName:pk.rp.name||'',
      userName:pk.user.name,
      userDisplayName:pk.user.displayName||pk.user.name,
      userId:b64(pk.user.id),
      challenge:b64(pk.challenge),
      pubKeyCredParams:(pk.pubKeyCredParams||[]).map(function(p){return p.alg;}),
      authenticatorAttachment:sel.authenticatorAttachment||'',
      residentKey:sel.residentKey||'',
      userVerification:sel.userVerification||'preferred',
      excludeCredentials:(pk.excludeCredentials||[]).map(function(c){return b64(c.id);}),
      origin:window.location.origin,
      callbackId:cbId,
      timeout:pk.timeout||60000
    });
    setTimeout(function(){
      if(_cbs.has(cbId)){_cbs.delete(cbId);reject(new DOMException('Timeout','NotAllowedError'));}
    },pk.timeout||60000);
  });
};

navigator.credentials.get=function(opts){
  if(!opts||!opts.publicKey)return origGet?origGet(opts):Promise.reject(new DOMException('Not supported','NotSupportedError'));
  if(!window.webkit||!window.webkit.messageHandlers||!window.webkit.messageHandlers.epocaWebAuthn)
    return origGet?origGet(opts):Promise.reject(new DOMException('Not supported','NotSupportedError'));
  return new Promise(function(resolve,reject){
    var cbId=String(++_id);
    _cbs.set(cbId,{resolve:resolve,reject:reject});
    var pk=opts.publicKey;
    window.webkit.messageHandlers.epocaWebAuthn.postMessage({
      type:'get',
      rpId:pk.rpId||window.location.hostname,
      challenge:b64(pk.challenge),
      allowCredentials:(pk.allowCredentials||[]).map(function(c){return b64(c.id);}),
      userVerification:pk.userVerification||'preferred',
      origin:window.location.origin,
      callbackId:cbId,
      timeout:pk.timeout||60000
    });
    setTimeout(function(){
      if(_cbs.has(cbId)){_cbs.delete(cbId);reject(new DOMException('Timeout','NotAllowedError'));}
    },pk.timeout||60000);
  });
};
})();"#;

// ── Response Channel ─────────────────────────────────────────────────

/// A completed WebAuthn ceremony result to inject back into the page.
#[derive(Debug)]
pub struct WebAuthnResponse {
    pub webview_ptr: usize,
    pub callback_id: String,
    pub ok: bool,
    /// JSON string with the credential response fields (base64url-encoded).
    pub response_json: Option<String>,
    /// Error message if !ok.
    pub error: Option<String>,
}

static WEBAUTHN_CHANNEL: OnceLock<(
    mpsc::SyncSender<WebAuthnResponse>,
    Mutex<mpsc::Receiver<WebAuthnResponse>>,
)> = OnceLock::new();

fn channel() -> &'static (
    mpsc::SyncSender<WebAuthnResponse>,
    Mutex<mpsc::Receiver<WebAuthnResponse>>,
) {
    WEBAUTHN_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(16);
        (tx, Mutex::new(rx))
    })
}

/// Drain all completed WebAuthn responses. Called each frame from
/// `Workbench::process_pending_nav()`.
pub fn drain_webauthn_responses() -> Vec<WebAuthnResponse> {
    let ch = channel();
    let rx = ch.1.lock().unwrap();
    let mut out = Vec::new();
    while let Ok(r) = rx.try_recv() {
        out.push(r);
    }
    out
}

// ── Handler Registration (macOS) ─────────────────────────────────────

/// Map from WKUserContentController pointer → webview_ptr.
/// Used inside the ObjC callback to route responses to the correct tab.
#[cfg(target_os = "macos")]
static UC_MAP: std::sync::LazyLock<Mutex<HashMap<usize, usize>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register the `epocaWebAuthn` WKScriptMessageHandler on the given
/// WKUserContentController. Must be called after obtaining `uc` from
/// `[webview configuration].userContentController`.
#[cfg(target_os = "macos")]
pub fn register_webauthn_handler(
    uc: *mut objc2::runtime::AnyObject,
    webview_ptr: usize,
) {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static objc2::runtime::AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaWebAuthnHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder =
                ClassBuilder::new("EpocaWebAuthnHandler", superclass).unwrap();

            unsafe extern "C" fn did_receive(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                uc_obj: *mut AnyObject,
                message: *mut AnyObject,
            ) {
                // Wrap in catch_unwind: panics in extern "C" abort the process.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle_webauthn_message(uc_obj, message);
                }));
            }

            builder.add_method(
                objc2::sel!(userContentController:didReceiveScriptMessage:),
                did_receive as unsafe extern "C" fn(_, _, _, _),
            );

            if let Some(proto) =
                objc2::runtime::AnyProtocol::get("WKScriptMessageHandler")
            {
                builder.add_protocol(proto);
            }

            builder.register()
        }
    });

    UC_MAP.lock().unwrap().insert(uc as usize, webview_ptr);

    unsafe {
        let handler: *mut objc2::runtime::AnyObject = objc2::msg_send![*cls, new];
        if handler.is_null() {
            return;
        }
        let name: *mut objc2::runtime::AnyObject = objc2::msg_send![
            objc2::runtime::AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaWebAuthn\0".as_ptr() as *const i8
        ];
        let _: () =
            objc2::msg_send![uc, addScriptMessageHandler: handler name: name];
        // Balance the +1 from `new` — UC retains handler via addScriptMessageHandler
        let _: () = objc2::msg_send![handler, release];
    }

    log::info!("WebAuthn: registered epocaWebAuthn handler (wv={webview_ptr:#x})");
}

#[cfg(not(target_os = "macos"))]
pub fn register_webauthn_handler(
    _uc: *mut objc2::runtime::AnyObject,
    _webview_ptr: usize,
) {
}

// ── ObjC Message Handling ────────────────────────────────────────────

/// Extract a string value from an NSDictionary for the given key.
#[cfg(target_os = "macos")]
unsafe fn dict_get_string(
    dict: *mut objc2::runtime::AnyObject,
    key: &[u8], // null-terminated
) -> Option<String> {
    use objc2::runtime::{AnyClass, AnyObject};
    let ns_key: *mut AnyObject = objc2::msg_send![
        AnyClass::get("NSString").unwrap(),
        stringWithUTF8String: key.as_ptr() as *const i8
    ];
    let val: *mut AnyObject = objc2::msg_send![dict, objectForKey: ns_key];
    if val.is_null() {
        return None;
    }
    let cstr: *const i8 = objc2::msg_send![val, UTF8String];
    if cstr.is_null() {
        return None;
    }
    Some(std::ffi::CStr::from_ptr(cstr).to_string_lossy().into_owned())
}

/// Extract an NSArray of NSString from a dict key → Vec<String>.
#[cfg(target_os = "macos")]
unsafe fn dict_get_string_array(
    dict: *mut objc2::runtime::AnyObject,
    key: &[u8],
) -> Vec<String> {
    use objc2::runtime::{AnyClass, AnyObject};
    let ns_key: *mut AnyObject = objc2::msg_send![
        AnyClass::get("NSString").unwrap(),
        stringWithUTF8String: key.as_ptr() as *const i8
    ];
    let arr: *mut AnyObject = objc2::msg_send![dict, objectForKey: ns_key];
    if arr.is_null() {
        return Vec::new();
    }
    let count: usize = objc2::msg_send![arr, count];
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let item: *mut AnyObject = objc2::msg_send![arr, objectAtIndex: i];
        if item.is_null() {
            continue;
        }
        let cstr: *const i8 = objc2::msg_send![item, UTF8String];
        if !cstr.is_null() {
            out.push(
                std::ffi::CStr::from_ptr(cstr)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    out
}

/// Extract an NSArray of NSNumber → Vec<i64>.
#[cfg(target_os = "macos")]
unsafe fn dict_get_number_array(
    dict: *mut objc2::runtime::AnyObject,
    key: &[u8],
) -> Vec<i64> {
    use objc2::runtime::{AnyClass, AnyObject};
    let ns_key: *mut AnyObject = objc2::msg_send![
        AnyClass::get("NSString").unwrap(),
        stringWithUTF8String: key.as_ptr() as *const i8
    ];
    let arr: *mut AnyObject = objc2::msg_send![dict, objectForKey: ns_key];
    if arr.is_null() {
        return Vec::new();
    }
    let count: usize = objc2::msg_send![arr, count];
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let item: *mut AnyObject = objc2::msg_send![arr, objectAtIndex: i];
        if !item.is_null() {
            let v: i64 = objc2::msg_send![item, longLongValue];
            out.push(v);
        }
    }
    out
}

/// Called from the ObjC handler when a JS message arrives.
#[cfg(target_os = "macos")]
unsafe fn handle_webauthn_message(
    uc_obj: *mut objc2::runtime::AnyObject,
    message: *mut objc2::runtime::AnyObject,
) {
    use objc2::runtime::AnyObject;

    let body: *mut AnyObject = objc2::msg_send![message, body];
    if body.is_null() {
        return;
    }

    // Guard: body must be NSDictionary — calling objectForKey: on a
    // non-dict (e.g. NSString) throws an ObjC exception that Rust
    // cannot catch (foreign exception → abort).
    let is_dict: bool = objc2::msg_send![
        body,
        isKindOfClass: objc2::runtime::AnyClass::get("NSDictionary").unwrap()
    ];
    if !is_dict {
        log::warn!("WebAuthn: message body is not NSDictionary, ignoring");
        return;
    }

    let Some(msg_type) = dict_get_string(body, b"type\0") else {
        return;
    };
    let Some(callback_id) = dict_get_string(body, b"callbackId\0") else {
        return;
    };
    let origin = dict_get_string(body, b"origin\0").unwrap_or_default();

    let webview_ptr = UC_MAP
        .lock()
        .unwrap()
        .get(&(uc_obj as usize))
        .copied()
        .unwrap_or(0);

    if webview_ptr == 0 {
        log::warn!("WebAuthn: no webview_ptr for UC {uc_obj:p}");
        return;
    }

    log::info!("WebAuthn: {msg_type} ceremony from {origin} (cb={callback_id})");

    // Check macOS version — security keys need 14.0+
    let os_version = macos_major_version();
    if os_version < 13 {
        send_error(webview_ptr, &callback_id, "WebAuthn requires macOS 13+");
        return;
    }

    match msg_type.as_str() {
        "create" => {
            let rp_id = dict_get_string(body, b"rpId\0").unwrap_or_default();
            let rp_name = dict_get_string(body, b"rpName\0").unwrap_or_default();
            let user_name = dict_get_string(body, b"userName\0").unwrap_or_default();
            let user_display_name =
                dict_get_string(body, b"userDisplayName\0").unwrap_or_default();
            let user_id = dict_get_string(body, b"userId\0").unwrap_or_default();
            let challenge = dict_get_string(body, b"challenge\0").unwrap_or_default();
            let algs = dict_get_number_array(body, b"pubKeyCredParams\0");
            let authenticator_attachment =
                dict_get_string(body, b"authenticatorAttachment\0").unwrap_or_default();
            let user_verification =
                dict_get_string(body, b"userVerification\0").unwrap_or_default();

            run_create_ceremony(
                webview_ptr,
                callback_id,
                rp_id,
                rp_name,
                user_name,
                user_display_name,
                user_id,
                challenge,
                algs,
                authenticator_attachment,
                user_verification,
            );
        }
        "get" => {
            let rp_id = dict_get_string(body, b"rpId\0").unwrap_or_default();
            let challenge = dict_get_string(body, b"challenge\0").unwrap_or_default();
            let allow_credentials =
                dict_get_string_array(body, b"allowCredentials\0");
            let user_verification =
                dict_get_string(body, b"userVerification\0").unwrap_or_default();

            run_get_ceremony(
                webview_ptr,
                callback_id,
                rp_id,
                challenge,
                allow_credentials,
                user_verification,
            );
        }
        _ => {
            log::warn!("WebAuthn: unknown type: {msg_type}");
        }
    }
}

// ── macOS Version Check ──────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_major_version() -> u32 {
    extern "C" {
        fn sysctlbyname(
            name: *const i8,
            oldp: *mut std::ffi::c_void,
            oldlenp: *mut usize,
            newp: *mut std::ffi::c_void,
            newlen: usize,
        ) -> i32;
    }
    let mut buf = [0u8; 32];
    let mut len = buf.len();
    let ret = unsafe {
        sysctlbyname(
            b"kern.osproductversion\0".as_ptr() as *const i8,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 && len > 0 {
        // buf contains e.g. "15.3.1\0"
        let s = String::from_utf8_lossy(&buf[..len]);
        if let Some(major) = s.trim_end_matches('\0').split('.').next() {
            if let Ok(v) = major.parse::<u32>() {
                return v;
            }
        }
    }
    12 // conservative fallback
}

// ── ASAuthorizationController Ceremonies ──────────────────────────────

/// Send an error response back through the channel.
#[cfg(target_os = "macos")]
fn send_error(webview_ptr: usize, callback_id: &str, msg: &str) {
    let _ = channel().0.try_send(WebAuthnResponse {
        webview_ptr,
        callback_id: callback_id.to_string(),
        ok: false,
        response_json: None,
        error: Some(msg.to_string()),
    });
}

/// Send a success response.
#[cfg(target_os = "macos")]
fn send_success(webview_ptr: usize, callback_id: &str, json: String) {
    let _ = channel().0.try_send(WebAuthnResponse {
        webview_ptr,
        callback_id: callback_id.to_string(),
        ok: true,
        response_json: Some(json),
        error: None,
    });
}

// ── ASAuthorizationController Delegate ────────────────────────────────

/// Thread-local storage for the pending ceremony context, so the delegate
/// callback can find the webview_ptr and callback_id.
#[cfg(target_os = "macos")]
static PENDING_CEREMONY: Mutex<Option<(usize, String)>> = Mutex::new(None);

/// Register the ASAuthorizationControllerDelegate class (once).
#[cfg(target_os = "macos")]
fn delegate_class() -> &'static objc2::runtime::AnyClass {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaWebAuthnDelegate") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder =
                ClassBuilder::new("EpocaWebAuthnDelegate", superclass).unwrap();

            // authorizationController:didCompleteWithAuthorization:
            unsafe extern "C" fn did_complete(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                _controller: *mut AnyObject,
                authorization: *mut AnyObject,
            ) {
                handle_authorization_success(authorization);
            }

            // authorizationController:didCompleteWithError:
            unsafe extern "C" fn did_fail(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                _controller: *mut AnyObject,
                error: *mut AnyObject,
            ) {
                handle_authorization_error(error);
            }

            // presentationAnchorForAuthorizationController:
            unsafe extern "C" fn presentation_anchor(
                _this: *mut AnyObject,
                _sel: objc2::runtime::Sel,
                _controller: *mut AnyObject,
            ) -> *mut AnyObject {
                // Return the key window as the presentation anchor.
                let app: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSApplication").unwrap(),
                    sharedApplication
                ];
                let window: *mut AnyObject = objc2::msg_send![app, keyWindow];
                window
            }

            builder.add_method(
                objc2::sel!(authorizationController:didCompleteWithAuthorization:),
                did_complete as unsafe extern "C" fn(_, _, _, _),
            );
            builder.add_method(
                objc2::sel!(authorizationController:didCompleteWithError:),
                did_fail as unsafe extern "C" fn(_, _, _, _),
            );
            builder.add_method(
                objc2::sel!(presentationAnchorForAuthorizationController:),
                presentation_anchor as unsafe extern "C" fn(_, _, _) -> *mut AnyObject,
            );

            // Conform to both delegate and presentation provider protocols
            if let Some(p) =
                objc2::runtime::AnyProtocol::get("ASAuthorizationControllerDelegate")
            {
                builder.add_protocol(p);
            }
            if let Some(p) = objc2::runtime::AnyProtocol::get(
                "ASAuthorizationControllerPresentationContextProviding",
            ) {
                builder.add_protocol(p);
            }

            builder.register()
        }
    })
}

/// Handle a successful ASAuthorization. Extract the credential and send
/// a JSON response back through the channel.
#[cfg(target_os = "macos")]
unsafe fn handle_authorization_success(
    authorization: *mut objc2::runtime::AnyObject,
) {
    use objc2::runtime::AnyObject;

    let pending = PENDING_CEREMONY.lock().unwrap().take();
    let Some((webview_ptr, callback_id)) = pending else {
        log::warn!("WebAuthn: success callback but no pending ceremony");
        return;
    };

    // Get the credential object
    let credential: *mut AnyObject = objc2::msg_send![authorization, credential];
    if credential.is_null() {
        send_error(webview_ptr, &callback_id, "No credential in response");
        return;
    }

    // Check if it's a platform or security key registration credential
    let is_registration: bool =
        objc2::msg_send![credential, respondsToSelector: objc2::sel!(attestationObject)];
    let is_assertion: bool =
        objc2::msg_send![credential, respondsToSelector: objc2::sel!(signature)];

    if is_registration {
        let json = extract_registration_credential(credential);
        match json {
            Some(j) => send_success(webview_ptr, &callback_id, j),
            None => send_error(webview_ptr, &callback_id, "Failed to extract registration"),
        }
    } else if is_assertion {
        let json = extract_assertion_credential(credential);
        match json {
            Some(j) => send_success(webview_ptr, &callback_id, j),
            None => send_error(webview_ptr, &callback_id, "Failed to extract assertion"),
        }
    } else {
        send_error(webview_ptr, &callback_id, "Unknown credential type");
    }
}

/// Handle an ASAuthorization error.
#[cfg(target_os = "macos")]
unsafe fn handle_authorization_error(error: *mut objc2::runtime::AnyObject) {
    let pending = PENDING_CEREMONY.lock().unwrap().take();
    let Some((webview_ptr, callback_id)) = pending else {
        log::warn!("WebAuthn: error callback but no pending ceremony");
        return;
    };

    let desc: *mut objc2::runtime::AnyObject =
        objc2::msg_send![error, localizedDescription];
    let msg = if !desc.is_null() {
        let cstr: *const i8 = objc2::msg_send![desc, UTF8String];
        if !cstr.is_null() {
            std::ffi::CStr::from_ptr(cstr)
                .to_string_lossy()
                .into_owned()
        } else {
            "Unknown error".to_string()
        }
    } else {
        "Unknown error".to_string()
    };

    log::info!("WebAuthn: ceremony failed: {msg}");
    send_error(webview_ptr, &callback_id, &msg);
}

// ── Credential Extraction ────────────────────────────────────────────

/// Base64url-encode NSData.
#[cfg(target_os = "macos")]
unsafe fn nsdata_to_b64url(data: *mut objc2::runtime::AnyObject) -> Option<String> {
    if data.is_null() {
        return None;
    }
    let bytes: *const u8 = objc2::msg_send![data, bytes];
    let length: usize = objc2::msg_send![data, length];
    if bytes.is_null() || length == 0 {
        return Some(String::new());
    }
    let slice = std::slice::from_raw_parts(bytes, length);
    Some(base64url_encode(slice))
}

fn base64url_encode(data: &[u8]) -> String {
    use alloc::string::String;
    const CHARS: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

extern crate alloc;

/// Extract registration credential fields into JSON.
#[cfg(target_os = "macos")]
unsafe fn extract_registration_credential(
    credential: *mut objc2::runtime::AnyObject,
) -> Option<String> {
    // credentialID → rawId
    let cred_id: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, credentialID];
    let raw_id = nsdata_to_b64url(cred_id)?;

    // rawAttestationObject → attestationObject
    let attestation_obj: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, rawAttestationObject];
    let attestation = nsdata_to_b64url(attestation_obj).unwrap_or_default();

    // rawClientDataJSON
    let client_data: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, rawClientDataJSON];
    let client_data_json = nsdata_to_b64url(client_data).unwrap_or_default();

    // Determine authenticatorAttachment from credential class
    let is_platform: bool = objc2::msg_send![
        credential,
        isKindOfClass: objc2::runtime::AnyClass::get(
            "ASAuthorizationPlatformPublicKeyCredentialRegistration"
        ).unwrap_or(objc2::runtime::AnyClass::get("NSObject").unwrap())
    ];
    let attachment = if is_platform { "platform" } else { "cross-platform" };

    // Build JSON response matching PublicKeyCredential shape
    let json = alloc::format!(
        r#"{{"id":"{}","rawId":"{}","response":{{"clientDataJSON":"{}","attestationObject":"{}"}},"authenticatorAttachment":"{}"}}"#,
        raw_id, raw_id, client_data_json, attestation, attachment
    );

    Some(json)
}

/// Extract assertion credential fields into JSON.
#[cfg(target_os = "macos")]
unsafe fn extract_assertion_credential(
    credential: *mut objc2::runtime::AnyObject,
) -> Option<String> {
    // credentialID → rawId
    let cred_id: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, credentialID];
    let raw_id = nsdata_to_b64url(cred_id)?;

    // rawAuthenticatorData
    let auth_data: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, rawAuthenticatorData];
    let authenticator_data = nsdata_to_b64url(auth_data).unwrap_or_default();

    // signature
    let sig: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, signature];
    let signature = nsdata_to_b64url(sig).unwrap_or_default();

    // rawClientDataJSON
    let client_data: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, rawClientDataJSON];
    let client_data_json = nsdata_to_b64url(client_data).unwrap_or_default();

    // userID (may be nil)
    let user_id_data: *mut objc2::runtime::AnyObject =
        objc2::msg_send![credential, userID];
    let user_handle = nsdata_to_b64url(user_id_data).unwrap_or_default();

    // Determine authenticatorAttachment from credential class
    let is_platform: bool = objc2::msg_send![
        credential,
        isKindOfClass: objc2::runtime::AnyClass::get(
            "ASAuthorizationPlatformPublicKeyCredentialAssertion"
        ).unwrap_or(objc2::runtime::AnyClass::get("NSObject").unwrap())
    ];
    let attachment = if is_platform { "platform" } else { "cross-platform" };

    let json = alloc::format!(
        r#"{{"id":"{}","rawId":"{}","response":{{"clientDataJSON":"{}","authenticatorData":"{}","signature":"{}","userHandle":"{}"}},"authenticatorAttachment":"{}"}}"#,
        raw_id, raw_id, client_data_json, authenticator_data, signature, user_handle, attachment
    );

    Some(json)
}

// ── Ceremony Execution ───────────────────────────────────────────────

/// Base64url decode to NSData.
#[cfg(target_os = "macos")]
unsafe fn b64url_to_nsdata(
    s: &str,
) -> *mut objc2::runtime::AnyObject {
    use objc2::runtime::{AnyClass, AnyObject};
    let bytes = base64url_decode(s);
    let data: *mut AnyObject = objc2::msg_send![
        AnyClass::get("NSData").unwrap(),
        dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void
        length: bytes.len()
    ];
    data
}

fn base64url_decode(input: &str) -> Vec<u8> {
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    // Simple base64 decoder
    const DECODE: [u8; 128] = {
        let mut t = [255u8; 128];
        let mut i = 0u8;
        while i < 26 { t[(b'A' + i) as usize] = i; i += 1; }
        i = 0;
        while i < 26 { t[(b'a' + i) as usize] = 26 + i; i += 1; }
        i = 0;
        while i < 10 { t[(b'0' + i) as usize] = 52 + i; i += 1; }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t[b'=' as usize] = 0;
        t
    };

    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        // Bounds-check: reject non-ASCII bytes
        if bytes[i] > 127 || bytes[i+1] > 127 || bytes[i+2] > 127 || bytes[i+3] > 127 {
            break;
        }
        let a = DECODE[bytes[i] as usize] as u32;
        let b = DECODE[bytes[i + 1] as usize] as u32;
        let c = DECODE[bytes[i + 2] as usize] as u32;
        let d = DECODE[bytes[i + 3] as usize] as u32;
        if a == 255 || b == 255 { break; }
        let triple = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((triple >> 16) as u8);
        if bytes[i + 2] != b'=' {
            out.push((triple >> 8) as u8);
        }
        if bytes[i + 3] != b'=' {
            out.push(triple as u8);
        }
        i += 4;
    }
    out
}

/// Run a `navigator.credentials.create()` ceremony via ASAuthorizationController.
#[cfg(target_os = "macos")]
fn run_create_ceremony(
    webview_ptr: usize,
    callback_id: String,
    rp_id: String,
    _rp_name: String,
    user_name: String,
    user_display_name: String,
    user_id: String,
    challenge: String,
    algs: Vec<i64>,
    authenticator_attachment: String,
    user_verification: String,
) {
    use objc2::runtime::{AnyClass, AnyObject};

    // Reject concurrent ceremonies
    {
        let mut pending = PENDING_CEREMONY.lock().unwrap();
        if pending.is_some() {
            drop(pending);
            send_error(webview_ptr, &callback_id, "Another WebAuthn ceremony is in progress");
            return;
        }
        *pending = Some((webview_ptr, callback_id.clone()));
    }

    unsafe {
        let challenge_data = b64url_to_nsdata(&challenge);
        let user_id_data = b64url_to_nsdata(&user_id);

        // Build NSStrings — bind CString to locals to prevent use-after-free
        let rp_id_cstr = std::ffi::CString::new(rp_id.as_str()).unwrap_or_default();
        let rp_id_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: rp_id_cstr.as_ptr()
        ];
        let user_display_cstr =
            std::ffi::CString::new(user_display_name.as_str()).unwrap_or_default();
        let user_display_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: user_display_cstr.as_ptr()
        ];
        let user_name_cstr =
            std::ffi::CString::new(user_name.as_str()).unwrap_or_default();
        let user_name_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: user_name_cstr.as_ptr()
        ];

        // Map userVerification to AS preference NSString
        let uv_str: &[u8] = match user_verification.as_str() {
            "required" => b"required\0",
            "discouraged" => b"discouraged\0",
            _ => b"preferred\0",
        };
        let uv_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: uv_str.as_ptr() as *const i8
        ];

        // Create an NSMutableArray of ASAuthorizationRequest objects
        let requests: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSMutableArray").unwrap(),
            new
        ];

        let want_platform = authenticator_attachment.is_empty()
            || authenticator_attachment == "platform";
        let want_cross_platform = authenticator_attachment.is_empty()
            || authenticator_attachment == "cross-platform";

        // Platform passkey provider (Touch ID / iCloud Keychain)
        if want_platform {
            if let Some(cls) = AnyClass::get(
                "ASAuthorizationPlatformPublicKeyCredentialProvider",
            ) {
                let provider: *mut AnyObject =
                    objc2::msg_send![cls, alloc];
                let provider: *mut AnyObject =
                    objc2::msg_send![provider, initWithRelyingPartyIdentifier: rp_id_ns];
                if !provider.is_null() {
                    let request: *mut AnyObject = objc2::msg_send![
                        provider,
                        createCredentialRegistrationRequestWithChallenge: challenge_data
                        name: user_display_ns
                        userID: user_id_data
                    ];
                    if !request.is_null() {
                        let _: () = objc2::msg_send![request, setUserVerificationPreference: uv_ns];
                        let _: () = objc2::msg_send![requests, addObject: request];
                    }
                }
            }
        }

        // Security key provider (USB FIDO2)
        if want_cross_platform {
            if let Some(cls) = AnyClass::get(
                "ASAuthorizationSecurityKeyPublicKeyCredentialProvider",
            ) {
                let provider: *mut AnyObject =
                    objc2::msg_send![cls, alloc];
                let provider: *mut AnyObject =
                    objc2::msg_send![provider, initWithRelyingPartyIdentifier: rp_id_ns];
                if !provider.is_null() {
                    let request: *mut AnyObject = objc2::msg_send![
                        provider,
                        createCredentialRegistrationRequestWithChallenge: challenge_data
                        displayName: user_display_ns
                        name: user_name_ns
                        userID: user_id_data
                    ];
                    if !request.is_null() {
                        // Set credential parameters (algorithms)
                        if !algs.is_empty() {
                            let params_arr: *mut AnyObject = objc2::msg_send![
                                AnyClass::get("NSMutableArray").unwrap(),
                                new
                            ];
                            for alg in &algs {
                                if let Some(param_cls) = AnyClass::get(
                                    "ASAuthorizationPublicKeyCredentialParameters",
                                ) {
                                    // COSEAlgorithmIdentifier is NSInteger
                                    let param: *mut AnyObject = objc2::msg_send![
                                        param_cls, alloc
                                    ];
                                    let param: *mut AnyObject = objc2::msg_send![
                                        param,
                                        initWithAlgorithm: *alg as isize
                                    ];
                                    if !param.is_null() {
                                        let _: () = objc2::msg_send![
                                            params_arr, addObject: param
                                        ];
                                    }
                                }
                            }
                            let _: () = objc2::msg_send![
                                request,
                                setCredentialParameters: params_arr
                            ];
                        }
                        let _: () = objc2::msg_send![request, setUserVerificationPreference: uv_ns];
                        let _: () = objc2::msg_send![requests, addObject: request];
                    }
                }
            }
        }

        // Check we have at least one request
        let count: usize = objc2::msg_send![requests, count];
        if count == 0 {
            *PENDING_CEREMONY.lock().unwrap() = None;
            send_error(
                webview_ptr,
                &callback_id,
                "No authentication providers available on this macOS version",
            );
            return;
        }

        // Create ASAuthorizationController with the requests
        if let Some(ctrl_cls) = AnyClass::get("ASAuthorizationController") {
            let controller: *mut AnyObject = objc2::msg_send![ctrl_cls, alloc];
            let controller: *mut AnyObject =
                objc2::msg_send![controller, initWithAuthorizationRequests: requests];
            if controller.is_null() {
                *PENDING_CEREMONY.lock().unwrap() = None;
                send_error(webview_ptr, &callback_id, "Failed to create controller");
                return;
            }

            // Set delegate and presentation provider
            let delegate: *mut AnyObject =
                objc2::msg_send![delegate_class(), new];
            let _: () = objc2::msg_send![controller, setDelegate: delegate];
            let _: () =
                objc2::msg_send![controller, setPresentationContextProvider: delegate];
            // Balance +1 from `new` — controller retains delegate via setDelegate/setPresentationContextProvider
            let _: () = objc2::msg_send![delegate, release];

            // Perform the ceremony
            let _: () = objc2::msg_send![controller, performRequests];
            log::info!("WebAuthn: create ceremony started");
        } else {
            *PENDING_CEREMONY.lock().unwrap() = None;
            send_error(
                webview_ptr,
                &callback_id,
                "ASAuthorizationController not available",
            );
        }
    }
}

/// Run a `navigator.credentials.get()` ceremony via ASAuthorizationController.
#[cfg(target_os = "macos")]
fn run_get_ceremony(
    webview_ptr: usize,
    callback_id: String,
    rp_id: String,
    challenge: String,
    allow_credentials: Vec<String>,
    user_verification: String,
) {
    use objc2::runtime::{AnyClass, AnyObject};

    // Reject concurrent ceremonies
    {
        let mut pending = PENDING_CEREMONY.lock().unwrap();
        if pending.is_some() {
            drop(pending);
            send_error(webview_ptr, &callback_id, "Another WebAuthn ceremony is in progress");
            return;
        }
        *pending = Some((webview_ptr, callback_id.clone()));
    }

    unsafe {
        let challenge_data = b64url_to_nsdata(&challenge);

        // Bind CString to local to prevent use-after-free
        let rp_id_cstr = std::ffi::CString::new(rp_id.as_str()).unwrap_or_default();
        let rp_id_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: rp_id_cstr.as_ptr()
        ];

        // Map userVerification to AS preference NSString
        let uv_str: &[u8] = match user_verification.as_str() {
            "required" => b"required\0",
            "discouraged" => b"discouraged\0",
            _ => b"preferred\0",
        };
        let uv_ns: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: uv_str.as_ptr() as *const i8
        ];

        let requests: *mut AnyObject = objc2::msg_send![
            AnyClass::get("NSMutableArray").unwrap(),
            new
        ];

        // Platform passkey assertion
        if let Some(cls) =
            AnyClass::get("ASAuthorizationPlatformPublicKeyCredentialProvider")
        {
            let provider: *mut AnyObject = objc2::msg_send![cls, alloc];
            let provider: *mut AnyObject =
                objc2::msg_send![provider, initWithRelyingPartyIdentifier: rp_id_ns];
            if !provider.is_null() {
                let request: *mut AnyObject = objc2::msg_send![
                    provider,
                    createCredentialAssertionRequestWithChallenge: challenge_data
                ];
                if !request.is_null() {
                    // Set allowed credentials if specified
                    if !allow_credentials.is_empty() {
                        let descs: *mut AnyObject = objc2::msg_send![
                            AnyClass::get("NSMutableArray").unwrap(),
                            new
                        ];
                        for cred_b64 in &allow_credentials {
                            let cred_data = b64url_to_nsdata(cred_b64);
                            if let Some(desc_cls) = AnyClass::get(
                                "ASAuthorizationPlatformPublicKeyCredentialDescriptor",
                            ) {
                                let desc: *mut AnyObject =
                                    objc2::msg_send![desc_cls, alloc];
                                let desc: *mut AnyObject = objc2::msg_send![
                                    desc,
                                    initWithCredentialID: cred_data
                                ];
                                if !desc.is_null() {
                                    let _: () =
                                        objc2::msg_send![descs, addObject: desc];
                                }
                            }
                        }
                        let _: () =
                            objc2::msg_send![request, setAllowedCredentials: descs];
                    }
                    let _: () = objc2::msg_send![request, setUserVerificationPreference: uv_ns];
                    let _: () = objc2::msg_send![requests, addObject: request];
                }
            }
        }

        // Security key assertion
        if let Some(cls) =
            AnyClass::get("ASAuthorizationSecurityKeyPublicKeyCredentialProvider")
        {
            let provider: *mut AnyObject = objc2::msg_send![cls, alloc];
            let provider: *mut AnyObject =
                objc2::msg_send![provider, initWithRelyingPartyIdentifier: rp_id_ns];
            if !provider.is_null() {
                let request: *mut AnyObject = objc2::msg_send![
                    provider,
                    createCredentialAssertionRequestWithChallenge: challenge_data
                ];
                if !request.is_null() {
                    if !allow_credentials.is_empty() {
                        let descs: *mut AnyObject = objc2::msg_send![
                            AnyClass::get("NSMutableArray").unwrap(),
                            new
                        ];
                        for cred_b64 in &allow_credentials {
                            let cred_data = b64url_to_nsdata(cred_b64);
                            if let Some(desc_cls) = AnyClass::get(
                                "ASAuthorizationSecurityKeyPublicKeyCredentialDescriptor",
                            ) {
                                let desc: *mut AnyObject =
                                    objc2::msg_send![desc_cls, alloc];
                                let desc: *mut AnyObject = objc2::msg_send![
                                    desc,
                                    initWithCredentialID: cred_data
                                ];
                                if !desc.is_null() {
                                    // Set transports to [USB]
                                    // ASAuthorizationSecurityKeyPublicKeyCredentialDescriptorTransportUSB = 1
                                    let transports: *mut AnyObject = objc2::msg_send![
                                        AnyClass::get("NSMutableArray").unwrap(),
                                        new
                                    ];
                                    let usb_str: *mut AnyObject = objc2::msg_send![
                                        AnyClass::get("NSString").unwrap(),
                                        stringWithUTF8String: b"usb\0".as_ptr() as *const i8
                                    ];
                                    let _: () = objc2::msg_send![
                                        transports, addObject: usb_str
                                    ];
                                    let _: () = objc2::msg_send![
                                        desc, setTransports: transports
                                    ];
                                    let _: () =
                                        objc2::msg_send![descs, addObject: desc];
                                }
                            }
                        }
                        let _: () =
                            objc2::msg_send![request, setAllowedCredentials: descs];
                    }
                    let _: () = objc2::msg_send![request, setUserVerificationPreference: uv_ns];
                    let _: () = objc2::msg_send![requests, addObject: request];
                }
            }
        }

        let count: usize = objc2::msg_send![requests, count];
        if count == 0 {
            *PENDING_CEREMONY.lock().unwrap() = None;
            send_error(
                webview_ptr,
                &callback_id,
                "No authentication providers available",
            );
            return;
        }

        if let Some(ctrl_cls) = AnyClass::get("ASAuthorizationController") {
            let controller: *mut AnyObject = objc2::msg_send![ctrl_cls, alloc];
            let controller: *mut AnyObject =
                objc2::msg_send![controller, initWithAuthorizationRequests: requests];
            if controller.is_null() {
                *PENDING_CEREMONY.lock().unwrap() = None;
                send_error(webview_ptr, &callback_id, "Failed to create controller");
                return;
            }

            let delegate: *mut AnyObject =
                objc2::msg_send![delegate_class(), new];
            let _: () = objc2::msg_send![controller, setDelegate: delegate];
            let _: () =
                objc2::msg_send![controller, setPresentationContextProvider: delegate];
            // Balance +1 from `new` — controller retains delegate
            let _: () = objc2::msg_send![delegate, release];

            let _: () = objc2::msg_send![controller, performRequests];
            log::info!("WebAuthn: get ceremony started");
        } else {
            *PENDING_CEREMONY.lock().unwrap() = None;
            send_error(
                webview_ptr,
                &callback_id,
                "ASAuthorizationController not available",
            );
        }
    }
}
