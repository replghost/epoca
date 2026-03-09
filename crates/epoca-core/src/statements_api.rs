//! Statements API for sandboxed SPA tabs.
//!
//! Apps call `window.epoca.statements.write(channel, data)` and
//! `window.epoca.statements.subscribe(channel)` for pub/sub messaging.
//!
//! Dual-path delivery:
//! 1. **Local** — in-memory pub/sub for same-host subscribers (instant).
//! 2. **Network** — submitted to the Substrate Statement Store pallet for
//!    cross-host gossip. Received statements are delivered to local subscribers.
//!
//! Channels are namespaced by `app_id` — app A cannot see app B's channels.
//! The namespace is transparent to the app (it just uses bare channel names).

use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};

/// Maximum statement data size (256 KB).
const MAX_STATEMENT_SIZE: usize = 256 * 1024;

/// A single published statement.
#[derive(Debug, Clone)]
pub struct Statement {
    pub author: String,
    pub channel: String,
    pub data: String,
    pub timestamp_ms: u64,
}

/// Events to push to subscribed webviews.
#[derive(Debug)]
pub struct StatementEvent {
    pub webview_ptr: usize,
    pub statement: Statement,
}

/// Where to deliver a subscription's events.
enum SubscriptionTarget {
    /// Deliver to the shared pending_events queue for workbench drain.
    Webview(usize),
    /// Deliver directly to a dedicated channel (for internal signaling).
    Direct(mpsc::SyncSender<Statement>),
}

/// Subscription entry.
struct Subscription {
    id: u64,
    target: SubscriptionTarget,
}

/// Global state: namespaced channels → subscribers.
struct StatementsState {
    /// channel_key → list of subscribers
    subscriptions: HashMap<String, Vec<Subscription>>,
    /// Pending events to deliver to webviews (drained by workbench).
    pending_events: Vec<StatementEvent>,
    /// Monotonic subscription ID counter.
    next_sub_id: u64,
}

static STATE: OnceLock<Mutex<StatementsState>> = OnceLock::new();

fn state() -> &'static Mutex<StatementsState> {
    STATE.get_or_init(|| Mutex::new(StatementsState {
        subscriptions: HashMap::new(),
        pending_events: Vec::new(),
        next_sub_id: 1,
    }))
}

/// Build the namespaced channel key.
fn channel_key(app_id: &str, channel: &str) -> String {
    format!("{app_id}/{channel}")
}

// ---------------------------------------------------------------------------
// Network submit worker — bounded channel prevents unbounded thread spawn
// ---------------------------------------------------------------------------

struct SubmitJob {
    app_id: String,
    channel: String,
    payload: Vec<u8>,
    priority: u32,
}

static SUBMIT_TX: OnceLock<mpsc::SyncSender<SubmitJob>> = OnceLock::new();

fn submit_tx() -> &'static mpsc::SyncSender<SubmitJob> {
    SUBMIT_TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<SubmitJob>(32);
        std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                if let Err(e) = epoca_chain::statement_store::submit(
                    &job.app_id,
                    &job.channel,
                    &job.payload,
                    job.priority,
                ) {
                    log::warn!("[statements] network submit failed: {e}");
                }
            }
        });
        tx
    })
}

// ---------------------------------------------------------------------------
// Network bridge — Statement Store integration
// ---------------------------------------------------------------------------

/// Initialize the statement store network bridge.
///
/// Hooks the `epoca_chain::statement_store` poll loop so received statements
/// are delivered to local subscribers.
pub fn init_network_bridge() {
    epoca_chain::statement_store::set_on_statement(Box::new(|stmt| {
        on_network_statement(stmt);
    }));
    log::info!("[statements] network bridge initialized");
}

/// Called by the statement store poll loop when a statement arrives from the network.
fn on_network_statement(stmt: epoca_chain::statement_store::Statement) {
    // Decode the data as JSON to extract author, channel, data, timestamp.
    let json_str = match String::from_utf8(stmt.data.clone()) {
        Ok(s) => s,
        Err(_) => {
            let preview: Vec<u8> = stmt.data.iter().take(64).copied().collect();
            log::warn!(
                "[statements] network: non-UTF8 statement data ({} bytes, first 64: {:02x?}), topics={}, ch={}, dropping",
                stmt.data.len(),
                preview,
                stmt.topics.len(),
                stmt.channel.is_some(),
            );
            return;
        }
    };
    let json: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[statements] network: invalid JSON in statement: {e}");
            return;
        }
    };

    let author = json.get("author").and_then(|v| v.as_str()).unwrap_or("");
    let channel = json.get("channel").and_then(|v| v.as_str()).unwrap_or("");
    let data = json.get("data").and_then(|v| v.as_str()).unwrap_or("");
    let app_id = json.get("app_id").and_then(|v| v.as_str()).unwrap_or("");
    let timestamp = json
        .get("timestamp_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    if channel.is_empty() || app_id.is_empty() {
        return;
    }

    // Validate JSON app_id/channel match the signed binary fields to prevent
    // cross-app spoofing. A remote peer could forge JSON fields otherwise.
    let expected_dk = epoca_chain::statement_store::string_to_topic(app_id);
    if stmt.decryption_key != Some(expected_dk) {
        log::warn!("[statements] network: app_id/decryption_key mismatch, dropping");
        return;
    }
    let expected_ch = epoca_chain::statement_store::string_to_topic(channel);
    if stmt.channel != Some(expected_ch) {
        log::warn!("[statements] network: channel/binary-channel mismatch, dropping");
        return;
    }

    let statement = Statement {
        author: author.to_string(),
        channel: channel.to_string(),
        data: data.to_string(),
        timestamp_ms: timestamp,
    };

    let key = channel_key(app_id, channel);
    deliver(&key, &statement, channel, app_id);
}

// ---------------------------------------------------------------------------
// Public API (called from workbench drain loop)
// ---------------------------------------------------------------------------

/// Deliver a statement to all subscribers on a channel key.
///
/// Collects delivery targets first, then pushes events — avoids
/// simultaneous mutable+immutable borrows of `StatementsState`.
fn deliver(key: &str, statement: &Statement, channel: &str, app_id: &str) {
    let mut st = state().lock().unwrap();

    // Collect targets to avoid borrow conflict.
    let mut webview_targets: Vec<usize> = Vec::new();
    let mut direct_targets: Vec<mpsc::SyncSender<Statement>> = Vec::new();

    if let Some(subs) = st.subscriptions.get(key) {
        for sub in subs {
            match &sub.target {
                SubscriptionTarget::Webview(ptr) => webview_targets.push(*ptr),
                SubscriptionTarget::Direct(tx) => direct_targets.push(tx.clone()),
            }
        }
    }

    let count = webview_targets.len() + direct_targets.len();

    for ptr in webview_targets {
        st.pending_events.push(StatementEvent {
            webview_ptr: ptr,
            statement: statement.clone(),
        });
    }

    // Direct sends don't need the lock, but we already hold it and it's cheap.
    for tx in direct_targets {
        let _ = tx.try_send(statement.clone());
    }

    if count > 0 {
        log::info!(
            "[statements] deliver: channel={channel} app={app_id} ({count} subscribers)",
        );
    }
}

/// Write a statement to a channel.
///
/// Delivers locally to same-host subscribers AND submits to the Statement Store
/// for cross-host gossip.
pub fn write(
    app_id: &str,
    author: &str,
    channel: &str,
    data: &str,
) -> Result<(), String> {
    if data.len() > MAX_STATEMENT_SIZE {
        return Err(format!(
            "statement too large ({} bytes, max {})",
            data.len(),
            MAX_STATEMENT_SIZE,
        ));
    }

    if channel.is_empty() {
        return Err("channel name cannot be empty".into());
    }
    if channel.contains('/') {
        return Err("channel name must not contain '/'".into());
    }

    let key = channel_key(app_id, channel);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let statement = Statement {
        author: author.to_string(),
        channel: channel.to_string(),
        data: data.to_string(),
        timestamp_ms: now,
    };

    // 1. Local delivery.
    deliver(&key, &statement, channel, app_id);
    log::info!(
        "[statements] write channel={channel} app={app_id} ({} bytes)",
        data.len(),
    );

    // 2. Network delivery via bounded submit worker.
    let payload = serde_json::json!({
        "app_id": app_id,
        "author": author,
        "channel": channel,
        "data": data,
        "timestamp_ms": now,
    });
    let priority = (now / 1000) as u32; // seconds as priority (last-write-wins)

    let _ = submit_tx().try_send(SubmitJob {
        app_id: app_id.to_string(),
        channel: channel.to_string(),
        payload: payload.to_string().into_bytes(),
        priority,
    });

    Ok(())
}

/// Subscribe to a channel (webview delivery). Returns Ok on success.
pub fn subscribe(
    app_id: &str,
    webview_ptr: usize,
    channel: &str,
) -> Result<(), String> {
    if channel.is_empty() {
        return Err("channel name cannot be empty".into());
    }
    if channel.contains('/') {
        return Err("channel name must not contain '/'".into());
    }

    let key = channel_key(app_id, channel);
    let mut st = state().lock().unwrap();

    // Check for duplicate first.
    if let Some(subs) = st.subscriptions.get(&key) {
        if subs.iter().any(|s| matches!(&s.target, SubscriptionTarget::Webview(p) if *p == webview_ptr)) {
            return Ok(());
        }
    }

    let id = st.next_sub_id;
    st.next_sub_id += 1;

    st.subscriptions.entry(key).or_default().push(Subscription {
        id,
        target: SubscriptionTarget::Webview(webview_ptr),
    });
    log::info!("[statements] subscribe channel={channel} app={app_id} webview={webview_ptr:#x}");
    Ok(())
}

/// Subscribe to a channel with a dedicated receiver (for internal signaling).
///
/// Returns `(sub_id, Receiver)`. The caller must call `unsubscribe(sub_id)`
/// when done to clean up the subscription.
pub fn subscribe_direct(
    app_id: &str,
    channel: &str,
) -> Result<(u64, mpsc::Receiver<Statement>), String> {
    if channel.is_empty() {
        return Err("channel name cannot be empty".into());
    }
    if channel.contains('/') {
        return Err("channel name must not contain '/'".into());
    }

    let key = channel_key(app_id, channel);
    let mut st = state().lock().unwrap();

    let id = st.next_sub_id;
    st.next_sub_id += 1;

    let (tx, rx) = mpsc::sync_channel::<Statement>(64);

    st.subscriptions.entry(key).or_default().push(Subscription {
        id,
        target: SubscriptionTarget::Direct(tx),
    });

    log::info!("[statements] subscribe_direct channel={channel} app={app_id} sub_id={id}");
    Ok((id, rx))
}

/// Remove a subscription by ID (used to clean up direct subscriptions).
pub fn unsubscribe(sub_id: u64) {
    let mut st = state().lock().unwrap();
    for subs in st.subscriptions.values_mut() {
        subs.retain(|s| s.id != sub_id);
    }
    st.subscriptions.retain(|_, subs| !subs.is_empty());
    log::info!("[statements] unsubscribe sub_id={sub_id}");
}

/// Drain pending statement events (called from workbench render loop).
pub fn drain_events() -> Vec<StatementEvent> {
    let mut st = state().lock().unwrap();
    std::mem::take(&mut st.pending_events)
}

/// Clean up all subscriptions for a closed webview.
pub fn cleanup_for_webview(webview_ptr: usize) {
    let mut st = state().lock().unwrap();
    for subs in st.subscriptions.values_mut() {
        subs.retain(|s| !matches!(&s.target, SubscriptionTarget::Webview(p) if *p == webview_ptr));
    }
    // Remove empty channels.
    st.subscriptions.retain(|_, subs| !subs.is_empty());
    // Drop any pending events for this webview.
    st.pending_events.retain(|e| e.webview_ptr != webview_ptr);
}
