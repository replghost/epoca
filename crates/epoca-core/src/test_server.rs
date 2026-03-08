//! Embedded HTTP test server for driving Epoca from external test scripts.
//!
//! Gated behind `#[cfg(feature = "test-server")]` and the `EPOCA_TEST=1` env var.
//! Listens on `127.0.0.1:9223`. Accepts requests on the main HTTP thread, sends
//! `TestCommand`s over a channel to the GPUI main thread, and blocks for the
//! response (same drain-channel pattern as `shield.rs`).

use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Snapshot types (serialized as JSON responses)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ApprovalSnapshot {
    pub kind: String,
    pub high_risk: bool,
}

#[derive(Serialize)]
pub struct AppSnapshot {
    pub active_tab_id: Option<u64>,
    pub tab_count: usize,
    pub tabs: Vec<TabSnapshot>,
    pub sidebar_mode: String,
    pub sidebar_anim: f32,
    pub omnibox_open: bool,
    pub url_bar_value: String,
    pub isolated_tabs: bool,
    pub active_context: Option<String>,
    pub pending_approval: Option<ApprovalSnapshot>,
}

#[derive(Serialize)]
pub struct TabSnapshot {
    pub id: u64,
    pub kind: String,
    pub title: String,
    pub url: Option<String>,
    pub active: bool,
    pub cursor_pointer: bool,
    pub blocked_count: u32,
    pub favicon_url: Option<String>,
    pub context_id: Option<String>,
    pub pinned: bool,
}

// ---------------------------------------------------------------------------
// Command channel (HTTP thread → GPUI main thread)
// ---------------------------------------------------------------------------

pub enum TestCommand {
    GetState {
        rsp: SyncSender<String>,
    },
    Action {
        body: String,
        rsp: SyncSender<String>,
    },
    EvalJs {
        js: String,
        eval_id: String,
        rsp: SyncSender<String>,
    },
}

static TEST_CHANNEL: OnceLock<(SyncSender<TestCommand>, Mutex<mpsc::Receiver<TestCommand>>)> =
    OnceLock::new();

fn test_channel() -> &'static (SyncSender<TestCommand>, Mutex<mpsc::Receiver<TestCommand>>) {
    TEST_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(64);
        (tx, Mutex::new(rx))
    })
}

// ---------------------------------------------------------------------------
// Pending JS eval results (correlation ID → response sender)
// ---------------------------------------------------------------------------

static PENDING_EVALS: OnceLock<Mutex<HashMap<String, SyncSender<String>>>> = OnceLock::new();

fn pending_evals() -> &'static Mutex<HashMap<String, SyncSender<String>>> {
    PENDING_EVALS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// Console log ring buffer (captures JS console output for test assertions)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct ConsoleEntry {
    pub seq: u64,
    pub level: String,
    pub message: String,
    pub timestamp_ms: u64,
}

struct ConsoleRing {
    entries: std::collections::VecDeque<ConsoleEntry>,
    next_seq: u64,
}

static CONSOLE_BUFFER: OnceLock<Mutex<ConsoleRing>> = OnceLock::new();

fn console_buffer() -> &'static Mutex<ConsoleRing> {
    CONSOLE_BUFFER.get_or_init(|| {
        Mutex::new(ConsoleRing {
            entries: std::collections::VecDeque::new(),
            next_seq: 0,
        })
    })
}

/// Push a console log entry into the ring buffer. Called from the epocaConsole
/// WKScriptMessageHandler when the `test-server` feature is active.
pub fn push_console_log(level: String, message: String) {
    let mut ring = console_buffer().lock().unwrap();
    let seq = ring.next_seq;
    ring.next_seq += 1;
    ring.entries.push_back(ConsoleEntry {
        seq,
        level,
        message,
        timestamp_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    });
    // Keep max 1000 entries
    while ring.entries.len() > 1000 {
        ring.entries.pop_front();
    }
}

fn get_console_since(since_seq: u64) -> Vec<ConsoleEntry> {
    let ring = console_buffer().lock().unwrap();
    ring.entries
        .iter()
        .filter(|e| e.seq >= since_seq)
        .cloned()
        .collect()
}

/// Called from the `epocaTestResult` WKScriptMessageHandler when JS posts
/// `{ id: "...", result: "..." }` back to native.
pub fn resolve_eval(id: String, result: String) {
    if let Some(sender) = pending_evals().lock().unwrap().remove(&id) {
        let _ = sender.send(result);
    }
}

// ---------------------------------------------------------------------------
// Test result channel (WKScriptMessageHandler → resolve_eval)
// ---------------------------------------------------------------------------

static TEST_RESULT_CHANNEL: OnceLock<(
    SyncSender<(String, String)>,
    Mutex<mpsc::Receiver<(String, String)>>,
)> = OnceLock::new();

fn test_result_channel() -> &'static (
    SyncSender<(String, String)>,
    Mutex<mpsc::Receiver<(String, String)>>,
) {
    TEST_RESULT_CHANNEL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(64);
        (tx, Mutex::new(rx))
    })
}

/// Post a test result from ObjC handler into the channel.
pub fn post_test_result(id: String, result: String) {
    let _ = test_result_channel().0.try_send((id, result));
}

/// Drain test result events and resolve pending evals. Called from drain_test_commands.
fn drain_test_results() {
    let ch = test_result_channel();
    let rx = ch.1.lock().unwrap();
    while let Ok((id, result)) = rx.try_recv() {
        resolve_eval(id, result);
    }
}

// ---------------------------------------------------------------------------
// Register epocaTestResult WKScriptMessageHandler
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
pub fn register_test_result_handler(uc: *mut objc2::runtime::AnyObject, _webview_ptr: usize) {
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    let cls = CLASS.get_or_init(|| {
        if let Some(c) = AnyClass::get("EpocaTestResultHandler") {
            return c;
        }
        unsafe {
            let superclass = AnyClass::get("NSObject").unwrap();
            let mut builder =
                ClassBuilder::new("EpocaTestResultHandler", superclass).unwrap();

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
                let id_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"id\0".as_ptr() as *const i8
                ];
                let result_key: *mut AnyObject = objc2::msg_send![
                    AnyClass::get("NSString").unwrap(),
                    stringWithUTF8String: b"result\0".as_ptr() as *const i8
                ];
                let id_val: *mut AnyObject = objc2::msg_send![body, objectForKey: id_key];
                let result_val: *mut AnyObject =
                    objc2::msg_send![body, objectForKey: result_key];
                if id_val.is_null() {
                    return;
                }
                let id_cstr: *const i8 = objc2::msg_send![id_val, UTF8String];
                if id_cstr.is_null() {
                    return;
                }
                let id = std::ffi::CStr::from_ptr(id_cstr)
                    .to_string_lossy()
                    .to_string();

                let result = if result_val.is_null() {
                    String::new()
                } else {
                    let cstr: *const i8 = objc2::msg_send![result_val, UTF8String];
                    if cstr.is_null() {
                        String::new()
                    } else {
                        std::ffi::CStr::from_ptr(cstr)
                            .to_string_lossy()
                            .to_string()
                    }
                };

                post_test_result(id, result);
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

    unsafe {
        let handler: *mut objc2::runtime::AnyObject = objc2::msg_send![*cls, new];
        if handler.is_null() {
            return;
        }
        let name: *mut objc2::runtime::AnyObject = objc2::msg_send![
            AnyClass::get("NSString").unwrap(),
            stringWithUTF8String: b"epocaTestResult\0".as_ptr() as *const i8
        ];
        let _: () = objc2::msg_send![uc, addScriptMessageHandler: handler name: name];
    }
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

/// Start the test server if `EPOCA_TEST=1` is set.
/// Call once from app init (idempotent — second call is a no-op).
pub fn maybe_start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if std::env::var("EPOCA_TEST").as_deref() != Ok("1") {
        return;
    }
    STARTED.get_or_init(|| {
        // Ensure channel is initialized before spawning the server thread.
        let _ = test_channel();
        std::thread::Builder::new()
            .name("test-server".into())
            .spawn(run_server)
            .expect("Failed to spawn test server thread");
        log::info!("Test server: starting on 127.0.0.1:9223");
    });
}

fn run_server() {
    let listener = match TcpListener::bind("127.0.0.1:9223") {
        Ok(l) => l,
        Err(e) => {
            log::error!("Test server: failed to bind port 9223: {e}");
            return;
        }
    };
    log::info!("Test server: listening on 127.0.0.1:9223");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                // Reject non-loopback peers.
                if let Ok(addr) = stream.peer_addr() {
                    if !addr.ip().is_loopback() {
                        continue;
                    }
                }
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .ok();
                std::thread::spawn(move || handle_connection(stream));
            }
            Err(e) => {
                log::warn!("Test server: accept error: {e}");
            }
        }
    }
}

fn handle_connection(mut stream: TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    // Read request line
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let parts: Vec<&str> = request_line.trim().split_whitespace().collect();
    if parts.len() < 2 {
        return;
    }
    let method = parts[0];
    let full_path = parts[1];

    // Split path and query string
    let (path, query) = match full_path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (full_path, ""),
    };

    // Read headers to find Content-Length
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
        if let Some(val) = trimmed.strip_prefix("content-length:") {
            content_length = val.trim().parse().unwrap_or(0);
        }
    }

    // Body size limit: 64KB
    if content_length > 65536 {
        write_response(&mut stream, 413, "Body too large");
        return;
    }

    // Read body
    let body = if content_length > 0 {
        let mut buf = vec![0u8; content_length];
        if reader.read_exact(&mut buf).is_err() {
            return;
        }
        String::from_utf8_lossy(&buf).to_string()
    } else {
        String::new()
    };

    // Route
    match (method, path) {
        ("GET", "/state") => {
            let (rsp_tx, rsp_rx) = mpsc::sync_channel(1);
            let cmd = TestCommand::GetState { rsp: rsp_tx };
            if test_channel().0.try_send(cmd).is_err() {
                write_response(&mut stream, 503, r#"{"error":"channel full"}"#);
                return;
            }
            match rsp_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(json) => write_json_response(&mut stream, 200, &json),
                Err(_) => write_response(&mut stream, 504, r#"{"error":"timeout"}"#),
            }
        }
        ("POST", "/action") => {
            let (rsp_tx, rsp_rx) = mpsc::sync_channel(1);
            let cmd = TestCommand::Action {
                body,
                rsp: rsp_tx,
            };
            if test_channel().0.try_send(cmd).is_err() {
                write_response(&mut stream, 503, r#"{"error":"channel full"}"#);
                return;
            }
            match rsp_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(json) => write_json_response(&mut stream, 200, &json),
                Err(_) => write_response(&mut stream, 504, r#"{"error":"timeout"}"#),
            }
        }
        ("GET", "/webview/eval") => {
            // Parse ?js=... from query string
            let js = parse_query_param(query, "js").unwrap_or_default();
            if js.is_empty() {
                write_response(&mut stream, 400, r#"{"error":"missing js param"}"#);
                return;
            }
            let eval_id = format!("eval_{}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos());
            let (rsp_tx, rsp_rx) = mpsc::sync_channel(1);
            let cmd = TestCommand::EvalJs {
                js,
                eval_id,
                rsp: rsp_tx,
            };
            if test_channel().0.try_send(cmd).is_err() {
                write_response(&mut stream, 503, r#"{"error":"channel full"}"#);
                return;
            }
            match rsp_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(json) => write_json_response(&mut stream, 200, &json),
                Err(_) => write_response(&mut stream, 504, r#"{"error":"eval timeout"}"#),
            }
        }
        ("POST", "/webview/wait") => {
            let parsed: serde_json::Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    write_response(&mut stream, 400, &format!(r#"{{"error":"bad json: {e}"}}"#));
                    return;
                }
            };
            let js = parsed
                .get("js")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let timeout_ms = parsed
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(5000);

            if js.is_empty() {
                write_response(&mut stream, 400, r#"{"error":"missing js"}"#);
                return;
            }

            let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
            let poll_interval = Duration::from_millis(100);

            loop {
                let eval_id = format!(
                    "wait_{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );

                let (rsp_tx, rsp_rx) = mpsc::sync_channel(1);
                let cmd = TestCommand::EvalJs {
                    js: js.clone(),
                    eval_id,
                    rsp: rsp_tx,
                };

                if test_channel().0.try_send(cmd).is_err() {
                    write_response(&mut stream, 503, r#"{"error":"channel full"}"#);
                    return;
                }

                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                match rsp_rx.recv_timeout(remaining.min(Duration::from_secs(2))) {
                    Ok(result) => {
                        if is_truthy(&result) {
                            let json = format!(r#"{{"ok":true,"value":{}}}"#, serde_json::json!(result));
                            write_json_response(&mut stream, 200, &json);
                            return;
                        }
                    }
                    Err(_) => {}
                }

                if std::time::Instant::now() >= deadline {
                    write_response(
                        &mut stream,
                        504,
                        r#"{"error":"wait timeout","value":null}"#,
                    );
                    return;
                }

                std::thread::sleep(poll_interval);
            }
        }
        ("GET", "/console") => {
            let since = parse_query_param(query, "since")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            let entries = get_console_since(since);
            let json = serde_json::to_string(&entries)
                .unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#));
            write_json_response(&mut stream, 200, &json);
        }
        _ => {
            write_response(&mut stream, 404, r#"{"error":"not found"}"#);
        }
    }
}

fn parse_query_param<'a>(query: &'a str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == key {
                // URL-decode the value (basic: %XX and +)
                return Some(url_decode(v));
            }
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let mut result = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                result.push(byte);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            result.push(b' ');
        } else {
            result.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&result).to_string()
}

/// Check if a JS eval result represents a truthy value.
fn is_truthy(result: &str) -> bool {
    !result.is_empty()
        && result != "false"
        && result != "null"
        && result != "undefined"
        && result != "0"
        && !result.starts_with("ERROR:")
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn write_json_response(stream: &mut TcpStream, status: u16, json: &str) {
    write_response(stream, status, json);
}

// ---------------------------------------------------------------------------
// Drain — called from Workbench::process_pending_nav on the GPUI main thread
// ---------------------------------------------------------------------------

use crate::tabs::WebViewTab;
use crate::workbench::{SidebarMode, Workbench};
use gpui::*;

pub fn drain_test_commands(wb: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    // First, drain any pending JS eval results from the WKScriptMessageHandler.
    drain_test_results();

    let ch = test_channel();
    let rx = ch.1.lock().unwrap();
    let mut cmds = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        cmds.push(cmd);
    }
    drop(rx);

    for cmd in cmds {
        match cmd {
            TestCommand::GetState { rsp } => {
                let snapshot = build_snapshot(wb, cx);
                let json = serde_json::to_string(&snapshot).unwrap_or_else(|e| {
                    format!(r#"{{"error":"serialize: {e}"}}"#)
                });
                let _ = rsp.send(json);
            }
            TestCommand::Action { body, rsp } => {
                let result = handle_action(wb, &body, window, cx);
                let _ = rsp.send(result);
            }
            TestCommand::EvalJs { js, eval_id, rsp } => {
                handle_eval_js(wb, &js, &eval_id, rsp, cx);
            }
        }
    }
}

fn build_snapshot(wb: &Workbench, cx: &App) -> AppSnapshot {
    let url_bar_value = wb.url_input.read(cx).value().to_string();
    let sidebar_mode = match wb.sidebar_mode {
        SidebarMode::Pinned => "pinned",
        SidebarMode::Overlay => "overlay",
    };

    let tabs: Vec<TabSnapshot> = wb
        .tabs
        .iter()
        .map(|tab| {
            let active = wb.active_tab_id == Some(tab.id);
            let (url, cursor_pointer, blocked_count) =
                if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                    let wv = entity.read(cx);
                    (
                        Some(wv.url().to_string()),
                        wv.cursor_pointer,
                        wv.blocked_count,
                    )
                } else {
                    (None, false, 0)
                };
            let kind = match &tab.kind {
                crate::tabs::TabKind::Welcome => "welcome".to_string(),
                crate::tabs::TabKind::Settings => "settings".to_string(),
                crate::tabs::TabKind::CodeEditor { .. } => "code_editor".to_string(),
                crate::tabs::TabKind::SandboxApp { .. } => "sandbox_app".to_string(),
                crate::tabs::TabKind::DeclarativeApp { .. } => "declarative_app".to_string(),
                crate::tabs::TabKind::WebView { .. } => "webview".to_string(),
                crate::tabs::TabKind::FramebufferApp { .. } => "framebuffer_app".to_string(),
                crate::tabs::TabKind::AppLibrary => "app_library".to_string(),
                crate::tabs::TabKind::Spa { .. } => "spa".to_string(),
                crate::tabs::TabKind::DotLoading { .. } => "dot_loading".to_string(),
                crate::tabs::TabKind::Bookmarks => "bookmarks".to_string(),
            };
            TabSnapshot {
                id: tab.id,
                kind,
                title: tab.title.clone(),
                url,
                active,
                cursor_pointer,
                blocked_count,
                favicon_url: tab.favicon_url.clone(),
                context_id: tab.context_id.clone(),
                pinned: tab.pinned,
            }
        })
        .collect();

    let pending_approval = wb.test_pending_approval_info().map(|(kind, high_risk)| {
        ApprovalSnapshot {
            kind: kind.to_string(),
            high_risk,
        }
    });

    AppSnapshot {
        active_tab_id: wb.active_tab_id,
        tab_count: tabs.len(),
        tabs,
        sidebar_mode: sidebar_mode.to_string(),
        sidebar_anim: wb.sidebar_anim,
        omnibox_open: wb.omnibox_open,
        url_bar_value,
        isolated_tabs: wb.isolated_tabs,
        active_context: wb.active_context.clone(),
        pending_approval,
    }
}

fn handle_action(
    wb: &mut Workbench,
    body: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> String {
    let parsed: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return format!(r#"{{"error":"bad json: {e}"}}"#),
    };
    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match action {
        "navigate" => {
            let url = parsed
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if url.is_empty() {
                return r#"{"error":"missing url"}"#.to_string();
            }
            // Handle .dot URLs through DOTNS resolution (same as URL bar)
            if url.starts_with("dot://") {
                wb.resolve_dot_url(&url, window, cx);
                return r#"{"ok":true}"#.to_string();
            }
            if url.ends_with(".dot") && !url.contains('/') && !url.contains(' ') {
                let dot_url = format!("dot://{}", url);
                wb.resolve_dot_url(&dot_url, window, cx);
                return r#"{"ok":true}"#.to_string();
            }
            // Navigate active tab or open new one
            if let Some(id) = wb.active_tab_id {
                if let Some(tab) = wb.tabs.iter().find(|t| t.id == id) {
                    if let Some(nav) = &tab.nav {
                        nav.load_url(&url, cx);
                        cx.notify();
                        return r#"{"ok":true}"#.to_string();
                    }
                }
            }
            // No active navigable tab — open a new one
            wb.open_webview(url, window, cx);
            r#"{"ok":true}"#.to_string()
        }
        "new_tab" => {
            wb.new_tab(window, cx);
            r#"{"ok":true}"#.to_string()
        }
        "close_tab" => {
            let tab_id = parsed
                .get("tab_id")
                .and_then(|v| v.as_u64())
                .or(wb.active_tab_id);
            if let Some(id) = tab_id {
                wb.close_tab_by_id(id, window, cx);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"no tab to close"}"#.to_string()
            }
        }
        "switch_tab" => {
            let tab_id = parsed.get("tab_id").and_then(|v| v.as_u64());
            if let Some(id) = tab_id {
                wb.switch_tab_by_id(id, window, cx);
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"missing tab_id"}"#.to_string()
            }
        }
        "reload" => {
            wb.reload_active_tab(false, window, cx);
            r#"{"ok":true}"#.to_string()
        }
        "set_isolated_tabs" => {
            let enabled = parsed
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            wb.isolated_tabs = enabled;
            cx.notify();
            r#"{"ok":true}"#.to_string()
        }
        "approve" => {
            if wb.test_approve_pending(window, cx) {
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"no pending approval"}"#.to_string()
            }
        }
        "deny" => {
            if wb.test_deny_pending(cx) {
                r#"{"ok":true}"#.to_string()
            } else {
                r#"{"error":"no pending approval"}"#.to_string()
            }
        }
        _ => {
            format!(r#"{{"error":"unknown action: {action}"}}"#)
        }
    }
}

fn handle_eval_js(
    wb: &Workbench,
    js: &str,
    eval_id: &str,
    rsp: SyncSender<String>,
    cx: &App,
) {
    // Find the active tab
    let active_tab = wb.active_tab_id.and_then(|id| {
        wb.tabs.iter().find(|t| t.id == id)
    });

    let Some(tab) = active_tab else {
        let _ = rsp.send(r#"{"error":"no active tab"}"#.to_string());
        return;
    };

    // Check if the tab supports JS eval before registering
    let can_eval = tab.entity.clone().downcast::<WebViewTab>().is_ok()
        || tab.entity.clone().downcast::<crate::tabs::SpaTab>().is_ok();

    if !can_eval {
        let _ = rsp.send(r#"{"error":"active tab does not support JS eval"}"#.to_string());
        return;
    }

    // Register the pending eval before injecting JS.
    pending_evals()
        .lock()
        .unwrap()
        .insert(eval_id.to_string(), rsp);

    // Wrap user JS in try/catch and post result back via epocaTestResult handler.
    let wrapped = format!(
        r#"(function(){{try{{var __r=String(eval({user_js}));window.webkit.messageHandlers.epocaTestResult.postMessage({{id:"{id}",result:__r}})}}catch(__e){{window.webkit.messageHandlers.epocaTestResult.postMessage({{id:"{id}",result:"ERROR:"+__e.message}})}}}})();"#,
        user_js = serde_json::to_string(js).unwrap_or_else(|_| format!("\"{}\"", js)),
        id = eval_id,
    );

    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
        entity.read(cx).evaluate_script(&wrapped, cx);
    } else if let Ok(entity) = tab.entity.clone().downcast::<crate::tabs::SpaTab>() {
        entity.read(cx).evaluate_script(&wrapped, cx);
    }
}
