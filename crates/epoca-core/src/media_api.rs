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
}

#[derive(Debug, Clone, PartialEq)]
enum SessionState {
    Signaling,
    Connected,
    #[allow(dead_code)]
    Closed,
}

struct MediaState {
    tracks: HashMap<u64, MediaTrack>,
    sessions: HashMap<u64, MediaSession>,
    pending_events: Vec<MediaEvent>,
}

static STATE: OnceLock<Mutex<MediaState>> = OnceLock::new();

fn state() -> &'static Mutex<MediaState> {
    STATE.get_or_init(|| {
        Mutex::new(MediaState {
            tracks: HashMap::new(),
            sessions: HashMap::new(),
            pending_events: Vec::new(),
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
pub fn is_offerer(peer_address: &str) -> bool {
    local_peer_id() < peer_address.to_string()
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
        var stream = await navigator.mediaDevices.getUserMedia({{ audio: {audio_bool}, video: {video_bool} }});
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

// ---------------------------------------------------------------------------
// Session lifecycle (MediaConnect)
// ---------------------------------------------------------------------------

/// Create a media session. Returns session_id.
pub fn create_session(
    webview_ptr: usize,
    app_id: &str,
    peer_address: &str,
    track_ids: Vec<u64>,
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

/// Start a signaling relay thread for a media session.
/// Subscribes to the peer's signaling channel and pushes received signals
/// as EvalJs events for the workbench to apply on the webview.
pub fn start_signaling(
    session_id: u64,
    app_id: &str,
    peer_address: &str,
) -> Result<Arc<AtomicBool>, String> {
    let local_id = local_peer_id();
    let listen_ch = signal_channel(app_id, peer_address, &local_id);
    let (sub_id, signal_rx) = crate::statements_api::subscribe_direct(app_id, &listen_ch)
        .map_err(|e| format!("subscribe signaling: {e}"))?;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    // Get webview_ptr from the session for pushing eval events.
    let webview_ptr = {
        let st = state().lock().unwrap();
        st.sessions.get(&session_id)
            .map(|s| s.webview_ptr)
            .ok_or("session not found")?
    };

    std::thread::spawn(move || {
        log::info!("[media] signaling thread for session={session_id} listening on {listen_ch}");
        while running_clone.load(Ordering::Acquire) {
            match signal_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(stmt) => {
                    let json: serde_json::Value = match serde_json::from_str(&stmt.data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    let sig_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let sig_data = json.get("data").and_then(|v| v.as_str()).unwrap_or("");
                    if sig_type.is_empty() {
                        continue;
                    }
                    // Generate JS to apply the signal and push as EvalJs event.
                    let js = apply_signal_js(session_id, sig_type, sig_data);
                    if !js.is_empty() {
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

    Ok(running)
}

/// Publish a signal to the remote peer via Statement Store.
pub fn publish_signal(
    session_id: u64,
    signal_type: &str,
    data: &str,
) -> Result<(), String> {
    let (app_id, peer_address) = {
        let st = state().lock().unwrap();
        let session = st.sessions.get(&session_id)
            .ok_or_else(|| format!("session {session_id} not found"))?;
        (session.app_id.clone(), session.peer_address.clone())
    };

    let local_id = local_peer_id();
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

/// JS snippet that obtains native WebRTC constructors from a temporary iframe.
/// The init script removes them from the main window, so the host retrieves
/// fresh copies from a blank iframe whose contentWindow is untouched.
/// The iframe is destroyed immediately after capture.
const GET_RTC_JS: &str = r#"
        var _f = document.createElement('iframe');
        _f.style.display = 'none';
        document.documentElement.appendChild(_f);
        var _RTC = _f.contentWindow.RTCPeerConnection;
        var _Desc = _f.contentWindow.RTCSessionDescription;
        var _ICE = _f.contentWindow.RTCIceCandidate;
        _f.remove();
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

        pc.onicecandidate = function(e) {{
            if (e.candidate) {{
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
            var rtid = {sid} * 1000 + sess.remoteTrackCount;
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
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'connected', data: '' }}
                }});
            }} else if (pc.connectionState === 'failed' || pc.connectionState === 'closed') {{
                window.webkit.messageHandlers.epocaHost.postMessage({{
                    id: 0, method: 'mediaSignal',
                    params: {{ sessionId: {sid}, type: 'closed', data: pc.connectionState }}
                }});
            }}
        }};

        if ({offerer}) {{
            var offer = await pc.createOffer();
            await pc.setLocalDescription(offer);
            window.webkit.messageHandlers.epocaHost.postMessage({{
                id: 0, method: 'mediaSignal',
                params: {{ sessionId: {sid}, type: 'offer', data: JSON.stringify(offer) }}
            }});
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
        var offer = JSON.parse(atob('{b64}'));
        await sess.pc.setRemoteDescription(offer);
        var answer = await sess.pc.createAnswer();
        await sess.pc.setLocalDescription(answer);
        window.webkit.messageHandlers.epocaHost.postMessage({{
            id: 0, method: 'mediaSignal',
            params: {{ sessionId: {sid}, type: 'answer', data: JSON.stringify(answer) }}
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
    if (sess && sess.pc) {{
        try {{ sess.pc.close(); }} catch(e) {{}}
        delete window.__epocaMediaSessions[{sid}];
    }}
}})()"#,
        sid = session_id,
    )
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

    st.tracks.retain(|_, t| t.webview_ptr != webview_ptr);
    st.sessions.retain(|_, s| s.webview_ptr != webview_ptr);
    st.pending_events.retain(|e| e.webview_ptr != webview_ptr);
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
