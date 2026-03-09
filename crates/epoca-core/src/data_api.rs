//! Data Connection API for sandboxed SPA tabs.
//!
//! Apps call `window.epoca.data.connect(peerAddress)`,
//! `window.epoca.data.send(connId, data)`, and
//! `window.epoca.data.close(connId)` for peer-to-peer data exchange.
//!
//! Transport: str0m (Sans-I/O WebRTC) with SDP signaling over the Statement Store.
//!
//! Flow (initiator side):
//! 1. SPA calls `connect(peerAddress)` → conn enters PendingApproval
//! 2. User approves → conn enters Signaling
//! 3. Initiator creates SDP offer, publishes to `{app}-offer-to-{peer}`
//! 4. Waits for answer on `{app}-answer-to-{local}`
//! 5. ICE completes → Connected
//!
//! Flow (receiver side):
//! 1. Incoming listener detects offer on `{app}-offer-to-{local}`
//! 2. Creates PendingApproval connection → approval dialog shown
//! 3. User approves → generates answer from stored offer SDP
//! 4. Publishes answer to `{app}-answer-to-{initiator}`
//! 5. ICE completes → Connected
//!
//! Connections are namespaced by `app_id` — app A cannot access app B's connections.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use str0m::channel::ChannelId;
use str0m::net::Receive;
use str0m::change::{SdpAnswer, SdpOffer};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

/// Maximum data payload size (256 KB).
const MAX_DATA_SIZE: usize = 256 * 1024;

/// Signaling poll interval.
const SIGNAL_POLL_MS: u64 = 500;

/// Google's public STUN servers for NAT traversal.
const STUN_SERVERS: &[&str] = &[
    "stun.l.google.com:19302",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
];

/// STUN magic cookie (RFC 5389).
const STUN_MAGIC: u32 = 0x2112_A442;

/// Timeout per STUN server attempt.
const STUN_TIMEOUT: Duration = Duration::from_secs(3);

/// Connection states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// Waiting for user approval dialog.
    PendingApproval,
    /// Signaling in progress (SDP offer/answer exchange).
    Signaling,
    /// Connected and ready for data transfer.
    Connected,
    /// Connection closed or failed.
    Closed,
}

/// Direction of a connection.
#[derive(Debug, Clone)]
enum ConnDirection {
    /// We initiated — we are the offerer.
    Outgoing,
    /// They initiated — we received an offer and will answer.
    Incoming { offer_sdp: String },
}

/// Events to push to webviews.
#[derive(Debug)]
pub struct DataEvent {
    pub webview_ptr: usize,
    pub event_type: DataEventType,
}

#[derive(Debug)]
pub enum DataEventType {
    Connected { conn_id: u64, peer_address: String },
    Data { conn_id: u64, data: String },
    Closed { conn_id: u64, reason: String },
    Error { conn_id: u64, error: String },
}

/// A managed peer connection (metadata only — Rtc lives on background thread).
struct PeerConnection {
    conn_id: u64,
    app_id: String,
    webview_ptr: usize,
    peer_address: String,
    state: ConnState,
    direction: ConnDirection,
    /// Signal to stop the background thread.
    running: Option<Arc<AtomicBool>>,
    /// Channel to send data to the background thread.
    data_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    /// Signaling subscription IDs to clean up on close.
    signal_sub_ids: Vec<u64>,
}

/// Tracks a running incoming listener for one (app_id, webview_ptr).
struct IncomingListener {
    app_id: String,
    webview_ptr: usize,
    running: Arc<AtomicBool>,
    sub_id: u64,
}

struct DataState {
    next_conn_id: u64,
    next_peer_seq: u64,
    connections: HashMap<u64, PeerConnection>,
    pending_events: Vec<DataEvent>,
    /// Per-webview peer IDs. Each SPA tab gets its own identity so two tabs
    /// in the same process can connect to each other via local delivery.
    peer_ids: HashMap<usize, String>,
    /// Active incoming-offer listeners, keyed by (app_id, webview_ptr).
    listeners: Vec<IncomingListener>,
}

static STATE: OnceLock<Mutex<DataState>> = OnceLock::new();

fn state() -> &'static Mutex<DataState> {
    STATE.get_or_init(|| {
        Mutex::new(DataState {
            next_conn_id: 1,
            next_peer_seq: 1,
            connections: HashMap::new(),
            pending_events: Vec::new(),
            peer_ids: HashMap::new(),
            listeners: Vec::new(),
        })
    })
}

/// Get or create a peer ID for a webview. Each tab gets a unique ID based on
/// the statement store public key + a sequence number.
fn peer_id_for(st: &mut DataState, webview_ptr: usize) -> String {
    if let Some(id) = st.peer_ids.get(&webview_ptr) {
        return id.clone();
    }
    let base = epoca_chain::statement_store::public_key_hex()
        .unwrap_or_else(|| "0000000000000000".to_string());
    let seq = st.next_peer_seq;
    st.next_peer_seq += 1;
    let id = format!("peer-{}-{seq}", &base[..16.min(base.len())]);
    st.peer_ids.insert(webview_ptr, id.clone());
    id
}

// ---------------------------------------------------------------------------
// Signaling channel naming
// ---------------------------------------------------------------------------

/// Channel where initiator publishes their SDP offer, targeted at the receiver.
fn offer_channel(app_id: &str, target_peer: &str) -> String {
    format!("{app_id}-offer-to-{target_peer}")
}

/// Channel where receiver publishes their SDP answer, targeted at the initiator.
fn answer_channel(app_id: &str, target_peer: &str) -> String {
    format!("{app_id}-answer-to-{target_peer}")
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initiate a data connection to a peer (outgoing).
/// Returns a conn_id on success (connection is in PendingApproval state).
pub fn connect(
    app_id: &str,
    webview_ptr: usize,
    peer_address: &str,
) -> Result<u64, String> {
    if peer_address.is_empty() {
        return Err("peer address cannot be empty".into());
    }

    let mut st = state().lock().unwrap();
    let conn_id = st.next_conn_id;
    st.next_conn_id += 1;

    st.connections.insert(
        conn_id,
        PeerConnection {
            conn_id,
            app_id: app_id.to_string(),
            webview_ptr,
            peer_address: peer_address.to_string(),
            state: ConnState::PendingApproval,
            direction: ConnDirection::Outgoing,
            running: None,
            data_tx: None,
            signal_sub_ids: Vec::new(),
        },
    );

    log::info!(
        "[data] outgoing connect request conn={conn_id} app={app_id} peer={peer_address}"
    );

    Ok(conn_id)
}

/// Send data over an established connection.
pub fn send(app_id: &str, conn_id: u64, data: &str) -> Result<(), String> {
    if data.len() > MAX_DATA_SIZE {
        return Err(format!(
            "data too large ({} bytes, max {})",
            data.len(),
            MAX_DATA_SIZE,
        ));
    }

    let st = state().lock().unwrap();
    let conn = st
        .connections
        .get(&conn_id)
        .ok_or_else(|| format!("connection {conn_id} not found"))?;

    if conn.app_id != app_id {
        return Err("connection belongs to a different app".into());
    }

    match conn.state {
        ConnState::Connected => {
            if let Some(tx) = &conn.data_tx {
                tx.try_send(data.as_bytes().to_vec())
                    .map_err(|_| "send buffer full".to_string())?;
                Ok(())
            } else {
                Err("data channel not ready".into())
            }
        }
        ConnState::PendingApproval => Err("connection pending approval".into()),
        ConnState::Signaling => Err("connection still signaling".into()),
        ConnState::Closed => Err("connection is closed".into()),
    }
}

/// Close a data connection.
pub fn close(app_id: &str, conn_id: u64) -> Result<(), String> {
    let mut st = state().lock().unwrap();

    // Check ownership first without removing.
    let (peer, webview_ptr, has_running, sub_ids) = {
        let conn = st
            .connections
            .get(&conn_id)
            .ok_or_else(|| format!("connection {conn_id} not found"))?;
        if conn.app_id != app_id {
            return Err("connection belongs to a different app".into());
        }
        (
            conn.peer_address.clone(),
            conn.webview_ptr,
            conn.running.as_ref().map(|r| r.clone()),
            conn.signal_sub_ids.clone(),
        )
    };

    // Stop the background thread.
    if let Some(running) = has_running {
        running.store(false, Ordering::Release);
    }

    // Clean up signaling subscriptions.
    for sub_id in &sub_ids {
        crate::statements_api::unsubscribe(*sub_id);
    }

    st.connections.remove(&conn_id);
    st.pending_events.push(DataEvent {
        webview_ptr,
        event_type: DataEventType::Closed {
            conn_id,
            reason: "closed by app".into(),
        },
    });

    log::info!("[data] close conn={conn_id} peer={peer}");
    Ok(())
}

/// Get the local peer ID for a given webview (creates one if needed).
pub fn local_peer_id(webview_ptr: usize) -> String {
    let mut st = state().lock().unwrap();
    peer_id_for(&mut st, webview_ptr)
}

/// Drain pending data events (called from workbench render loop).
pub fn drain_events() -> Vec<DataEvent> {
    let mut st = state().lock().unwrap();
    std::mem::take(&mut st.pending_events)
}

/// Clean up all connections and listeners for a closed webview.
pub fn cleanup_for_webview(webview_ptr: usize) {
    let mut st = state().lock().unwrap();
    let mut sub_ids_to_clean = Vec::new();
    for conn in st.connections.values() {
        if conn.webview_ptr == webview_ptr {
            if let Some(running) = &conn.running {
                running.store(false, Ordering::Release);
            }
            sub_ids_to_clean.extend_from_slice(&conn.signal_sub_ids);
        }
    }
    st.connections
        .retain(|_, conn| conn.webview_ptr != webview_ptr);
    st.pending_events
        .retain(|e| e.webview_ptr != webview_ptr);

    // Stop incoming listeners for this webview.
    for listener in &st.listeners {
        if listener.webview_ptr == webview_ptr {
            listener.running.store(false, Ordering::Release);
            sub_ids_to_clean.push(listener.sub_id);
        }
    }
    st.listeners.retain(|l| l.webview_ptr != webview_ptr);
    st.peer_ids.remove(&webview_ptr);

    drop(st);

    // Clean up signaling subscriptions outside the lock.
    for sub_id in sub_ids_to_clean {
        crate::statements_api::unsubscribe(sub_id);
    }
}

/// Get pending approval requests for the workbench approval dialog.
pub fn pending_approvals() -> Vec<(u64, String, String)> {
    let st = state().lock().unwrap();
    st.connections
        .values()
        .filter(|c| c.state == ConnState::PendingApproval)
        .map(|c| (c.conn_id, c.app_id.clone(), c.peer_address.clone()))
        .collect()
}

/// Approve a pending connection (called after user approves in dialog).
pub fn approve_connection(conn_id: u64) -> Result<(), String> {
    let mut st = state().lock().unwrap();

    // Extract info and peer ID before taking a mutable ref to the connection.
    let (webview_ptr, app_id, peer_address, direction) = {
        let conn = st.connections.get(&conn_id)
            .ok_or_else(|| format!("connection {conn_id} not found"))?;
        if conn.state != ConnState::PendingApproval {
            return Err(format!("connection {conn_id} not pending approval"));
        }
        (conn.webview_ptr, conn.app_id.clone(), conn.peer_address.clone(), conn.direction.clone())
    };
    let local_peer_id = peer_id_for(&mut st, webview_ptr);

    let conn = st.connections.get_mut(&conn_id).unwrap();
    conn.state = ConnState::Signaling;

    let running = Arc::new(AtomicBool::new(true));
    conn.running = Some(running.clone());

    let (data_tx, data_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    conn.data_tx = Some(data_tx);

    match &direction {
        ConnDirection::Outgoing => {
            // Outgoing: we are the offerer.
            // Subscribe to answers targeted at us.
            let ans_ch = answer_channel(&app_id, &local_peer_id);
            let (sub_id, signal_rx) = crate::statements_api::subscribe_direct(&app_id, &ans_ch)
                .map_err(|e| format!("subscribe answer channel: {e}"))?;
            conn.signal_sub_ids.push(sub_id);
            drop(st);

            log::info!("[data] approved outgoing conn={conn_id}, starting as offerer");

            std::thread::spawn(move || {
                let result = run_webrtc_offerer(
                    conn_id, &app_id, webview_ptr, &local_peer_id, &peer_address,
                    &running, &data_rx, &signal_rx,
                );
                finish_connection(conn_id, webview_ptr, sub_id, &result);
            });
        }
        ConnDirection::Incoming { offer_sdp } => {
            // Incoming: we have the offer SDP, we are the answerer.
            let offer_sdp = offer_sdp.clone();
            // No signaling subscription needed — we already have the offer.
            drop(st);

            log::info!("[data] approved incoming conn={conn_id}, starting as answerer");

            std::thread::spawn(move || {
                let result = run_webrtc_answerer(
                    conn_id, &app_id, webview_ptr, &local_peer_id, &peer_address,
                    &offer_sdp, &running, &data_rx,
                );
                finish_connection(conn_id, webview_ptr, 0, &result);
            });
        }
    }

    Ok(())
}

/// Deny a pending connection (called after user denies in dialog).
pub fn deny_connection(conn_id: u64) -> Result<(), String> {
    let mut st = state().lock().unwrap();
    let conn = st
        .connections
        .get(&conn_id)
        .ok_or_else(|| format!("connection {conn_id} not found"))?;

    let webview_ptr = conn.webview_ptr;
    st.connections.remove(&conn_id);
    st.pending_events.push(DataEvent {
        webview_ptr,
        event_type: DataEventType::Error {
            conn_id,
            error: "connection denied by user".into(),
        },
    });

    log::info!("[data] denied conn={conn_id}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Incoming connection listener
// ---------------------------------------------------------------------------

/// Start listening for incoming data connection offers for a given app/webview.
/// Call this when a SPA tab with `data = true` permission opens.
pub fn start_incoming_listener(app_id: &str, webview_ptr: usize) {
    let mut st = state().lock().unwrap();
    let local_peer_id = peer_id_for(&mut st, webview_ptr);

    // Don't start a duplicate listener.
    if st.listeners.iter().any(|l| l.app_id == app_id && l.webview_ptr == webview_ptr) {
        return;
    }

    // Subscribe to offers targeted at us.
    let channel = offer_channel(app_id, &local_peer_id);
    let (sub_id, signal_rx) = match crate::statements_api::subscribe_direct(app_id, &channel) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("[data] failed to start incoming listener: {e}");
            return;
        }
    };

    let running = Arc::new(AtomicBool::new(true));
    st.listeners.push(IncomingListener {
        app_id: app_id.to_string(),
        webview_ptr,
        running: running.clone(),
        sub_id,
    });

    let app_id = app_id.to_string();
    drop(st);

    log::info!("[data] incoming listener started for app={app_id} on channel={channel}");

    std::thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            match signal_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(stmt) => {
                    let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let sdp_str = json.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                    let from_peer = json.get("from").and_then(|v| v.as_str()).unwrap_or("");

                    if msg_type != "offer" || sdp_str.is_empty() || from_peer.is_empty() {
                        continue;
                    }

                    log::info!(
                        "[data] incoming offer from {from_peer} for app={app_id}"
                    );

                    // Create an incoming PendingApproval connection.
                    let mut st = state().lock().unwrap();
                    let conn_id = st.next_conn_id;
                    st.next_conn_id += 1;

                    st.connections.insert(
                        conn_id,
                        PeerConnection {
                            conn_id,
                            app_id: app_id.clone(),
                            webview_ptr,
                            peer_address: from_peer.to_string(),
                            state: ConnState::PendingApproval,
                            direction: ConnDirection::Incoming {
                                offer_sdp: sdp_str.to_string(),
                            },
                            running: None,
                            data_tx: None,
                            signal_sub_ids: Vec::new(),
                        },
                    );

                    log::info!(
                        "[data] created incoming conn={conn_id} from {from_peer} (pending approval)"
                    );
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        log::info!("[data] incoming listener stopped for app={app_id}");
    });
}

// ---------------------------------------------------------------------------
// Background WebRTC threads
// ---------------------------------------------------------------------------

/// Clean up after a connection thread finishes.
fn finish_connection(conn_id: u64, webview_ptr: usize, sub_id: u64, result: &Result<(), String>) {
    if let Err(e) = result {
        log::warn!("[data] conn={conn_id} failed: {e}");
        push_event(DataEvent {
            webview_ptr,
            event_type: DataEventType::Error {
                conn_id,
                error: e.clone(),
            },
        });
    }

    if sub_id > 0 {
        crate::statements_api::unsubscribe(sub_id);
    }

    let mut st = state().lock().unwrap();
    if let Some(conn) = st.connections.get_mut(&conn_id) {
        conn.state = ConnState::Closed;
        conn.signal_sub_ids.clear();
    }
}

/// Run WebRTC as the offerer (outgoing connection).
fn run_webrtc_offerer(
    conn_id: u64,
    app_id: &str,
    webview_ptr: usize,
    local_peer_id: &str,
    peer_address: &str,
    running: &AtomicBool,
    data_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    signal_rx: &std::sync::mpsc::Receiver<crate::statements_api::Statement>,
) -> Result<(), String> {
    // Health check: verify statement store is reachable before signaling.
    epoca_chain::statement_store::ping()
        .map_err(|e| format!("signaling unavailable: {e}"))?;

    let (socket, local_addr, srflx_addr) = bind_ice_socket(conn_id)?;

    let now = Instant::now();
    let mut rtc = Rtc::builder()
        .set_ice_lite(false)
        .set_stats_interval(None)
        .build(now);
    // Initialize DTLS timing before any SDP or poll operations.
    let _ = rtc.handle_input(Input::Timeout(now));

    let candidate =
        Candidate::host(local_addr, "udp").map_err(|e| format!("host candidate: {e}"))?;
    rtc.add_local_candidate(candidate);

    if let Some(srflx) = srflx_addr {
        match Candidate::server_reflexive(srflx, local_addr, "udp") {
            Ok(c) => { rtc.add_local_candidate(c); }
            Err(e) => log::warn!("[data] conn={conn_id} srflx candidate error: {e}"),
        }
    }

    // Create data channel and generate offer.
    let mut api = rtc.sdp_api();
    let ch_id = api.add_channel("epoca-data".to_string());
    let (offer, pending) = api.apply().ok_or("SDP apply returned None")?;
    let offer_sdp = offer.to_sdp_string();

    log::info!("[data] conn={conn_id} publishing offer ({} bytes)", offer_sdp.len());

    // Publish offer targeted at the peer.
    let off_ch = offer_channel(app_id, peer_address);
    let payload = serde_json::json!({
        "type": "offer",
        "from": local_peer_id,
        "sdp": offer_sdp,
    });
    crate::statements_api::write(app_id, local_peer_id, &off_ch, &payload.to_string())
        .map_err(|e| format!("publish offer: {e}"))?;

    // Block-wait for the SDP answer BEFORE entering the ICE loop.
    // DTLS is only initialized when accept_answer() calls set_active(), so calling
    // poll_output() before that causes a dimpl panic ("need handle_timeout before
    // poll_output").  We solve this by receiving the answer in a simple blocking
    // loop that never touches poll_output.
    let answer_timeout = Duration::from_secs(30);
    let wait_start = Instant::now();
    let answer_sdp = loop {
        if !running.load(Ordering::Acquire) {
            return Err("connection stopped while waiting for answer".into());
        }
        if wait_start.elapsed() > answer_timeout {
            return Err("signaling timeout: no answer received (30s)".into());
        }
        match signal_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(stmt) => {
                let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let sdp_str = json.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                if msg_type == "answer" && !sdp_str.is_empty() {
                    break sdp_str.to_string();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("signaling channel disconnected".into());
            }
        }
    };

    log::info!("[data] conn={conn_id} received answer, accepting");
    let answer = SdpAnswer::from_sdp_string(&answer_sdp)
        .map_err(|e| format!("parse answer SDP: {e}"))?;
    rtc.sdp_api()
        .accept_answer(pending, answer)
        .map_err(|e| format!("accept answer: {e}"))?;
    log::info!("[data] conn={conn_id} answer accepted, starting ICE");

    // Now DTLS is initialized — safe to enter the poll_output loop.
    let mut data_ch_id = Some(ch_id);

    signaling_and_ice_loop(
        conn_id, app_id, webview_ptr, local_peer_id, peer_address,
        &socket, local_addr, &mut rtc, running, data_rx,
        &mut data_ch_id, &mut None, None, true,
    )
}

/// Run WebRTC as the answerer (incoming connection with pre-received offer).
fn run_webrtc_answerer(
    conn_id: u64,
    app_id: &str,
    webview_ptr: usize,
    local_peer_id: &str,
    peer_address: &str,
    offer_sdp: &str,
    running: &AtomicBool,
    data_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
) -> Result<(), String> {
    // Health check: verify statement store is reachable before signaling.
    epoca_chain::statement_store::ping()
        .map_err(|e| format!("signaling unavailable: {e}"))?;

    let (socket, local_addr, srflx_addr) = bind_ice_socket(conn_id)?;

    let now = Instant::now();
    let mut rtc = Rtc::builder()
        .set_ice_lite(false)
        .set_stats_interval(None)
        .build(now);
    // Initialize DTLS timing before any SDP or poll operations.
    let _ = rtc.handle_input(Input::Timeout(now));

    let candidate =
        Candidate::host(local_addr, "udp").map_err(|e| format!("host candidate: {e}"))?;
    rtc.add_local_candidate(candidate);

    if let Some(srflx) = srflx_addr {
        match Candidate::server_reflexive(srflx, local_addr, "udp") {
            Ok(c) => { rtc.add_local_candidate(c); }
            Err(e) => log::warn!("[data] conn={conn_id} srflx candidate error: {e}"),
        }
    }

    // Accept the offer and generate answer.
    let offer = SdpOffer::from_sdp_string(offer_sdp)
        .map_err(|e| format!("parse offer SDP: {e}"))?;
    let answer = rtc
        .sdp_api()
        .accept_offer(offer)
        .map_err(|e| format!("accept offer: {e}"))?;

    let answer_sdp = answer.to_sdp_string();
    log::info!("[data] conn={conn_id} publishing answer ({} bytes)", answer_sdp.len());

    // Publish answer targeted at the initiator.
    let ans_ch = answer_channel(app_id, peer_address);
    let payload = serde_json::json!({
        "type": "answer",
        "from": local_peer_id,
        "sdp": answer_sdp,
    });
    crate::statements_api::write(app_id, local_peer_id, &ans_ch, &payload.to_string())
        .map_err(|e| format!("publish answer: {e}"))?;

    let mut data_ch_id: Option<ChannelId> = None;

    signaling_and_ice_loop(
        conn_id, app_id, webview_ptr, local_peer_id, peer_address,
        &socket, local_addr, &mut rtc, running, data_rx,
        &mut data_ch_id, &mut None, None, false,
    )
}

// ---------------------------------------------------------------------------
// STUN NAT traversal
// ---------------------------------------------------------------------------

/// Build a minimal STUN Binding Request (20 bytes, no attributes).
fn build_stun_binding_request() -> ([u8; 20], [u8; 12]) {
    let mut pkt = [0u8; 20];
    // Message type: Binding Request (0x0001)
    pkt[0] = 0x00;
    pkt[1] = 0x01;
    // Message length: 0 (no attributes)
    pkt[2] = 0x00;
    pkt[3] = 0x00;
    // Magic cookie
    pkt[4..8].copy_from_slice(&STUN_MAGIC.to_be_bytes());
    // Transaction ID: timestamp nanos + process ID for uniqueness
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut txn_id = [0u8; 12];
    txn_id[0..8].copy_from_slice(&(nanos as u64).to_be_bytes());
    txn_id[8..12].copy_from_slice(&std::process::id().to_be_bytes());
    pkt[8..20].copy_from_slice(&txn_id);
    (pkt, txn_id)
}

/// Parse a STUN Binding Success Response to extract the mapped address.
fn parse_stun_response(buf: &[u8], expected_txn: &[u8; 12]) -> Option<std::net::SocketAddr> {
    if buf.len() < 20 {
        return None;
    }

    // Check message type: Binding Success Response (0x0101)
    let msg_type = u16::from_be_bytes([buf[0], buf[1]]);
    if msg_type != 0x0101 {
        return None;
    }

    // Check magic cookie
    if buf[4..8] != STUN_MAGIC.to_be_bytes() {
        return None;
    }

    // Check transaction ID matches our request
    if buf[8..20] != *expected_txn {
        return None;
    }

    let attr_len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
    if buf.len() < 20 + attr_len {
        return None;
    }

    // Walk attributes looking for XOR-MAPPED-ADDRESS (0x0020) or MAPPED-ADDRESS (0x0001)
    let mut pos = 20;
    let end = 20 + attr_len;
    let mut mapped_fallback: Option<std::net::SocketAddr> = None;

    while pos + 4 <= end {
        let attr_type = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let alen = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
        let attr_start = pos + 4;

        if attr_start + alen > end {
            break;
        }

        let attr_data = &buf[attr_start..attr_start + alen];
        match attr_type {
            0x0020 => {
                // XOR-MAPPED-ADDRESS — preferred, return immediately
                if let Some(addr) = parse_xor_mapped(attr_data, expected_txn) {
                    return Some(addr);
                }
            }
            0x0001 => {
                // MAPPED-ADDRESS — fallback
                mapped_fallback = parse_mapped(attr_data);
            }
            _ => {}
        }

        // Advance to next attribute (padded to 4-byte boundary)
        pos = attr_start + ((alen + 3) & !3);
    }

    mapped_fallback
}

fn parse_xor_mapped(data: &[u8], txn_id: &[u8; 12]) -> Option<std::net::SocketAddr> {
    if data.len() < 8 {
        return None;
    }
    let family = data[1];
    let xor_port = u16::from_be_bytes([data[2], data[3]]);
    let port = xor_port ^ (STUN_MAGIC >> 16) as u16;

    match family {
        0x01 => {
            // IPv4: XOR with magic cookie
            let magic = STUN_MAGIC.to_be_bytes();
            let ip = std::net::Ipv4Addr::new(
                data[4] ^ magic[0],
                data[5] ^ magic[1],
                data[6] ^ magic[2],
                data[7] ^ magic[3],
            );
            Some(std::net::SocketAddr::new(ip.into(), port))
        }
        0x02 if data.len() >= 20 => {
            // IPv6: XOR with magic cookie + transaction ID
            let mut addr = [0u8; 16];
            addr.copy_from_slice(&data[4..20]);
            let magic = STUN_MAGIC.to_be_bytes();
            for i in 0..4 {
                addr[i] ^= magic[i];
            }
            for i in 0..12 {
                addr[4 + i] ^= txn_id[i];
            }
            let ip = std::net::Ipv6Addr::from(addr);
            Some(std::net::SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

fn parse_mapped(data: &[u8]) -> Option<std::net::SocketAddr> {
    if data.len() < 8 {
        return None;
    }
    let family = data[1];
    let port = u16::from_be_bytes([data[2], data[3]]);

    match family {
        0x01 => {
            let ip = std::net::Ipv4Addr::new(data[4], data[5], data[6], data[7]);
            Some(std::net::SocketAddr::new(ip.into(), port))
        }
        0x02 if data.len() >= 20 => {
            let mut addr = [0u8; 16];
            addr.copy_from_slice(&data[4..20]);
            let ip = std::net::Ipv6Addr::from(addr);
            Some(std::net::SocketAddr::new(ip.into(), port))
        }
        _ => None,
    }
}

/// Resolve public IP:port via STUN binding requests.
///
/// Tries Google STUN servers in sequence, returns the first successful result.
/// The socket must be in **blocking** mode (read timeout will be set internally).
fn resolve_stun(
    socket: &UdpSocket,
    local_addr: std::net::SocketAddr,
    conn_id: u64,
) -> Option<std::net::SocketAddr> {
    use std::net::ToSocketAddrs;

    let start = Instant::now();
    let (request, txn_id) = build_stun_binding_request();

    log::info!(
        "[data] conn={conn_id} STUN: resolving public address ({} servers)",
        STUN_SERVERS.len()
    );

    // Set read timeout for blocking recv
    if socket.set_read_timeout(Some(Duration::from_secs(1))).is_err() {
        log::warn!("[data] conn={conn_id} STUN: failed to set read timeout");
        return None;
    }

    let mut buf = [0u8; 512];

    for server in STUN_SERVERS {
        // Resolve STUN server DNS
        let server_addr = match server.to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => {
                    log::warn!("[data] conn={conn_id} STUN: no addresses for {server}");
                    continue;
                }
            },
            Err(e) => {
                log::warn!("[data] conn={conn_id} STUN: DNS failed for {server}: {e}");
                continue;
            }
        };

        let attempt_start = Instant::now();

        // Try twice per server (initial + one retransmit)
        for attempt in 0..2u8 {
            if let Err(e) = socket.send_to(&request, server_addr) {
                log::warn!("[data] conn={conn_id} STUN: send to {server} failed: {e}");
                break;
            }

            // Read responses until timeout
            let deadline = Instant::now() + STUN_TIMEOUT;
            while Instant::now() < deadline {
                match socket.recv_from(&mut buf) {
                    Ok((n, from)) if from.ip() == server_addr.ip() => {
                        if let Some(mapped) = parse_stun_response(&buf[..n], &txn_id) {
                            log::info!(
                                "[data] conn={conn_id} STUN: {server} → {mapped} ({}ms, attempt {})",
                                attempt_start.elapsed().as_millis(),
                                attempt + 1,
                            );
                            log::info!(
                                "[data] conn={conn_id} STUN: local={local_addr} public={mapped} (total {}ms)",
                                start.elapsed().as_millis(),
                            );
                            return Some(mapped);
                        }
                    }
                    Ok(_) => {
                        // Response from unexpected source, keep waiting
                    }
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        // Read timeout expired, try next attempt/server
                        break;
                    }
                    Err(_) => break,
                }
            }

            log::info!(
                "[data] conn={conn_id} STUN: {server} attempt {} timed out ({}ms)",
                attempt + 1,
                attempt_start.elapsed().as_millis(),
            );
        }
    }

    log::warn!(
        "[data] conn={conn_id} STUN: all {} servers failed ({}ms) — host candidate only",
        STUN_SERVERS.len(),
        start.elapsed().as_millis(),
    );
    None
}

/// Bind a UDP socket for ICE, resolve STUN, and return (socket, host_addr, optional srflx_addr).
fn bind_ice_socket(
    conn_id: u64,
) -> Result<(UdpSocket, std::net::SocketAddr, Option<std::net::SocketAddr>), String> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind: {e}"))?;
    let bound_addr = socket
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;

    let local_ip = local_network_ip().unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let local_addr = std::net::SocketAddr::new(local_ip, bound_addr.port());

    log::info!("[data] conn={conn_id} bound UDP {bound_addr} → host candidate {local_addr}");

    // Resolve public address via STUN (blocking, before ICE starts)
    let srflx_addr = resolve_stun(&socket, local_addr, conn_id);

    // Switch to non-blocking for ICE
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;

    Ok((socket, local_addr, srflx_addr))
}

/// Shared signaling + ICE + data loop used by both offerer and answerer.
#[allow(clippy::too_many_arguments)]
fn signaling_and_ice_loop(
    conn_id: u64,
    _app_id: &str,
    webview_ptr: usize,
    _local_peer_id: &str,
    peer_address: &str,
    socket: &UdpSocket,
    local_addr: std::net::SocketAddr,
    rtc: &mut Rtc,
    running: &AtomicBool,
    data_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    data_ch_id: &mut Option<ChannelId>,
    pending_offer: &mut Option<str0m::change::SdpPendingOffer>,
    signal_rx: Option<&std::sync::mpsc::Receiver<crate::statements_api::Statement>>,
    is_offerer: bool,
) -> Result<(), String> {
    let mut connected = false;
    let mut signaling_done = signal_rx.is_none(); // answerer has no signal_rx — already done
    let mut buf = vec![0u8; 4096];
    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    // Signaling + ICE phase.
    while running.load(Ordering::Acquire) && !connected {
        if start.elapsed() > timeout {
            return Err("signaling timeout (30s)".into());
        }

        // Poll signaling (offerer waiting for answer).
        if !signaling_done {
            if let Some(rx) = signal_rx {
                while let Ok(stmt) = rx.try_recv() {
                    let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let sdp_str = json.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                    if sdp_str.is_empty() {
                        continue;
                    }

                    if is_offerer && msg_type == "answer" {
                        log::info!("[data] conn={conn_id} received answer");
                        let answer = SdpAnswer::from_sdp_string(sdp_str)
                            .map_err(|e| format!("parse answer SDP: {e}"))?;
                        if let Some(p) = pending_offer.take() {
                            rtc.sdp_api()
                                .accept_answer(p, answer)
                                .map_err(|e| format!("accept answer: {e}"))?;
                        }
                        signaling_done = true;
                        log::info!("[data] conn={conn_id} answer accepted, waiting for ICE");
                    }
                }
            }
        }

        // Drive str0m — must feed timeout before polling.
        let _ = rtc.handle_input(Input::Timeout(Instant::now()));
        match rtc.poll_output() {
            Ok(Output::Transmit(tx)) => {
                let _ = socket.send_to(&tx.contents, tx.destination);
            }
            Ok(Output::Timeout(t)) => {
                let wait = t.saturating_duration_since(Instant::now());
                let sleep = wait.min(Duration::from_millis(SIGNAL_POLL_MS));
                std::thread::sleep(sleep);
            }
            Ok(Output::Event(ev)) => match ev {
                Event::IceConnectionStateChange(IceConnectionState::Connected)
                | Event::IceConnectionStateChange(IceConnectionState::Completed) => {
                    log::info!("[data] conn={conn_id} ICE connected");
                }
                Event::ChannelOpen(ch_id, label) => {
                    log::info!(
                        "[data] conn={conn_id} data channel open: {label} (id={ch_id:?})"
                    );
                    connected = true;
                    *data_ch_id = Some(ch_id);

                    {
                        let mut st = state().lock().unwrap();
                        if let Some(conn) = st.connections.get_mut(&conn_id) {
                            conn.state = ConnState::Connected;
                        }
                    }
                    push_event(DataEvent {
                        webview_ptr,
                        event_type: DataEventType::Connected {
                            conn_id,
                            peer_address: peer_address.to_string(),
                        },
                    });
                }
                Event::ChannelData(data) => {
                    if let Ok(text) = String::from_utf8(data.data.to_vec()) {
                        push_event(DataEvent {
                            webview_ptr,
                            event_type: DataEventType::Data { conn_id, data: text },
                        });
                    }
                }
                Event::ChannelClose(_) => {
                    return Ok(());
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    return Err("ICE disconnected".into());
                }
                _ => {}
            },
            Err(e) => {
                return Err(format!("rtc error: {e}"));
            }
        }

        // Feed incoming UDP packets to str0m.
        feed_udp(socket, rtc, local_addr, &mut buf);

        // Send queued outgoing data.
        if connected {
            send_queued(rtc, *data_ch_id, data_rx, conn_id);
        }
    }

    if !connected {
        return Err("connection thread stopped before establishing".into());
    }

    // Connected — run data loop until closed.
    log::info!("[data] conn={conn_id} entering data loop");
    while running.load(Ordering::Acquire) {
        let _ = rtc.handle_input(Input::Timeout(Instant::now()));
        match rtc.poll_output() {
            Ok(Output::Transmit(tx)) => {
                let _ = socket.send_to(&tx.contents, tx.destination);
            }
            Ok(Output::Timeout(t)) => {
                let wait = t.saturating_duration_since(Instant::now());
                std::thread::sleep(wait.min(Duration::from_millis(50)));
            }
            Ok(Output::Event(ev)) => match ev {
                Event::ChannelData(data) => {
                    if let Ok(text) = String::from_utf8(data.data.to_vec()) {
                        push_event(DataEvent {
                            webview_ptr,
                            event_type: DataEventType::Data { conn_id, data: text },
                        });
                    }
                }
                Event::ChannelClose(_) => {
                    push_event(DataEvent {
                        webview_ptr,
                        event_type: DataEventType::Closed {
                            conn_id,
                            reason: "closed by peer".into(),
                        },
                    });
                    return Ok(());
                }
                Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                    push_event(DataEvent {
                        webview_ptr,
                        event_type: DataEventType::Closed {
                            conn_id,
                            reason: "ICE disconnected".into(),
                        },
                    });
                    return Ok(());
                }
                _ => {}
            },
            Err(e) => {
                return Err(format!("rtc error: {e}"));
            }
        }

        feed_udp(socket, rtc, local_addr, &mut buf);
        send_queued(rtc, *data_ch_id, data_rx, conn_id);
    }

    Ok(())
}

/// Feed incoming UDP packets into str0m.
fn feed_udp(
    socket: &UdpSocket,
    rtc: &mut Rtc,
    local_addr: std::net::SocketAddr,
    buf: &mut [u8],
) {
    loop {
        match socket.recv_from(buf) {
            Ok((n, from)) => {
                let now = Instant::now();
                if let Some(r) =
                    Receive::new(str0m::net::Protocol::Udp, from, local_addr, &buf[..n]).ok()
                {
                    let _ = rtc.handle_input(Input::Receive(now, r));
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Send queued outgoing data through the data channel.
fn send_queued(
    rtc: &mut Rtc,
    data_ch_id: Option<ChannelId>,
    data_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    conn_id: u64,
) {
    if let Some(ch_id) = data_ch_id {
        while let Ok(data) = data_rx.try_recv() {
            if let Some(mut ch) = rtc.channel(ch_id) {
                if let Err(e) = ch.write(true, &data) {
                    log::warn!("[data] conn={conn_id} write error: {e}");
                }
            }
        }
    }
}

/// Push an event to the global pending events queue.
fn push_event(event: DataEvent) {
    let mut st = state().lock().unwrap();
    st.pending_events.push(event);
}

/// Discover a local network IP by connecting a UDP socket to a public address.
/// The socket is never actually sent data — connect() just configures routing.
fn local_network_ip() -> Option<std::net::IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    // Connect to a public DNS address to let the OS pick the outbound interface.
    sock.connect("8.8.8.8:80").ok()?;
    Some(sock.local_addr().ok()?.ip())
}
