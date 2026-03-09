//! Statement Store client — submits and polls statements via Substrate RPC.
//!
//! Implements the `sp-statement-store` binary encoding format and connects
//! via WebSocket to a Substrate node with the statement store pallet.
//!
//! Used for cross-host pub/sub: SPA apps call `window.epoca.statements.write()`
//! and the host submits the statement to the chain for gossip to other nodes.
//!
//! The statement store is a last-write-wins channel system:
//! - Each statement has a `decryption_key` (room/document ID) and optional `channel`
//! - Topics are used for filtering (e.g. "ss-epoca", "presence", "offer", "answer")
//! - Priority field determines which statement wins on conflict

use blake2::{Blake2b, Digest, digest::consts::U32};
type Blake2b256 = Blake2b<U32>;
use schnorrkel::Keypair;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

/// Topic = 32-byte blake2b hash.
pub type Topic = [u8; 32];

/// WebSocket endpoints with statement store pallet.
const SS_ENDPOINTS: &[&str] = &[
    "wss://pop3-testnet.parity-lab.parity.io/people",
];

/// Hash a string into a 32-byte Topic (blake2b-256).
pub fn string_to_topic(s: &str) -> Topic {
    let mut hasher = Blake2b256::new();
    hasher.update(s.as_bytes());
    let result = hasher.finalize();
    let mut topic = [0u8; 32];
    topic.copy_from_slice(&result);
    topic
}

// ---------------------------------------------------------------------------
// Statement binary encoding (sp-statement-store compatible)
//
// Substrate format: Compact<u32> field count, then tagged fields in order.
// Field tags: 0=Proof, 1=DecryptionKey, 2=Expiry, 3=Channel, 4-7=Topics, 8=Data
// Each field: u8 tag + SCALE-encoded payload.
// Proof::Sr25519 = tag 0, variant 0, sig[64], signer[32].
// Data = tag 8, Compact<u32> length, bytes.
// Signing payload: same fields without Compact prefix and without Proof field.
// ---------------------------------------------------------------------------

/// A decoded statement from the statement store.
#[derive(Debug, Clone)]
pub struct Statement {
    pub proof_pubkey: Option<[u8; 32]>,
    pub decryption_key: Option<Topic>,
    pub channel: Option<Topic>,
    pub priority: u32,
    pub topics: Vec<Topic>,
    pub data: Vec<u8>,
}

/// Encode SCALE Compact<u32>.
fn encode_compact_u32(val: u32) -> Vec<u8> {
    if val < 0x40 {
        vec![(val as u8) << 2]
    } else if val < 0x4000 {
        let v = (val << 2) | 0x01;
        vec![v as u8, (v >> 8) as u8]
    } else if val < 0x4000_0000 {
        let v = (val << 2) | 0x02;
        v.to_le_bytes().to_vec()
    } else {
        let mut out = vec![0x03];
        out.extend_from_slice(&val.to_le_bytes());
        out
    }
}

/// Decode SCALE Compact<u32>, returns (value, bytes_consumed).
fn decode_compact_u32(data: &[u8]) -> Result<(u32, usize), String> {
    if data.is_empty() {
        return Err("compact: empty".into());
    }
    let mode = data[0] & 0x03;
    match mode {
        0 => Ok(((data[0] >> 2) as u32, 1)),
        1 => {
            if data.len() < 2 {
                return Err("compact: truncated 2-byte".into());
            }
            let v = u16::from_le_bytes([data[0], data[1]]) >> 2;
            Ok((v as u32, 2))
        }
        2 => {
            if data.len() < 4 {
                return Err("compact: truncated 4-byte".into());
            }
            let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) >> 2;
            Ok((v, 4))
        }
        3 => {
            if data.len() < 5 {
                return Err("compact: truncated big".into());
            }
            let v = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
            Ok((v, 5))
        }
        _ => unreachable!(),
    }
}

/// Encode a statement into the sp-statement-store SCALE binary format.
///
/// Format: Compact<u32> field_count, then each field as (u8 tag, SCALE payload).
/// The signature is over blake2b_256 of the signing payload (fields without
/// the Compact prefix and without the Proof field).
pub fn encode_statement(
    decryption_key: Option<&Topic>,
    channel: Option<&Topic>,
    priority: u32,
    topics: &[Topic],
    data: &[u8],
    sr25519_pubkey: &[u8; 32],
    sr25519_sign: &dyn Fn(&[u8]) -> [u8; 64],
) -> Vec<u8> {
    assert!(topics.len() <= 4, "max 4 topics");

    // Expiry: upper 32 bits = timestamp (seconds), lower 32 bits = priority.
    // Use a timestamp far in the future to avoid "alreadyExpired".
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiry_ts = (now_secs + 3600) as u32; // 1 hour from now
    let expiry: u64 = ((expiry_ts as u64) << 32) | (priority as u64);

    // Count fields (excluding proof, which is added separately).
    let mut num_fields: u32 = 1; // proof always present
    if decryption_key.is_some() {
        num_fields += 1;
    }
    num_fields += 1; // expiry always present
    if channel.is_some() {
        num_fields += 1;
    }
    num_fields += topics.len() as u32;
    if !data.is_empty() {
        num_fields += 1;
    }

    // Build signing payload (fields without Compact prefix and without Proof).
    let mut sign_payload = Vec::new();
    if let Some(dk) = decryption_key {
        sign_payload.push(1u8); // tag: DecryptionKey
        sign_payload.extend_from_slice(dk);
    }
    sign_payload.push(2u8); // tag: Expiry
    sign_payload.extend_from_slice(&expiry.to_le_bytes());
    if let Some(ch) = channel {
        sign_payload.push(3u8); // tag: Channel
        sign_payload.extend_from_slice(ch);
    }
    for (i, t) in topics.iter().enumerate() {
        sign_payload.push(4u8 + i as u8); // tag: Topic1..4
        sign_payload.extend_from_slice(t);
    }
    if !data.is_empty() {
        sign_payload.push(8u8); // tag: Data
        sign_payload.extend_from_slice(&encode_compact_u32(data.len() as u32));
        sign_payload.extend_from_slice(data);
    }

    // Sign the raw signing payload (substrate verifies against raw bytes, not hash).
    let signature = sr25519_sign(&sign_payload);

    // Assemble full encoded statement.
    let mut out = Vec::new();

    // Compact<u32> field count prefix.
    out.extend_from_slice(&encode_compact_u32(num_fields));

    // Field 0: AuthenticityProof (Proof::Sr25519 = variant 0)
    out.push(0u8); // tag: AuthenticityProof
    out.push(0u8); // Proof variant 0 = Sr25519
    out.extend_from_slice(&signature); // signature first in Sr25519 struct
    out.extend_from_slice(sr25519_pubkey); // then signer

    // Remaining fields (same as sign_payload).
    out.extend_from_slice(&sign_payload);

    out
}

/// Decode a statement from SCALE binary encoding (sp-statement-store format).
pub fn decode_statement(encoded: &[u8]) -> Result<Statement, String> {
    if encoded.is_empty() {
        return Err("empty statement".into());
    }

    let (num_fields, mut pos) = decode_compact_u32(encoded)?;

    let mut proof_pubkey = None;
    let mut decryption_key = None;
    let mut channel = None;
    let mut priority = 0u32;
    let mut topics = Vec::new();
    let mut data = Vec::new();

    for _ in 0..num_fields {
        if pos >= encoded.len() {
            return Err("truncated field tag".into());
        }
        let tag = encoded[pos];
        pos += 1;

        match tag {
            0 => {
                // AuthenticityProof
                if pos >= encoded.len() {
                    return Err("truncated proof variant".into());
                }
                let variant = encoded[pos];
                pos += 1;
                match variant {
                    0 | 1 => {
                        // Sr25519 or Ed25519: sig[64] + signer[32]
                        if pos + 96 > encoded.len() {
                            return Err("truncated proof".into());
                        }
                        let mut pk = [0u8; 32];
                        pk.copy_from_slice(&encoded[pos + 64..pos + 96]);
                        proof_pubkey = Some(pk);
                        pos += 96;
                    }
                    2 => {
                        // Secp256k1: sig[65] + signer[33]
                        if pos + 98 > encoded.len() {
                            return Err("truncated secp proof".into());
                        }
                        pos += 98;
                    }
                    3 => {
                        // OnChain: who[32] + block_hash[32] + u64
                        if pos + 72 > encoded.len() {
                            return Err("truncated onchain proof".into());
                        }
                        let mut pk = [0u8; 32];
                        pk.copy_from_slice(&encoded[pos..pos + 32]);
                        proof_pubkey = Some(pk);
                        pos += 72;
                    }
                    _ => return Err(format!("unknown proof variant: {variant}")),
                }
            }
            1 => {
                // DecryptionKey [32]
                if pos + 32 > encoded.len() {
                    return Err("truncated decryption_key".into());
                }
                let mut dk = [0u8; 32];
                dk.copy_from_slice(&encoded[pos..pos + 32]);
                decryption_key = Some(dk);
                pos += 32;
            }
            2 => {
                // Expiry u64 — upper 32 = timestamp, lower 32 = priority
                if pos + 8 > encoded.len() {
                    return Err("truncated expiry".into());
                }
                let expiry = u64::from_le_bytes([
                    encoded[pos],
                    encoded[pos + 1],
                    encoded[pos + 2],
                    encoded[pos + 3],
                    encoded[pos + 4],
                    encoded[pos + 5],
                    encoded[pos + 6],
                    encoded[pos + 7],
                ]);
                priority = expiry as u32; // lower 32 bits
                pos += 8;
            }
            3 => {
                // Channel [32]
                if pos + 32 > encoded.len() {
                    return Err("truncated channel".into());
                }
                let mut ch = [0u8; 32];
                ch.copy_from_slice(&encoded[pos..pos + 32]);
                channel = Some(ch);
                pos += 32;
            }
            4..=7 => {
                // Topic [32]
                if pos + 32 > encoded.len() {
                    return Err("truncated topic".into());
                }
                let mut t = [0u8; 32];
                t.copy_from_slice(&encoded[pos..pos + 32]);
                topics.push(t);
                pos += 32;
            }
            8 => {
                // Data: Compact<u32> length + bytes
                let (data_len, consumed) =
                    decode_compact_u32(&encoded[pos..]).map_err(|e| format!("data len: {e}"))?;
                pos += consumed;
                let data_len = data_len as usize;
                if pos + data_len > encoded.len() {
                    return Err("truncated data".into());
                }
                data = encoded[pos..pos + data_len].to_vec();
                pos += data_len;
            }
            _ => {
                // Unknown field — can't decode further without knowing size.
                return Err(format!("unknown field tag: {tag}"));
            }
        }
    }

    Ok(Statement {
        proof_pubkey,
        decryption_key,
        channel,
        priority,
        topics,
        data,
    })
}

fn blake2b_256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Blake2b256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        out.push(u8::from_str_radix(&s[i..i + 2], 16).ok()?);
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// RPC client — WebSocket to Substrate node
// ---------------------------------------------------------------------------

/// Submit a signed statement to the statement store via RPC.
pub fn rpc_submit(encoded_statement: &[u8]) -> Result<(), String> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "statement_submit",
        "params": [hex_encode(encoded_statement)]
    });
    let payload_str = payload.to_string();

    for endpoint in SS_ENDPOINTS {
        log::info!("[ss] submitting statement to {endpoint}");
        match ws_request(endpoint, &payload_str) {
            Ok(resp) => {
                let body: serde_json::Value = match serde_json::from_str(&resp) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("[ss] failed to parse response from {endpoint}: {e}");
                        continue;
                    }
                };
                if let Some(err) = body.get("error") {
                    log::warn!("[ss] RPC error from {endpoint}: {err}");
                    continue;
                }
                let result = body.get("result");
                log::info!("[ss] statement submitted: {result:?}");
                // Check for rejection statuses returned as string result.
                if let Some(status) = result.and_then(|v| v.as_str()) {
                    match status {
                        "ok" => return Ok(()),
                        other => return Err(format!("statement rejected: {other}")),
                    }
                }
                return Ok(());
            }
            Err(e) => {
                log::warn!("[ss] WS error for {endpoint}: {e}");
                continue;
            }
        }
    }
    Err("all statement store endpoints failed".into())
}

/// Fetch broadcast statements matching the given topics.
/// Returns decoded statements.
pub fn rpc_get_broadcasts(topics: &[Topic]) -> Result<Vec<Statement>, String> {
    let topic_hexes: Vec<String> = topics.iter().map(|t| hex_encode(t)).collect();
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "statement_broadcastsStatement",
        "params": [topic_hexes]
    });
    let payload_str = payload.to_string();

    for endpoint in SS_ENDPOINTS {
        match ws_request(endpoint, &payload_str) {
            Ok(resp) => {
                let body: serde_json::Value = match serde_json::from_str(&resp) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("[ss] failed to parse response from {endpoint}: {e}");
                        continue;
                    }
                };
                if let Some(err) = body.get("error") {
                    log::warn!("[ss] RPC error from {endpoint}: {err}");
                    continue;
                }
                // Result is an array of [hex_hash, hex_encoded_statement] pairs.
                let result = match body.get("result").and_then(|v| v.as_array()) {
                    Some(r) => r,
                    None => {
                        log::warn!("[ss] unexpected response format from {endpoint}");
                        continue;
                    }
                };

                let mut statements = Vec::new();
                for item in result {
                    // Each item is [hash_hex, encoded_hex].
                    let encoded_hex = if let Some(arr) = item.as_array() {
                        arr.get(1).and_then(|v| v.as_str())
                    } else {
                        item.as_str()
                    };
                    if let Some(hex_str) = encoded_hex {
                        if let Some(bytes) = hex_decode(hex_str) {
                            match decode_statement(&bytes) {
                                Ok(stmt) => statements.push(stmt),
                                Err(e) => {
                                    log::warn!("[ss] failed to decode statement: {e}");
                                }
                            }
                        }
                    }
                }

                log::info!("[ss] fetched {} broadcasts", statements.len());
                return Ok(statements);
            }
            Err(e) => {
                log::warn!("[ss] WS error for {endpoint}: {e}");
                continue;
            }
        }
    }
    Err("all statement store endpoints failed".into())
}

/// Send a single JSON-RPC request over WebSocket and return the response.
fn ws_request(endpoint: &str, payload: &str) -> Result<String, String> {
    use std::net::TcpStream;
    use std::time::Duration;
    use tungstenite::{client::IntoClientRequest, Message};

    let request = endpoint
        .into_client_request()
        .map_err(|e| format!("bad WS URL: {e}"))?;
    let host = request
        .uri()
        .host()
        .ok_or("no host in endpoint")?
        .to_string();
    let port = request.uri().port_u16().unwrap_or(443);

    // Resolve hostname to IP, then connect with timeout.
    use std::net::ToSocketAddrs;
    let addr = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolve: {e}"))?
        .next()
        .ok_or_else(|| format!("DNS resolve: no addresses for {host}"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("TCP connect failed: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let (mut ws, _) = tungstenite::client_tls(request, tcp)
        .map_err(|e| format!("WS handshake failed: {e}"))?;

    ws.send(Message::Text(payload.to_string()))
        .map_err(|e| format!("WS send failed: {e}"))?;

    // Read until we get a text message back.
    loop {
        match ws.read() {
            Ok(Message::Text(text)) => {
                let _ = ws.close(None);
                return Ok(text.to_string());
            }
            Ok(Message::Ping(data)) => {
                let _ = ws.send(Message::Pong(data));
            }
            Ok(Message::Close(_)) => {
                return Err("connection closed".into());
            }
            Ok(_) => continue,
            Err(e) => return Err(format!("WS read failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Statement Store client — manages connection and signing
// ---------------------------------------------------------------------------

/// Substrate signing context (matches runtime verifier).
const SIGNING_CTX: &[u8] = b"substrate";

/// Global statement store client state.
struct StoreState {
    /// Ephemeral sr25519 keypair for signing statements.
    keypair: Keypair,
    /// Background poll thread running flag.
    running: Arc<AtomicBool>,
}

static STORE: OnceLock<Mutex<Option<StoreState>>> = OnceLock::new();

fn store() -> &'static Mutex<Option<StoreState>> {
    STORE.get_or_init(|| Mutex::new(None))
}

/// Callback type for received statements.
pub type StatementCallback = Box<dyn Fn(Statement) + Send + 'static>;

/// Global callback for received statements (set by epoca-core statements_api).
static ON_STATEMENT: OnceLock<Mutex<Option<StatementCallback>>> = OnceLock::new();

fn on_statement() -> &'static Mutex<Option<StatementCallback>> {
    ON_STATEMENT.get_or_init(|| Mutex::new(None))
}

/// Set the callback invoked when a new statement is received from the network.
pub fn set_on_statement(cb: StatementCallback) {
    *on_statement().lock().unwrap() = Some(cb);
}

/// App-level namespace topic.
const EPOCA_TOPIC: &str = "ss-epoca";

/// Initialize the statement store with an ephemeral sr25519 keypair.
///
/// Generates a random keypair for signing gossip messages. This identity
/// is independent of the wallet and persists for the app's lifetime.
/// Call once at startup.
pub fn init() {
    let keypair = Keypair::generate();
    let pubkey = keypair.public.to_bytes();
    let running = Arc::new(AtomicBool::new(true));
    let running2 = running.clone();

    {
        let mut st = store().lock().unwrap();
        // Stop existing poll thread if any.
        if let Some(old) = st.as_ref() {
            old.running.store(false, Ordering::Relaxed);
        }
        *st = Some(StoreState {
            keypair,
            running: running.clone(),
        });
    }

    // Start background poll thread.
    std::thread::spawn(move || {
        poll_loop(running2);
    });

    log::info!(
        "[ss] statement store initialized (ephemeral pubkey={}...)",
        hex_encode(&pubkey[..4])
    );
}

/// Shut down the statement store client.
pub fn shutdown() {
    let mut st = store().lock().unwrap();
    if let Some(s) = st.as_ref() {
        s.running.store(false, Ordering::Relaxed);
    }
    *st = None;
}

/// Quick connectivity check — tries to reach any statement store endpoint.
/// Returns Ok(endpoint) on success, Err(message) if all endpoints are unreachable.
pub fn ping() -> Result<String, String> {
    use std::net::ToSocketAddrs;
    use std::net::TcpStream;
    use std::time::Duration;
    use tungstenite::client::IntoClientRequest;

    let mut last_err = String::new();

    for endpoint in SS_ENDPOINTS {
        let request = match endpoint.into_client_request() {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("{endpoint}: bad URL: {e}");
                log::warn!("[ss] ping: {last_err}");
                continue;
            }
        };
        let host = match request.uri().host() {
            Some(h) => h.to_string(),
            None => continue,
        };
        let port = request.uri().port_u16().unwrap_or(443);

        // Resolve hostname to IP, then TCP connect with timeout
        let addr = match format!("{host}:{port}").to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => {
                    last_err = format!("{endpoint}: DNS returned no addresses");
                    log::warn!("[ss] ping: {last_err}");
                    continue;
                }
            },
            Err(e) => {
                last_err = format!("{endpoint}: DNS failed: {e}");
                log::warn!("[ss] ping: {last_err}");
                continue;
            }
        };

        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(_) => {
                log::info!("[ss] ping: {endpoint} reachable");
                return Ok(endpoint.to_string());
            }
            Err(e) => {
                last_err = format!("{endpoint}: {e}");
                log::warn!("[ss] ping: {last_err}");
                continue;
            }
        }
    }

    Err(format!("statement store unreachable: {last_err}"))
}

/// Statement store status for the settings status panel.
#[derive(Debug, Clone)]
pub enum StoreStatus {
    /// Not initialized yet.
    Offline,
    /// Running with the given ephemeral public key (hex-encoded first 8 bytes).
    Running { pubkey_short: String },
}

/// Query current statement store status.
pub fn status() -> StoreStatus {
    let st = store().lock().unwrap();
    match st.as_ref() {
        None => StoreStatus::Offline,
        Some(state) => {
            let pk = state.keypair.public.to_bytes();
            StoreStatus::Running {
                pubkey_short: hex_encode(&pk[..8]),
            }
        }
    }
}

/// Return the full hex-encoded public key of the ephemeral keypair.
/// Returns `None` if the store is not initialized.
pub fn public_key_hex() -> Option<String> {
    let st = store().lock().unwrap();
    st.as_ref().map(|s| hex_encode(&s.keypair.public.to_bytes()))
}

/// Submit a statement to the network.
///
/// `app_id` is used as the decryption_key namespace.
/// `channel` is hashed into a Topic.
/// `data` is the JSON payload bytes.
pub fn submit(
    app_id: &str,
    channel: &str,
    data: &[u8],
    priority: u32,
) -> Result<(), String> {
    let encoded = {
        let st = store().lock().unwrap();
        let state = st.as_ref().ok_or("statement store not initialized")?;

        let dk = string_to_topic(app_id);
        let ch = string_to_topic(channel);
        let epoca_topic = string_to_topic(EPOCA_TOPIC);

        let pubkey = state.keypair.public.to_bytes();
        let sign_fn = |hash: &[u8]| -> [u8; 64] {
            let ctx = schnorrkel::signing_context(SIGNING_CTX);
            state.keypair.sign(ctx.bytes(hash)).to_bytes()
        };

        encode_statement(
            Some(&dk),
            Some(&ch),
            priority,
            &[epoca_topic],
            data,
            &pubkey,
            &sign_fn,
        )
    };

    rpc_submit(&encoded)
}

/// Maximum dedup cache size — evict oldest half when exceeded.
const MAX_SEEN: usize = 8192;

/// Background subscription loop — subscribes to statement_subscribeStatement
/// and processes incoming notifications. Reconnects on failure.
fn poll_loop(running: Arc<AtomicBool>) {
    // Dedup key = hash(decryption_key || channel || data). Value = priority.
    let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
    let mut seen_order: Vec<[u8; 32]> = Vec::new();

    while running.load(Ordering::Relaxed) {
        match subscribe_loop(&running, &mut seen, &mut seen_order) {
            Ok(()) => break, // clean shutdown
            Err(e) => {
                log::warn!("[ss] subscription error: {e}, reconnecting in 3s...");
                // Wait before reconnecting.
                for _ in 0..6 {
                    if !running.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }

    log::info!("[ss] poll loop stopped");
}

/// Open a persistent WebSocket, subscribe with topic filter "any",
/// and deliver incoming statements via the callback.
fn subscribe_loop(
    running: &Arc<AtomicBool>,
    seen: &mut HashMap<[u8; 32], u32>,
    seen_order: &mut Vec<[u8; 32]>,
) -> Result<(), String> {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    use tungstenite::{client::IntoClientRequest, Message};

    // Connect to first working endpoint.
    let endpoint = SS_ENDPOINTS[0];
    let request = endpoint
        .into_client_request()
        .map_err(|e| format!("bad WS URL: {e}"))?;
    let host = request
        .uri()
        .host()
        .ok_or("no host in endpoint")?
        .to_string();
    let port = request.uri().port_u16().unwrap_or(443);
    let addr = format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| format!("DNS resolve: {e}"))?
        .next()
        .ok_or_else(|| format!("DNS: no addresses for {host}"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|e| format!("TCP connect: {e}"))?;
    // Short read timeout to allow periodic `running` flag checks.
    // tungstenite preserves frame state across WouldBlock/TimedOut errors.
    tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let (mut ws, _) = tungstenite::client_tls(request, tcp)
        .map_err(|e| format!("WS handshake: {e}"))?;

    log::info!("[ss] connected to {endpoint}, subscribing...");

    // Send subscription request: statement_subscribeStatement with TopicFilter::Any
    let sub_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "statement_subscribeStatement",
        "params": ["any"]
    });
    ws.send(Message::Text(sub_req.to_string()))
        .map_err(|e| format!("send subscribe: {e}"))?;

    // Read subscription response to get sub_id (skip any Ping frames).
    let resp_text = loop {
        match ws.read().map_err(|e| format!("read subscribe response: {e}"))? {
            Message::Text(t) => break t.to_string(),
            Message::Ping(data) => {
                let _ = ws.send(Message::Pong(data));
                continue;
            }
            other => return Err(format!("unexpected response: {other:?}")),
        }
    };
    let resp_json: serde_json::Value =
        serde_json::from_str(&resp_text).map_err(|e| format!("parse response: {e}"))?;
    if let Some(err) = resp_json.get("error") {
        return Err(format!("subscribe RPC error: {err}"));
    }
    let sub_id = resp_json
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("no subscription id in response")?
        .to_string();
    log::info!("[ss] subscribed (id={sub_id})");

    // Read notifications until shutdown or error.
    let mut timeout_count: u32 = 0;
    while running.load(Ordering::Relaxed) {
        let msg = match ws.read() {
            Ok(Message::Text(t)) => {
                timeout_count = 0;
                t.to_string()
            }
            Ok(Message::Ping(data)) => {
                timeout_count = 0;
                log::debug!("[ss] got ping, sending pong");
                let _ = ws.send(Message::Pong(data));
                continue;
            }
            Ok(Message::Close(_)) => return Err("WS closed by server".into()),
            Ok(other) => {
                log::debug!("[ss] got non-text message: {other:?}");
                continue;
            }
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                timeout_count += 1;
                if timeout_count % 12 == 1 {
                    log::debug!("[ss] read timeout #{timeout_count}, still listening...");
                }
                continue;
            }
            Err(e) => return Err(format!("WS read: {e}")),
        };

        // Parse notification.
        let json: serde_json::Value = match serde_json::from_str(&msg) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[ss] non-JSON message: {e} — {}", &msg[..msg.len().min(200)]);
                continue;
            }
        };

        let method = json.get("method").and_then(|v| v.as_str()).unwrap_or("");
        log::info!(
            "[ss] notification: method={method}, msg_len={}",
            msg.len(),
        );

        // Try multiple JSON pointer paths — substrate serialization varies.
        let statements_hex = json
            .pointer("/params/result/data/statements")
            .and_then(|v| v.as_array())
            .or_else(|| {
                json.pointer("/params/result/newStatements/statements")
                    .and_then(|v| v.as_array())
            })
            .or_else(|| {
                json.pointer("/params/result/statements")
                    .and_then(|v| v.as_array())
            });
        let statements_hex = match statements_hex {
            Some(arr) => {
                log::info!("[ss] found {} statements in notification", arr.len());
                arr
            }
            None => {
                // Log the structure to help debug.
                if let Some(result) = json.pointer("/params/result") {
                    let preview = result.to_string();
                    log::info!(
                        "[ss] notification has /params/result but no statements found. Preview: {}",
                        &preview[..preview.len().min(500)]
                    );
                }
                continue;
            }
        };

        for hex_val in statements_hex {
            let hex_str = match hex_val.as_str() {
                Some(s) => s,
                None => continue,
            };
            let bytes = match hex_decode(hex_str) {
                Some(b) => b,
                None => continue,
            };
            let stmt = match decode_statement(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("[ss] decode statement: {e}");
                    continue;
                }
            };

            // Dedup.
            let dedup_input = [
                stmt.decryption_key.unwrap_or([0; 32]).as_slice(),
                stmt.channel.unwrap_or([0; 32]).as_slice(),
                &stmt.data,
            ]
            .concat();
            let hash = blake2b_256(&dedup_input);

            if let Some(&prev) = seen.get(&hash) {
                if stmt.priority <= prev {
                    continue;
                }
            }
            if !seen.contains_key(&hash) {
                seen_order.push(hash);
            }
            seen.insert(hash, stmt.priority);

            if seen.len() > MAX_SEEN {
                let half = seen_order.len() / 2;
                for key in seen_order.drain(..half) {
                    seen.remove(&key);
                }
            }

            // Deliver to callback.
            if let Ok(guard) = on_statement().lock() {
                if let Some(cb) = guard.as_ref() {
                    cb(stmt);
                }
            }
        }
    }

    // Clean shutdown — unsubscribe.
    let unsub = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "statement_unsubscribeStatement",
        "params": [sub_id]
    });
    let _ = ws.send(Message::Text(unsub.to_string()));
    let _ = ws.close(None);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_to_topic() {
        let t1 = string_to_topic("hello");
        let t2 = string_to_topic("hello");
        let t3 = string_to_topic("world");
        assert_eq!(t1, t2);
        assert_ne!(t1, t3);
        assert_eq!(t1.len(), 32);
    }

    #[test]
    fn test_encode_decode_roundtrip() {
        let dk = string_to_topic("test-room");
        let ch = string_to_topic("presence");
        let topics = vec![string_to_topic("ss-epoca")];
        let data = b"hello world";
        let pubkey = [0xAA; 32];
        let sign_fn = |_hash: &[u8]| -> [u8; 64] { [0xBB; 64] };

        let encoded = encode_statement(
            Some(&dk),
            Some(&ch),
            42,
            &topics,
            data,
            &pubkey,
            &sign_fn,
        );

        let decoded = decode_statement(&encoded).expect("decode failed");
        assert_eq!(decoded.proof_pubkey, Some(pubkey));
        assert_eq!(decoded.decryption_key, Some(dk));
        assert_eq!(decoded.channel, Some(ch));
        assert_eq!(decoded.priority, 42);
        assert_eq!(decoded.topics.len(), 1);
        assert_eq!(decoded.topics[0], topics[0]);
        assert_eq!(decoded.data, data);
    }

    #[test]
    fn test_encode_minimal() {
        let pubkey = [0x01; 32];
        let sign_fn = |_: &[u8]| -> [u8; 64] { [0x02; 64] };

        let encoded = encode_statement(None, None, 0, &[], &[], &pubkey, &sign_fn);
        let decoded = decode_statement(&encoded).expect("decode failed");
        assert_eq!(decoded.decryption_key, None);
        assert_eq!(decoded.channel, None);
        assert_eq!(decoded.priority, 0);
        assert!(decoded.topics.is_empty());
        assert!(decoded.data.is_empty());
    }

    /// Submit a real statement to previewnet for integration testing.
    /// Run with: cargo test -p epoca-chain -- --ignored test_submit_to_previewnet --nocapture
    #[test]
    #[ignore]
    fn test_submit_to_previewnet() {
        env_logger::init();

        // Target: the running app's incoming listener channel.
        let target_peer = std::env::var("TARGET_PEER")
            .unwrap_or_else(|_| "peer-0xa2ce6d3a925727-1".to_string());
        let app_id = "com.epoca.data-test";
        let channel = format!("{app_id}-offer-to-{target_peer}");

        // Build a fake offer JSON payload (matching what statements_api::write produces).
        let fake_offer = serde_json::json!({
            "app_id": app_id,
            "author": "test-script",
            "channel": channel,
            "data": "{\"type\":\"offer\",\"from\":\"test-script\",\"sdp\":\"v=0\\r\\n\"}",
            "timestamp_ms": 9999999999u64,
        });
        let payload_bytes = fake_offer.to_string().into_bytes();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        // Use Alice's dev account keypair (has allowance on previewnet).
        // Alice's mini-secret key is well-known for all Substrate dev chains.
        let alice_seed: [u8; 32] = [
            0xe5, 0xbe, 0x9a, 0x50, 0x92, 0xb8, 0x1b, 0xca,
            0x64, 0xbe, 0x81, 0xd2, 0x12, 0xe7, 0xf2, 0xf9,
            0xeb, 0xa1, 0x83, 0xbb, 0x7a, 0x90, 0x95, 0x4f,
            0x7b, 0x76, 0x36, 0x1f, 0x6e, 0xdb, 0x5c, 0x0a,
        ];
        let secret = schnorrkel::MiniSecretKey::from_bytes(&alice_seed)
            .expect("valid mini secret");
        let keypair = secret.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
        let pubkey = keypair.public.to_bytes();
        println!("Using Alice pubkey: {}", hex_encode(&pubkey));
        let sign_fn = |msg: &[u8]| -> [u8; 64] {
            let ctx = schnorrkel::signing_context(b"substrate");
            keypair.sign(ctx.bytes(msg)).to_bytes()
        };

        let dk = string_to_topic(app_id);
        let ch = string_to_topic(&channel);
        let epoca_topic = string_to_topic("ss-epoca");

        let encoded = encode_statement(
            Some(&dk),
            Some(&ch),
            now_secs,
            &[epoca_topic],
            &payload_bytes,
            &pubkey,
            &sign_fn,
        );

        println!("Encoded statement: {} bytes", encoded.len());
        println!("Target channel: {channel}");
        println!("Submitting to previewnet...");

        let result = rpc_submit(&encoded);
        println!("Submit result: {result:?}");
        assert!(result.is_ok(), "submit failed: {result:?}");
    }
}
