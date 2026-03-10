//! Media API — Phase A implementation using WebView-native WebRTC.
//!
//! Manages media sessions (getUserMedia, peer connections) by orchestrating
//! the WKWebView's built-in WebRTC stack via evaluateScript. App code only
//! sees opaque track/session IDs through window.epoca.media.*.
//!
//! MediaConnect flow:
//! 1. App calls media.connect(peer, trackIds) → session_id
//! 2. Host saves RTCPeerConnection constructor in __epocaRTC before freezing
//! 3. Host evaluates JS to create RTCPeerConnection, add tracks, generate offer
//! 4. JS callbacks post signaling data to host via messageHandler
//! 5. Host relays signaling via Statement Store to remote peer
//! 6. Remote host evaluates JS to apply remote SDP/ICE
//! 7. WebRTC connects, media flows directly between WebViews

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

/// Global monotonic track ID counter.
static NEXT_TRACK_ID: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(1));
/// Global monotonic session ID counter.
static NEXT_SESSION_ID: LazyLock<Mutex<u64>> = LazyLock::new(|| Mutex::new(1));

#[derive(Debug)]
pub struct MediaEvent {
    pub webview_ptr: usize,
    pub event_type: MediaEventType,
}

#[derive(Debug)]
pub enum MediaEventType {
    TrackReady { track_id: u64, kind: String },
    SessionConnected { session_id: u64, peer_address: String },
    RemoteTrack { session_id: u64, track_id: u64, kind: String },
    SessionClosed { session_id: u64, reason: String },
    Error { session_id: u64, error: String },
    /// Evaluate this JS on the webview (signaling thread → render loop).
    EvalJs { js: String },
}

struct MediaTrack {
    #[allow(dead_code)]
    track_id: u64,
    #[allow(dead_code)]
    kind: String,
    webview_ptr: usize,
}

struct MediaSession {
    session_id: u64,
    webview_ptr: usize,
    app_id: String,
    peer_address: String,
    track_ids: Vec<u64>,
    state: SessionState,
    /// Signal to stop the signaling background thread.
    signaling_running: Option<Arc<AtomicBool>>,
    /// Local peer ID used for this session's signaling channels.
    local_peer_id: String,
}

#[derive(Debug, Clone, PartialEq)]
enum SessionState {
    Signaling,
    Connected,
    #[allow(dead_code)]
    Closed,
}

struct PendingIncomingCall {
    caller_id: String,
    app_id: String,
    local_peer_id: String,
    /// Pre-subscribed signaling channel (sub_id, receiver).
    /// Signals buffer here between ring arrival and user accept.
    signal_sub: Option<(u64, std::sync::mpsc::Receiver<crate::statements_api::Statement>)>,
}

struct MediaState {
    tracks: HashMap<u64, MediaTrack>,
    sessions: HashMap<u64, MediaSession>,
    pending_events: Vec<MediaEvent>,
    /// Local track IDs per webview (set by getUserMedia, used by ring listener).
    local_tracks: HashMap<usize, Vec<u64>>,
    /// Webviews that already have a ring listener running.
    ring_listeners: std::collections::HashSet<usize>,
    /// Pending incoming calls that have not yet been accepted by the user.
    pending_incoming: HashMap<usize, PendingIncomingCall>,
    /// Stop flags for ring listener threads, keyed by webview_ptr.
    ring_listener_flags: HashMap<usize, Arc<AtomicBool>>,
}

static STATE: OnceLock<Mutex<MediaState>> = OnceLock::new();

fn state() -> &'static Mutex<MediaState> {
    STATE.get_or_init(|| {
        Mutex::new(MediaState {
            tracks: HashMap::new(),
            sessions: HashMap::new(),
            pending_events: Vec::new(),
            local_tracks: HashMap::new(),
            ring_listeners: std::collections::HashSet::new(),
            pending_incoming: HashMap::new(),
            ring_listener_flags: HashMap::new(),
        })
    })
}

fn next_track_id() -> u64 {
    let mut id = NEXT_TRACK_ID.lock().unwrap();
    let v = *id;
    *id += 1;
    v
}

fn next_session_id() -> u64 {
    let mut id = NEXT_SESSION_ID.lock().unwrap();
    let v = *id;
    *id += 1;
    v
}

// ---------------------------------------------------------------------------
// Peer identity (shared with data_api)
// ---------------------------------------------------------------------------

/// Get the local peer ID (derived from statement store keypair).
fn local_peer_id() -> String {
    epoca_chain::statement_store::public_key_hex()
        .map(|hex| format!("peer-{}", &hex[..16.min(hex.len())]))
        .unwrap_or_else(|| {
            format!(
                "peer-{:016x}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64
            )
        })
}

/// Public accessor for local peer ID.
pub fn local_peer_id_pub() -> String {
    local_peer_id()
}

/// Determine if we are the offerer (lower peer ID = offerer).
pub fn is_offerer(local_peer_id: &str, peer_address: &str) -> bool {
    local_peer_id < peer_address
}

/// Signaling channel name: from → to.
fn signal_channel(app_id: &str, from: &str, to: &str) -> String {
    format!("{app_id}-media-from-{from}-to-{to}")
}

// ---------------------------------------------------------------------------
// getUserMedia API
// ---------------------------------------------------------------------------

/// Request media tracks. Returns (audio_track_id, video_track_id).
pub fn request_get_user_media(
    webview_ptr: usize,
    audio: bool,
    video: bool,
) -> (Option<u64>, Option<u64>) {
    let mut st = state().lock().unwrap();

    let audio_track_id = if audio {
        let id = next_track_id();
        st.tracks.insert(id, MediaTrack { track_id: id, kind: "audio".into(), webview_ptr });
        Some(id)
    } else {
        None
    };

    let video_track_id = if video {
        let id = next_track_id();
        st.tracks.insert(id, MediaTrack { track_id: id, kind: "video".into(), webview_ptr });
        Some(id)
    } else {
        None
    };

    // Store local track IDs for this webview (used by ring listener for incoming calls).
    let mut ids = Vec::new();
    if let Some(id) = audio_track_id { ids.push(id); }
    if let Some(id) = video_track_id { ids.push(id); }
    st.local_tracks.insert(webview_ptr, ids);

    (audio_track_id, video_track_id)
}

/// JS to call getUserMedia and store tracks in __epocaMediaTracks.
pub fn get_user_media_js(
    audio_track_id: Option<u64>,
    video_track_id: Option<u64>,
    audio: bool,
    video: bool,
) -> String {
    format!(
        r#"(async function() {{
    try {{
        if (!window.__epocaMediaTracks) window.__epocaMediaTracks = {{}};
        // Recover navigator.mediaDevices from a blank iframe if unavailable
        // (custom URL schemes like epocaapp:// may not be secure contexts).
        var _md = navigator.mediaDevices;
        if (!_md || !_md.getUserMedia) {{
            var _f = document.createElement('iframe');
            _f.style.display = 'none';
            document.documentElement.appendChild(_f);
            _md = _f.contentWindow.navigator.mediaDevices;
            _f.remove();
        }}
        if (!_md || !_md.getUserMedia) {{
            throw new Error('getUserMedia not available (not a secure context?)');
        }}
        var stream = await _md.getUserMedia({{ audio: {audio_bool}, video: {video_bool} }});
        var tracks = stream.getTracks();
        for (var i = 0; i < tracks.length; i++) {{
            var t = tracks[i];
            var tid = t.kind === 'audio' ? {audio_tid} : {video_tid};
            if (tid > 0) {{
                window.__epocaMediaTracks[tid] = {{ stream: stream, track: t }};
                window.__epocaPush('mediaTrackReady', {{ trackId: tid, kind: t.kind }});
            }}
        }}
    }} catch(e) {{
        window.__epocaPush('mediaError', {{ sessionId: 0, error: e.message || 'getUserMedia failed' }});
    }}
}})()"#,
        audio_bool = if audio { "true" } else { "false" },
        video_bool = if video { "true" } else { "false" },
        audio_tid = audio_track_id.unwrap_or(0),
        video_tid = video_track_id.unwrap_or(0),
    )
}

/// JS to attach a track to a DOM element by id.
pub fn attach_track_js(track_id: u64, element_id: &str) -> String {
    let safe_element_id = element_id
        .replace('\\', "")
        .replace('\'', "")
        .replace('"', "")
        .replace('\n', "")
        .replace('\r', "");

    format!(
        r#"(function() {{
    var entry = window.__epocaMediaTracks && window.__epocaMediaTracks[{track_id}];
    if (!entry) {{ console.warn('epoca: track {track_id} not found'); return; }}
    var el = document.getElementById('{element_id}');
    if (!el) {{ console.warn('epoca: element {element_id} not found'); return; }}
    if (el.srcObject !== entry.stream) {{
        el.srcObject = entry.stream;
    }}
}})()"#,
        track_id = track_id,
        element_id = safe_element_id,
    )
}

/// JS to enable/disable a track (mute audio or disable camera).
pub fn set_track_enabled_js(track_id: u64, enabled: bool) -> String {
    format!(
        r#"(function() {{
    var entry = window.__epocaMediaTracks && window.__epocaMediaTracks[{track_id}];
    if (!entry || !entry.track) {{ console.warn('epoca: track {track_id} not found'); return; }}
    entry.track.enabled = {enabled};
}})()"#,
        track_id = track_id,
        enabled = if enabled { "true" } else { "false" },
    )
}

// ---------------------------------------------------------------------------
// Session lifecycle (MediaConnect)
// ---------------------------------------------------------------------------

/// Create a media session. Returns session_id.
pub fn create_session(
    webview_ptr: usize,
    app_id: &str,
    peer_address: &str,
    track_ids: Vec<u64>,
    local_peer_id: &str,
) -> u64 {
    let session_id = next_session_id();
    let mut st = state().lock().unwrap();
    st.sessions.insert(session_id, MediaSession {
        session_id,
        webview_ptr,
        app_id: app_id.to_string(),
        peer_address: peer_address.to_string(),
        track_ids,
        state: SessionState::Signaling,
        signaling_running: None,
        local_peer_id: local_peer_id.to_string(),
    });
    log::info!("[media] session {session_id} created for peer={peer_address}");
    session_id
}

/// Get the peer address for a session.
pub fn session_peer_address(session_id: u64) -> Option<String> {
    let st = state().lock().unwrap();
    st.sessions.get(&session_id).map(|s| s.peer_address.clone())
}

/// Store the signaling thread handle on a session.
pub fn set_signaling_handle(session_id: u64, handle: Arc<AtomicBool>) {
    let mut st = state().lock().unwrap();
    if let Some(session) = st.sessions.get_mut(&session_id) {
        session.signaling_running = Some(handle);
    }
}

/// Mark session as connected and push event.
pub fn session_connected(session_id: u64) {
    let mut st = state().lock().unwrap();
    let info = st.sessions.get_mut(&session_id).map(|s| {
        s.state = SessionState::Connected;
        (s.webview_ptr, s.peer_address.clone())
    });
    if let Some((webview_ptr, peer_address)) = info {
        st.pending_events.push(MediaEvent {
            webview_ptr,
            event_type: MediaEventType::SessionConnected { session_id, peer_address },
        });
    }
}

/// Register a remote track and push event.
pub fn push_remote_track(session_id: u64, track_id: u64, kind: &str) {
    let mut st = state().lock().unwrap();
    let webview_ptr = st.sessions.get(&session_id).map(|s| s.webview_ptr);
    if let Some(wv) = webview_ptr {
        st.tracks.insert(track_id, MediaTrack {
            track_id,
            kind: kind.to_string(),
            webview_ptr: wv,
        });
        st.pending_events.push(MediaEvent {
            webview_ptr: wv,
            event_type: MediaEventType::RemoteTrack {
                session_id,
                track_id,
                kind: kind.to_string(),
            },
        });
    }
}

/// Close a session and push event.
pub fn close_session(session_id: u64, reason: &str) {
    let mut st = state().lock().unwrap();
    if let Some(session) = st.sessions.remove(&session_id) {
        if let Some(running) = &session.signaling_running {
            running.store(false, Ordering::Release);
        }
        st.pending_events.push(MediaEvent {
            webview_ptr: session.webview_ptr,
            event_type: MediaEventType::SessionClosed {
                session_id,
                reason: reason.to_string(),
            },
        });
    }
}

/// Push an EvalJs event for the workbench to process.
fn push_eval_js(webview_ptr: usize, js: String) {
    let mut st = state().lock().unwrap();
    st.pending_events.push(MediaEvent {
        webview_ptr,
        event_type: MediaEventType::EvalJs { js },
    });
}

// ---------------------------------------------------------------------------
// Signaling relay (Statement Store ↔ WebView JS)
// ---------------------------------------------------------------------------

/// Parse a signaling statement and return the JS to apply it, if valid.
fn signal_statement_to_js(session_id: u64, stmt: &crate::statements_api::Statement) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(&stmt.data).ok()?;
    let sig_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let sig_data = json.get("data").and_then(|v| v.as_str()).unwrap_or("");
    if sig_type.is_empty() {
        return None;
    }
    let js = apply_signal_js(session_id, sig_type, sig_data);
    if js.is_empty() { None } else { Some(js) }
}

/// Start a signaling relay thread using an existing subscription receiver.
fn start_signaling_with_rx(
    session_id: u64,
    sub_id: u64,
    signal_rx: std::sync::mpsc::Receiver<crate::statements_api::Statement>,
    webview_ptr: usize,
) -> Arc<AtomicBool> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    std::thread::spawn(move || {
        log::info!("[media] signaling thread for session={session_id} (live)");
        while running_clone.load(Ordering::Acquire) {
            match signal_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(stmt) => {
                    if let Some(js) = signal_statement_to_js(session_id, &stmt) {
                        push_eval_js(webview_ptr, js);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        crate::statements_api::unsubscribe(sub_id);
        log::info!("[media] signaling thread for session={session_id} stopped");
    });

    running
}

/// Start a signaling relay thread for a media session.
/// Subscribes to the peer's signaling channel and pushes received signals
/// as EvalJs events for the workbench to apply on the webview.
pub fn start_signaling(
    session_id: u64,
    app_id: &str,
    peer_address: &str,
    local_peer_id: &str,
) -> Result<Arc<AtomicBool>, String> {
    let local_id = local_peer_id.to_string();
    let listen_ch = signal_channel(app_id, peer_address, &local_id);
    let (sub_id, signal_rx) = crate::statements_api::subscribe_direct(app_id, &listen_ch)
        .map_err(|e| format!("subscribe signaling: {e}"))?;

    let webview_ptr = {
        let st = state().lock().unwrap();
        st.sessions.get(&session_id)
            .map(|s| s.webview_ptr)
            .ok_or("session not found")?
    };

    Ok(start_signaling_with_rx(session_id, sub_id, signal_rx, webview_ptr))
}

/// Publish a signal to the remote peer via Statement Store.
pub fn publish_signal(
    session_id: u64,
    signal_type: &str,
    data: &str,
) -> Result<(), String> {
    let (app_id, peer_address, local_id) = {
        let st = state().lock().unwrap();
        let session = st.sessions.get(&session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        (session.app_id.clone(), session.peer_address.clone(), session.local_peer_id.clone())
    };

    let write_ch = signal_channel(&app_id, &local_id, &peer_address);

    let payload = serde_json::json!({
        "type": signal_type,
        "data": data,
    });

    crate::statements_api::write(&app_id, &local_id, &write_ch, &payload.to_string())
        .map_err(|e| format!("publish signal: {e}"))
}

// ---------------------------------------------------------------------------
// JS generation for RTCPeerConnection lifecycle
// ---------------------------------------------------------------------------

/// JS snippet that obtains native WebRTC constructors from a hidden iframe.
/// The main frame's `epocaapp://` scheme doesn't expose RTCPeerConnection,
/// but a blank iframe (about:blank) does.  The iframe is kept alive —
/// removing it would tear down the browsing context and invalidate the
/// native constructors WebKit handed us.
const GET_RTC_JS: &str = r#"
        if (!window.__epocaRTCFrame) {
            var _f = document.createElement('iframe');
            _f.style.display = 'none';
            document.documentElement.appendChild(_f);
            window.__epocaRTCFrame = _f;
        }
        var _RTC = window.__epocaRTCFrame.contentWindow.RTCPeerConnection;
        var _Desc = window.__epocaRTCFrame.contentWindow.RTCSessionDescription;
        var _ICE = window.__epocaRTCFrame.contentWindow.RTCIceCandidate;
        if (!_RTC) {
            console.error('epoca: WebRTC not available in iframe');
            throw new Error('WebRTC not available');
        }
"#;

/// JS to create RTCPeerConnection, add tracks, and (if offerer) generate offer.
/// Signaling callbacks post to host via messageHandler.
pub fn setup_session_js(session_id: u64, track_ids: &[u64], is_offerer: bool) -> String {
    let track_ids_json = track_ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"(async function() {{
    try {{
{get_rtc}
        if (!window.__epocaMediaTracks) window.__epocaMediaTracks = {{}};
        if (!window.__epocaMediaSessions) window.__epocaMediaSessions = {{}};
        var pc = new _RTC({{
            iceServers: [{{ urls: 'stun:stun.l.google.com:19302' }}]
        }});
        window.__epocaMediaSessions[{sid}] = {{ pc: pc, remoteTrackCount: 0 }};

        var trackIds = [{tids}];
        for (var i = 0; i < trackIds.length; i++) {{
            var entry = window.__epocaMediaTracks[trackIds[i]];
            if (entry && entry.track) {{
                pc.addTrack(entry.track, entry.stream);
            }}
        }}

        var sess = window.__epocaMediaSessions[{sid}];
        sess.iceCandidates = [];

        pc.onicecandidate = function(e) {{
            if (e.candidate) {{
                sess.iceCandidates.push(e.candidate);
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'candidate', data: JSON.stringify(e.candidate) }}
                }});
            }}
        }};

        pc.ontrack = function(e) {{
            var sess = window.__epocaMediaSessions[{sid}];
            if (!sess) return;
            sess.remoteTrackCount++;
            var rtid = 1000000 + {sid} * 1000 + sess.remoteTrackCount;
            window.__epocaMediaTracks[rtid] = {{
                stream: e.streams[0] || new MediaStream([e.track]),
                track: e.track
            }};
            window.webkit.messageHandlers.epocaHost.postMessage({{
                id: 0, method: 'mediaSignal',
                params: {{ sessionId: {sid}, type: 'remoteTrack',
                           data: JSON.stringify({{ trackId: rtid, kind: e.track.kind }}) }}
            }});
        }};

        pc.onconnectionstatechange = function() {{
            if (pc.connectionState === 'connected') {{
                if (sess.offerRetry) {{ clearInterval(sess.offerRetry); sess.offerRetry = null; }}
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'connected', data: '' }}
                }});
            }} else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {{
                if (sess.offerRetry) {{ clearInterval(sess.offerRetry); sess.offerRetry = null; }}
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'closed', data: pc.connectionState }}
                }});
            }}
        }};

        if ({offerer}) {{
            var offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            var offerJson = JSON.stringify(offer);
            window.webkit.messageHandlers.epocaHost.postMessage({{
                id: 0, method: 'mediaSignal',
                params: {{ sessionId: {sid}, type: 'offer', data: offerJson }}
            }});
            // Retry offer + ICE every 3s until answer received.
            sess.offerRetry = setInterval(function() {{
                if (!sess.pendingOffer) {{ clearInterval(sess.offerRetry); sess.offerRetry = null; return; }}
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'offer', data: offerJson }}
                }});
                for (var ci = 0; ci < sess.iceCandidates.length; ci++) {{
                    window.webkit.messageHandlers.epocaHost.postMessage({{
                        id: 0, method: 'mediaSignal',
                        params: {{ sessionId: {sid}, type: 'candidate', data: JSON.stringify(sess.iceCandidates[ci]) }}
                    }});
                }}
            }}, 3000);
            sess.pendingOffer = true;
        }}
    }} catch(e) {{
        console.error('epoca media setup error:', e);
        window.__epocaPush('mediaError', {{ sessionId: {sid}, error: e.message || 'setup failed' }});
    }}
}})()"#,
        get_rtc = GET_RTC_JS,
        sid = session_id,
        tids = track_ids_json,
        offerer = if is_offerer { "true" } else { "false" },
    )
}

/// JS to apply a remote signal (offer, answer, or ICE candidate) to a session.
/// SDP data is base64-encoded to avoid escaping issues.
pub fn apply_signal_js(session_id: u64, signal_type: &str, data: &str) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(data.as_bytes());

    match signal_type {
        "offer" => format!(
            r#"(async function() {{
    try {{
        var sess = window.__epocaMediaSessions && window.__epocaMediaSessions[{sid}];
        if (!sess || !sess.pc) return;
        // If we already processed an offer, re-send the stored answer.
        if (sess.pc.remoteDescription && sess.localAnswer) {{
            window.webkit.messageHandlers.epocaHost.postMessage({{
                id: 0, method: 'mediaSignal',
                params: {{ sessionId: {sid}, type: 'answer', data: sess.localAnswer }}
            }});
            return;
        }}
        var offer = JSON.parse(atob('{b64}'));
        await sess.pc.setRemoteDescription(offer);
        var answer = await sess.pc.createAnswer();
        await sess.pc.setLocalDescription(answer);
        sess.localAnswer = JSON.stringify(answer);
        window.webkit.messageHandlers.epocaHost.postMessage({{
            id: 0, method: 'mediaSignal',
            params: {{ sessionId: {sid}, type: 'answer', data: sess.localAnswer }}
        }});
    }} catch(e) {{ console.error('epoca: apply offer error:', e); }}
}})()"#,
            sid = session_id, b64 = b64,
        ),
        "answer" => format!(
            r#"(async function() {{
    try {{
        var sess = window.__epocaMediaSessions && window.__epocaMediaSessions[{sid}];
        if (!sess || !sess.pc) return;
        // Stop offer retry — answer received.
        sess.pendingOffer = false;
        if (sess.offerRetry) {{ clearInterval(sess.offerRetry); sess.offerRetry = null; }}
        var answer = JSON.parse(atob('{b64}'));
        await sess.pc.setRemoteDescription(answer);
    }} catch(e) {{ console.error('epoca: apply answer error:', e); }}
}})()"#,
            sid = session_id, b64 = b64,
        ),
        "candidate" => format!(
            r#"(async function() {{
    try {{
        var sess = window.__epocaMediaSessions && window.__epocaMediaSessions[{sid}];
        if (!sess || !sess.pc) return;
        var cand = JSON.parse(atob('{b64}'));
        await sess.pc.addIceCandidate(cand);
    }} catch(e) {{ console.warn('epoca: add ICE candidate error:', e); }}
}})()"#,
            sid = session_id, b64 = b64,
        ),
        _ => String::new(),
    }
}

/// JS to close a session's RTCPeerConnection.
pub fn close_session_js(session_id: u64) -> String {
    format!(
        r#"(function() {{
    var sess = window.__epocaMediaSessions && window.__epocaMediaSessions[{sid}];
    if (sess) {{
        if (sess.offerRetry) {{ clearInterval(sess.offerRetry); sess.offerRetry = null; }}
        if (sess.pc) {{ try {{ sess.pc.close(); }} catch(e) {{}} }}
        delete window.__epocaMediaSessions[{sid}];
    }}
}})()"#,
        sid = session_id,
    )
}

// ---------------------------------------------------------------------------
// Incoming call ring (auto-accept)
// ---------------------------------------------------------------------------

/// Ring channel name for incoming calls to a specific peer.
fn ring_channel(app_id: &str, peer_id: &str) -> String {
    format!("{app_id}-media-ring-{peer_id}")
}

/// Publish a "ring" to a remote peer so they auto-create a session.
pub fn publish_ring(
    app_id: &str,
    local_peer_id: &str,
    remote_peer_id: &str,
) -> Result<(), String> {
    let ch = ring_channel(app_id, remote_peer_id);
    let payload = serde_json::json!({
        "type": "ring",
        "from": local_peer_id,
    });
    log::info!("[media] publishing ring to {remote_peer_id} on channel {ch}");
    crate::statements_api::write(app_id, local_peer_id, &ch, &payload.to_string())
        .map_err(|e| format!("publish ring: {e}"))
}

/// Start listening for incoming calls on this peer's ring channel.
/// Only starts once per webview. When a ring arrives, stores the pending call
/// and notifies JS — actual session creation is deferred until the user accepts.
pub fn start_ring_listener(
    app_id: &str,
    local_peer_id: &str,
    webview_ptr: usize,
) -> Result<(), String> {
    // Only start once per webview.
    {
        let mut st = state().lock().unwrap();
        if !st.ring_listeners.insert(webview_ptr) {
            return Ok(()); // already listening
        }
    }

    let ch = ring_channel(app_id, local_peer_id);
    let (sub_id, ring_rx) = crate::statements_api::subscribe_direct(app_id, &ch)
        .map_err(|e| format!("subscribe ring: {e}"))?;

    let app_id = app_id.to_string();
    let local_id = local_peer_id.to_string();

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();
    {
        let mut st = state().lock().unwrap();
        st.ring_listener_flags.insert(webview_ptr, running);
    }

    std::thread::spawn(move || {
        log::info!("[media] ring listener started on {ch}");
        while running_clone.load(Ordering::Acquire) {
            match ring_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(stmt) => {
                    let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if json.get("type").and_then(|v| v.as_str()) != Some("ring") {
                        continue;
                    }
                    let caller_id = match json.get("from").and_then(|v| v.as_str()) {
                        Some(id) => id.to_string(),
                        None => continue,
                    };
                    // Don't ring ourselves.
                    if caller_id == local_id {
                        continue;
                    }
                    log::info!("[media] incoming call from {caller_id}");

                    // Defer session creation until the user accepts — store pending call.
                    // Subscribe to the signaling channel NOW so the caller's offer
                    // buffers in the mpsc channel until accept time.
                    let listen_ch = signal_channel(&app_id, &caller_id, &local_id);
                    let pre_sub = match crate::statements_api::subscribe_direct(&app_id, &listen_ch) {
                        Ok((sub_id, rx)) => {
                            log::info!("[media] pre-subscribed signaling on {listen_ch}");
                            Some((sub_id, rx))
                        }
                        Err(e) => {
                            log::warn!("[media] failed to pre-subscribe signaling: {e}");
                            None
                        }
                    };
                    {
                        let mut st = state().lock().unwrap();
                        st.pending_incoming.insert(webview_ptr, PendingIncomingCall {
                            caller_id: caller_id.clone(),
                            app_id: app_id.clone(),
                            local_peer_id: local_id.clone(),
                            signal_sub: pre_sub,
                        });
                    }

                    // Notify JS about the incoming call (no session_id yet).
                    let safe_caller = caller_id
                        .replace('\\', "\\\\")
                        .replace('\'', "\\'");
                    let notify_js = format!(
                        "window.__epocaPush('mediaIncomingCall', {{peer: '{safe_caller}'}})",
                    );
                    push_eval_js(webview_ptr, notify_js);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        crate::statements_api::unsubscribe(sub_id);
        log::info!("[media] ring listener stopped on {ch}");
    });

    log::info!("[media] ring listener registered for {}", local_peer_id);
    Ok(())
}

/// Accept a pending incoming call: create session, start signaling, return IDs.
/// Returns buffered signal JS strings that must be evaluated AFTER setup_session_js.
pub fn accept_incoming_call(
    webview_ptr: usize,
) -> Result<(u64, String, String, Vec<u64>, String, Vec<String>), String> {
    let pending = {
        let mut st = state().lock().unwrap();
        st.pending_incoming.remove(&webview_ptr)
            .ok_or("no pending incoming call")?
    };

    let track_ids = {
        let st = state().lock().unwrap();
        st.local_tracks.get(&webview_ptr).cloned().unwrap_or_default()
    };

    let session_id = create_session(
        webview_ptr, &pending.app_id, &pending.caller_id,
        track_ids.clone(), &pending.local_peer_id,
    );

    let mut buffered_js = Vec::new();

    if let Some((sub_id, signal_rx)) = pending.signal_sub {
        // Drain signals that arrived between ring and accept.
        loop {
            match signal_rx.try_recv() {
                Ok(stmt) => {
                    if let Some(js) = signal_statement_to_js(session_id, &stmt) {
                        buffered_js.push(js);
                    }
                }
                Err(_) => break,
            }
        }
        log::info!("[media] drained {} buffered signals for session {session_id}", buffered_js.len());

        // Start live signaling thread with the same receiver (no gap).
        let handle = start_signaling_with_rx(session_id, sub_id, signal_rx, webview_ptr);
        set_signaling_handle(session_id, handle);
    } else {
        // Fallback: no pre-subscription, start fresh (may miss early signals).
        match start_signaling(session_id, &pending.app_id, &pending.caller_id, &pending.local_peer_id) {
            Ok(handle) => set_signaling_handle(session_id, handle),
            Err(e) => {
                close_session(session_id, &format!("signaling failed: {e}"));
                return Err(format!("signaling failed: {e}"));
            }
        }
    }

    Ok((session_id, pending.caller_id, pending.app_id, track_ids, pending.local_peer_id, buffered_js))
}

// ---------------------------------------------------------------------------
// Event drain & cleanup
// ---------------------------------------------------------------------------

/// Drain pending media events for the workbench render loop.
pub fn drain_events() -> Vec<MediaEvent> {
    let mut st = state().lock().unwrap();
    std::mem::take(&mut st.pending_events)
}

/// Clean up all media state for a closed webview.
pub fn cleanup_for_webview(webview_ptr: usize) {
    let mut st = state().lock().unwrap();

    // Stop signaling threads for sessions on this webview.
    for session in st.sessions.values() {
        if session.webview_ptr == webview_ptr {
            if let Some(running) = &session.signaling_running {
                running.store(false, Ordering::Release);
            }
        }
    }

    // Stop ring listener thread for this webview.
    if let Some(flag) = st.ring_listener_flags.remove(&webview_ptr) {
        flag.store(false, Ordering::Release);
    }

    st.tracks.retain(|_, t| t.webview_ptr != webview_ptr);
    st.sessions.retain(|_, s| s.webview_ptr != webview_ptr);
    st.pending_events.retain(|e| e.webview_ptr != webview_ptr);
    st.local_tracks.remove(&webview_ptr);
    st.ring_listeners.remove(&webview_ptr);
    // Unsubscribe pre-subscribed signaling channel if pending call was never accepted.
    if let Some(pending) = st.pending_incoming.remove(&webview_ptr) {
        if let Some((sub_id, _rx)) = pending.signal_sub {
            crate::statements_api::unsubscribe(sub_id);
        }
    }
}

/// Returns JS to stop all tracks belonging to a webview.
pub fn cleanup_tracks_js(webview_ptr: usize) -> Option<String> {
    let st = state().lock().unwrap();
    let track_ids: Vec<u64> = st.tracks.iter()
        .filter(|(_, t)| t.webview_ptr == webview_ptr)
        .map(|(id, _)| *id)
        .collect();

    if track_ids.is_empty() {
        return None;
    }

    let ids_json = track_ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",");

    Some(format!(
        r#"(function() {{
    var ids = [{ids}];
    for (var i = 0; i < ids.length; i++) {{
        var entry = window.__epocaMediaTracks && window.__epocaMediaTracks[ids[i]];
        if (entry && entry.track) {{ entry.track.stop(); }}
        if (window.__epocaMediaTracks) delete window.__epocaMediaTracks[ids[i]];
    }}
}})()"#,
        ids = ids_json,
    ))
}
