//! Data Connection API for sandboxed SPA tabs.
//!
//! Apps call `window.epoca.data.connect(peerAddress)`,
//! `window.epoca.data.send(connId, data)`, and
//! `window.epoca.data.close(connId)` for peer-to-peer data exchange.
//!
//! Transport: str0m (Sans-I/O WebRTC) with SDP signaling over the Statement Store.
//!
//! Flow:
//! 1. SPA calls `connect(peerAddress)` → conn enters PendingApproval
//! 2. User approves → conn enters Signaling
//! 3. Deterministic offerer selection (lower address = offerer)
//! 4. Offerer creates SDP offer, publishes to statement store
//! 5. Answerer receives offer, creates SDP answer, publishes to statement store
//! 6. ICE connectivity checks complete → Connected
//! 7. Data flows over str0m data channel → events pushed to SPA
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
    /// Signal to stop the background thread.
    running: Option<Arc<AtomicBool>>,
    /// Channel to send data to the background thread.
    data_tx: Option<std::sync::mpsc::SyncSender<Vec<u8>>>,
    /// Signaling subscription IDs to clean up on close.
    signal_sub_ids: Vec<u64>,
}

struct DataState {
    next_conn_id: u64,
    connections: HashMap<u64, PeerConnection>,
    pending_events: Vec<DataEvent>,
    /// Our peer identity for signaling (derived from statement store keypair).
    local_peer_id: String,
}

static STATE: OnceLock<Mutex<DataState>> = OnceLock::new();

fn state() -> &'static Mutex<DataState> {
    STATE.get_or_init(|| {
        // Derive peer ID from the statement store's ephemeral public key.
        // Falls back to timestamp if store isn't initialized yet.
        let peer_id = epoca_chain::statement_store::public_key_hex()
            .map(|hex| format!("peer-{}", &hex[..16.min(hex.len())]))
            .unwrap_or_else(|| {
                format!(
                    "peer-{:016x}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64
                )
            });
        Mutex::new(DataState {
            next_conn_id: 1,
            connections: HashMap::new(),
            pending_events: Vec::new(),
            local_peer_id: peer_id,
        })
    })
}

// ---------------------------------------------------------------------------
// Signaling channel naming (matches web3-meet conventions)
// ---------------------------------------------------------------------------

fn offer_channel(app_id: &str, from: &str, to: &str) -> String {
    format!("{app_id}-offers-from-{from}-to-{to}")
}

fn answer_channel(app_id: &str, from: &str, to: &str) -> String {
    format!("{app_id}-answers-from-{from}-to-{to}")
}

/// Deterministic offerer: lower peer ID is the offerer.
fn is_offerer(my_id: &str, their_id: &str) -> bool {
    my_id < their_id
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initiate a data connection to a peer.
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
            running: None,
            data_tx: None,
            signal_sub_ids: Vec::new(),
        },
    );

    log::info!(
        "[data] connect request conn={conn_id} app={app_id} peer={peer_address} (pending approval)"
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

/// Drain pending data events (called from workbench render loop).
pub fn drain_events() -> Vec<DataEvent> {
    let mut st = state().lock().unwrap();
    std::mem::take(&mut st.pending_events)
}

/// Clean up all connections for a closed webview.
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
    let local_peer_id = st.local_peer_id.clone();
    let conn = st
        .connections
        .get_mut(&conn_id)
        .ok_or_else(|| format!("connection {conn_id} not found"))?;

    if conn.state != ConnState::PendingApproval {
        return Err(format!("connection {conn_id} not pending approval"));
    }

    conn.state = ConnState::Signaling;
    let app_id = conn.app_id.clone();
    let peer_address = conn.peer_address.clone();
    let webview_ptr = conn.webview_ptr;

    let running = Arc::new(AtomicBool::new(true));
    conn.running = Some(running.clone());

    let (data_tx, data_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(64);
    conn.data_tx = Some(data_tx);

    // Set up dedicated signaling channels via subscribe_direct.
    let offerer = is_offerer(&local_peer_id, &peer_address);
    let offer_ch = if offerer {
        offer_channel(&app_id, &local_peer_id, &peer_address)
    } else {
        offer_channel(&app_id, &peer_address, &local_peer_id)
    };
    let answer_ch = if offerer {
        answer_channel(&app_id, &peer_address, &local_peer_id)
    } else {
        answer_channel(&app_id, &local_peer_id, &peer_address)
    };

    // Subscribe to the channel we need to listen on.
    let listen_ch = if offerer { &answer_ch } else { &offer_ch };
    let (sub_id, signal_rx) = crate::statements_api::subscribe_direct(&app_id, listen_ch)
        .map_err(|e| format!("subscribe signaling: {e}"))?;
    conn.signal_sub_ids.push(sub_id);

    drop(st);

    log::info!("[data] approved conn={conn_id}, starting signaling (offerer={offerer})");

    std::thread::spawn(move || {
        let result = run_webrtc(
            conn_id,
            &app_id,
            webview_ptr,
            &local_peer_id,
            &peer_address,
            &running,
            &data_rx,
            &signal_rx,
            sub_id,
        );

        if let Err(e) = &result {
            log::warn!("[data] conn={conn_id} failed: {e}");
            push_event(DataEvent {
                webview_ptr,
                event_type: DataEventType::Error {
                    conn_id,
                    error: e.clone(),
                },
            });
        }

        // Clean up signaling subscription.
        crate::statements_api::unsubscribe(sub_id);

        let mut st = state().lock().unwrap();
        if let Some(conn) = st.connections.get_mut(&conn_id) {
            conn.state = ConnState::Closed;
            conn.signal_sub_ids.clear();
        }
    });

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
// Background WebRTC thread
// ---------------------------------------------------------------------------

fn run_webrtc(
    conn_id: u64,
    app_id: &str,
    webview_ptr: usize,
    local_peer_id: &str,
    peer_address: &str,
    running: &AtomicBool,
    data_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    signal_rx: &std::sync::mpsc::Receiver<crate::statements_api::Statement>,
    _signal_sub_id: u64,
) -> Result<(), String> {
    // Bind a UDP socket for ICE.
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("UDP bind: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    let local_addr = socket
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;

    log::info!("[data] conn={conn_id} bound UDP {local_addr}");

    // Create str0m Rtc.
    let mut rtc = Rtc::builder()
        .set_ice_lite(false)
        .set_stats_interval(None)
        .build(Instant::now());

    // Add local host candidate.
    let candidate =
        Candidate::host(local_addr, "udp").map_err(|e| format!("host candidate: {e}"))?;
    rtc.add_local_candidate(candidate);

    let offerer = is_offerer(local_peer_id, peer_address);
    log::info!(
        "[data] conn={conn_id} offerer={offerer} local={local_peer_id} peer={peer_address}"
    );

    // Channel names for signaling (no '/' — validated by statements_api).
    let offer_ch = if offerer {
        offer_channel(app_id, local_peer_id, peer_address)
    } else {
        offer_channel(app_id, peer_address, local_peer_id)
    };
    let answer_ch = if offerer {
        answer_channel(app_id, peer_address, local_peer_id)
    } else {
        answer_channel(app_id, local_peer_id, peer_address)
    };

    let mut data_ch_id: Option<ChannelId> = None;
    let mut pending_offer = None;

    if offerer {
        // Create data channel and generate offer.
        let mut api = rtc.sdp_api();
        let ch_id = api.add_channel("epoca-data".to_string());
        data_ch_id = Some(ch_id);

        let (offer, p) = api.apply().ok_or("SDP apply returned None")?;
        let offer_sdp = offer.to_sdp_string();
        pending_offer = Some(p);

        log::info!(
            "[data] conn={conn_id} publishing offer ({} bytes)",
            offer_sdp.len()
        );

        // Publish offer via statement store.
        let payload = serde_json::json!({
            "type": "offer",
            "from": local_peer_id,
            "sdp": offer_sdp,
        });
        crate::statements_api::write(app_id, local_peer_id, &offer_ch, &payload.to_string())
            .map_err(|e| format!("publish offer: {e}"))?;
    }

    // Signaling + ICE loop.
    let mut connected = false;
    let mut signaling_done = false;
    let mut buf = vec![0u8; 4096];
    let start = Instant::now();
    let timeout = Duration::from_secs(30);

    while running.load(Ordering::Acquire) && !connected {
        if start.elapsed() > timeout {
            return Err("signaling timeout (30s)".into());
        }

        // Poll for signaling messages from the dedicated receiver.
        if !signaling_done {
            while let Ok(stmt) = signal_rx.try_recv() {
                let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let sdp_str = json.get("sdp").and_then(|v| v.as_str()).unwrap_or("");
                if sdp_str.is_empty() {
                    continue;
                }

                if !offerer && msg_type == "offer" {
                    log::info!("[data] conn={conn_id} received offer, generating answer");
                    let offer = SdpOffer::from_sdp_string(sdp_str)
                        .map_err(|e| format!("parse offer SDP: {e}"))?;
                    let answer = rtc
                        .sdp_api()
                        .accept_offer(offer)
                        .map_err(|e| format!("accept offer: {e}"))?;

                    let answer_sdp = answer.to_sdp_string();

                    let payload = serde_json::json!({
                        "type": "answer",
                        "from": local_peer_id,
                        "sdp": answer_sdp,
                    });
                    crate::statements_api::write(
                        app_id,
                        local_peer_id,
                        &answer_ch,
                        &payload.to_string(),
                    )
                    .map_err(|e| format!("publish answer: {e}"))?;

                    signaling_done = true;
                    log::info!("[data] conn={conn_id} answer published, waiting for ICE");
                } else if offerer && msg_type == "answer" {
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

        // Drive str0m.
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
                    data_ch_id = Some(ch_id);

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
                            event_type: DataEventType::Data {
                                conn_id,
                                data: text,
                            },
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
        loop {
            match socket.recv_from(&mut buf) {
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

        // Send queued outgoing data.
        if connected {
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
    }

    if !connected {
        return Err("connection thread stopped before establishing".into());
    }

    // Connected — run data loop until closed.
    log::info!("[data] conn={conn_id} entering data loop");
    while running.load(Ordering::Acquire) {
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
                            event_type: DataEventType::Data {
                                conn_id,
                                data: text,
                            },
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

        // Feed incoming UDP.
        loop {
            match socket.recv_from(&mut buf) {
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

        // Send outgoing data.
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

    Ok(())
}

/// Push an event to the global pending events queue.
fn push_event(event: DataEvent) {
    let mut st = state().lock().unwrap();
    st.pending_events.push(event);
}
