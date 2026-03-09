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
                // Result can be a string ("ok") or object ({"status":"rejected","reason":"noAllowance"}).
                if let Some(s) = result.and_then(|v| v.as_str()) {
                    if s == "ok" { return Ok(()); }
                    return Err(format!("statement rejected: {s}"));
                }
                if let Some(obj) = result.and_then(|v| v.as_object()) {
                    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    let reason = obj.get("reason").and_then(|v| v.as_str()).unwrap_or("unknown");
                    if status == "rejected" {
                        return Err(format!("statement rejected: {reason}"));
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

/// Load a persisted sr25519 keypair from ~/.epoca/peer_key, or generate and save one.
fn load_or_generate_keypair() -> Keypair {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = std::path::PathBuf::from(&home).join(".epoca");
    let path = dir.join("peer_key");

    // Try loading existing key (32-byte mini secret).
    if let Ok(data) = std::fs::read(&path) {
        if data.len() == 32 {
            if let Ok(mini) = schnorrkel::MiniSecretKey::from_bytes(&data) {
                let kp = mini.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
                log::info!("[ss] loaded peer key from {}", path.display());
                return kp;
            }
        }
        log::warn!("[ss] invalid peer_key file, regenerating");
    }

    // Generate new key and save.
    let mini = schnorrkel::MiniSecretKey::generate();
    let kp = mini.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
    let _ = std::fs::create_dir_all(&dir);
    if let Err(e) = std::fs::write(&path, mini.as_bytes()) {
        log::warn!("[ss] failed to save peer key: {e}");
    } else {
        log::info!("[ss] generated and saved new peer key to {}", path.display());
    }
    kp
}

/// Initialize the statement store with a persisted keypair.
/// On first run, generates a random key and saves it to ~/.epoca/peer_key.
/// On subsequent runs, loads the saved key for a stable peer identity.
/// Alice's sudo provisions the on-chain allowance for this key.
pub fn init() {
    let keypair = load_or_generate_keypair();
    log::info!("[ss] using persisted peer key");
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
        "[ss] statement store initialized (pubkey={}...)",
        hex_encode(&pubkey[..4])
    );

    // Ensure the signing key has statement store allowance on-chain.
    // Uses Alice (sudo) to write the allowance via System.set_storage.
    std::thread::spawn(|| {
        match ensure_allowance() {
            Ok(()) => log::info!("[ss] allowance confirmed"),
            Err(e) => log::warn!("[ss] ensure_allowance failed: {e}"),
        }
    });
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

/// Return the raw 32-byte public key of the signing keypair.
pub fn public_key_bytes() -> Option<[u8; 32]> {
    let st = store().lock().unwrap();
    st.as_ref().map(|s| s.keypair.public.to_bytes())
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

// ---------------------------------------------------------------------------
// Allowance provisioning — Sudo.sudo(System.set_storage) extrinsic
// ---------------------------------------------------------------------------

/// Alice's well-known mini-secret key (all Substrate dev/test chains).
const ALICE_MINI_SECRET: [u8; 32] = [
    0xe5, 0xbe, 0x9a, 0x50, 0x92, 0xb8, 0x1b, 0xca,
    0x64, 0xbe, 0x81, 0xd2, 0x12, 0xe7, 0xf2, 0xf9,
    0xeb, 0xa1, 0x83, 0xbb, 0x7a, 0x90, 0x95, 0x4f,
    0x7b, 0x76, 0x36, 0x1f, 0x6e, 0xdb, 0x5c, 0x0a,
];

/// Encode a length-prefixed SCALE bytes field: Compact<u32>(len) + bytes.
fn scale_bytes(data: &[u8]) -> Vec<u8> {
    let mut out = encode_compact_u32(data.len() as u32);
    out.extend_from_slice(data);
    out
}

/// Query pallet indices for System and Sudo from the chain metadata.
///
/// Strategy: fetch raw metadata hex, perform a structural scan of SCALE metadata v14
/// to find PalletMetadata entries by name and extract their `index: u8` fields.
///
/// The SCALE metadata v14 PalletMetadata encoding (observed on Substrate chains):
///   name:     String (compact_len + utf8)
///   storage:  Vec<StorageEntryMetadata>  (compact_count + entries)
///   calls:    Option<compact type_ref>   (0x00 | 0x01 + compact)
///   events:   Option<compact type_ref>   (0x00 | 0x01 + compact)
///   constants: Vec<PalletConstantMetadata>
///   errors:   Option<compact type_ref>   (0x00 | 0x01 + compact)
///   index:    u8
///
/// Returns (system_index, sudo_index).
fn query_pallet_indices(endpoint: &str) -> Result<(u8, u8), String> {
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "state_getMetadata",
        "params": []
    });
    let resp = ws_request(endpoint, &req.to_string())?;
    let body: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("parse metadata response: {e}"))?;
    let hex_str = body
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("no result in metadata response")?;
    let meta_bytes = hex_decode(hex_str).ok_or("invalid hex in metadata response")?;

    if meta_bytes.len() < 5 {
        return Err("metadata too short".into());
    }
    if &meta_bytes[0..4] != b"meta" {
        return Err(format!("bad metadata magic: {:02x?}", &meta_bytes[0..4]));
    }
    let version = meta_bytes[4];
    log::info!("[ss] metadata version: {version}");

    // System pallet is always index 0 in all Substrate chains — hardcode it.
    let system_index: u8 = 0;

    // For Sudo: scan metadata for a pallet with this name and extract its index.
    // We do a structural parse of each occurrence.
    let sudo_index = scan_pallet_index_in_metadata(&meta_bytes, "Sudo")
        .ok_or("Sudo pallet not found in metadata — chain may not have sudo")?;

    log::info!("[ss] pallet indices: System={system_index}, Sudo={sudo_index}");
    Ok((system_index, sudo_index))
}

/// Scan raw SCALE metadata v14 bytes for a pallet by name and return its `index: u8`.
///
/// Searches all occurrences of the compact-encoded pallet name, then attempts a
/// structural walk of the PalletMetadata fields to reach the `index: u8` at the end.
///
/// The observed SCALE layout for PalletMetadata in metadata v14 is:
///   name:      String
///   storage:   Vec<StorageEntryMetadata>   (compact count, then entries)
///   calls:     Option<compact<u32>>        (0x00=None or 0x01+compact)
///   events:    Option<compact<u32>>        (0x00=None or 0x01+compact)
///   constants: Vec<PalletConstantMetadata> (complex — see below)
///   errors:    Option<compact<u32>>        (0x00=None or 0x01+compact)
///   index:     u8
///
/// StorageEntryMetadata: name(String) + modifier(u8) + type(StorageEntryType) +
///                       default(Vec<u8>) + docs(Vec<String>)
/// StorageEntryType: kind(u8=0 Plain|1 Map) + compact type refs + optional hasher bytes
/// PalletConstantMetadata: name(String) + ty(compact) + value(Vec<u8>) + docs(Vec<String>)
fn scan_pallet_index_in_metadata(meta: &[u8], pallet_name: &str) -> Option<u8> {
    let name_bytes = pallet_name.as_bytes();
    let mut compact_name = encode_compact_u32(name_bytes.len() as u32);
    compact_name.extend_from_slice(name_bytes);

    // Scan all occurrences of the compact-encoded name in the metadata bytes.
    let search_space = if meta.len() > 5 { &meta[5..] } else { return None };
    let mut start = 0;

    while start + compact_name.len() <= search_space.len() {
        if &search_space[start..start + compact_name.len()] == compact_name.as_slice() {
            let after_name = start + compact_name.len();
            if let Some(idx) = try_parse_pallet_after_name(search_space, after_name) {
                log::debug!(
                    "[ss] found pallet '{pallet_name}' index={idx} at offset {start}"
                );
                return Some(idx);
            }
        }
        start += 1;
    }
    None
}

/// Attempt to parse PalletMetadata fields starting immediately after the pallet name.
///
/// Returns the pallet index byte if the structural walk succeeds, or `None` if it
/// encounters an inconsistency (which means this occurrence is not a real pallet).
fn try_parse_pallet_after_name(meta: &[u8], pos: usize) -> Option<u8> {
    let limit = meta.len();
    let mut p = pos;

    // --- storage: Vec<StorageEntryMetadata> ---
    // compact count, then each entry: name(str) + modifier(u8) + type + default(bytes) + docs
    let (storage_count, consumed) = decode_compact_u32(&meta[p..]).ok()?;
    p += consumed;
    // Sanity: a pallet won't have more than 200 storage entries.
    if storage_count > 200 {
        return None;
    }
    for _ in 0..storage_count {
        p = skip_storage_entry(meta, p, limit)?;
    }

    // --- calls: Option<compact<u32>> ---
    p = skip_option_compact(meta, p, limit)?;

    // --- events: Option<compact<u32>> ---
    p = skip_option_compact(meta, p, limit)?;

    // --- constants: Vec<PalletConstantMetadata> ---
    let (const_count, consumed) = decode_compact_u32(&meta[p..]).ok()?;
    p += consumed;
    if const_count > 100 {
        return None;
    }
    for _ in 0..const_count {
        p = skip_pallet_constant(meta, p, limit)?;
    }

    // --- errors: Option<compact<u32>> ---
    p = skip_option_compact(meta, p, limit)?;

    // --- index: u8 ---
    if p >= limit {
        return None;
    }
    let index = meta[p];
    // Sanity check: pallet indices are < 255 but realistically < 200.
    if index > 200 {
        return None;
    }
    Some(index)
}

/// Skip an `Option<compact<u32>>` field: either `0x00` (None) or `0x01 + compact`.
fn skip_option_compact(meta: &[u8], pos: usize, limit: usize) -> Option<usize> {
    if pos >= limit {
        return None;
    }
    match meta[pos] {
        0x00 => Some(pos + 1),
        0x01 => {
            let p = pos + 1;
            if p >= limit {
                return None;
            }
            let (_, consumed) = decode_compact_u32(&meta[p..]).ok()?;
            Some(p + consumed)
        }
        _ => None, // unexpected tag — not a valid Option<compact>
    }
}

/// Skip a `StorageEntryMetadata`:
///   name: String
///   modifier: u8
///   type: StorageEntryType  (0=Plain compact, 1=Map hashers+key+val)
///   default: Vec<u8>
///   docs: Vec<String>
fn skip_storage_entry(meta: &[u8], pos: usize, limit: usize) -> Option<usize> {
    // name: String
    let (name_len, consumed) = decode_compact_u32(&meta[pos..]).ok()?;
    if name_len > 512 {
        return None; // sanity
    }
    let mut p = pos + consumed + name_len as usize;
    if p >= limit {
        return None;
    }

    // modifier: u8 (0=Optional, 1=Default)
    let modifier = meta[p];
    if modifier > 1 {
        return None;
    }
    p += 1;

    // type: StorageEntryType
    if p >= limit {
        return None;
    }
    match meta[p] {
        0 => {
            // Plain — compact type ref
            p += 1;
            let (_, consumed) = decode_compact_u32(&meta[p..]).ok()?;
            p += consumed;
        }
        1 => {
            // Map — hashers (Vec<u8>) + key (compact) + value (compact)
            p += 1;
            let (hasher_count, consumed) = decode_compact_u32(&meta[p..]).ok()?;
            p += consumed;
            if hasher_count > 8 {
                return None;
            }
            p += hasher_count as usize; // each hasher is 1 byte enum
            let (_, consumed) = decode_compact_u32(&meta[p..]).ok()?;
            p += consumed; // key type ref
            let (_, consumed) = decode_compact_u32(&meta[p..]).ok()?;
            p += consumed; // value type ref
        }
        _ => return None, // unknown storage type
    }

    // default: Vec<u8> — compact length + bytes
    let (default_len, consumed) = decode_compact_u32(&meta[p..]).ok()?;
    p += consumed + default_len as usize;
    if p > limit {
        return None;
    }

    // docs: Vec<String>
    p = skip_vec_of_strings(meta, p, limit)?;

    Some(p)
}

/// Skip a `PalletConstantMetadata`:
///   name: String
///   ty: compact<u32>
///   value: Vec<u8>
///   docs: Vec<String>
fn skip_pallet_constant(meta: &[u8], pos: usize, limit: usize) -> Option<usize> {
    // name: String
    let (name_len, consumed) = decode_compact_u32(&meta[pos..]).ok()?;
    if name_len > 256 {
        return None;
    }
    let mut p = pos + consumed + name_len as usize;
    if p > limit {
        return None;
    }
    // ty: compact<u32>
    let (_, consumed) = decode_compact_u32(&meta[p..]).ok()?;
    p += consumed;
    // value: Vec<u8>
    let (val_len, consumed) = decode_compact_u32(&meta[p..]).ok()?;
    p += consumed + val_len as usize;
    if p > limit {
        return None;
    }
    // docs: Vec<String>
    p = skip_vec_of_strings(meta, p, limit)?;
    Some(p)
}

/// Skip a `Vec<String>`: compact count, then each string as compact_len + utf8 bytes.
fn skip_vec_of_strings(meta: &[u8], pos: usize, limit: usize) -> Option<usize> {
    let (count, consumed) = decode_compact_u32(&meta[pos..]).ok()?;
    if count > 1024 {
        return None;
    }
    let mut p = pos + consumed;
    for _ in 0..count {
        let (slen, consumed) = decode_compact_u32(&meta[p..]).ok()?;
        if slen > 65536 {
            return None;
        }
        p += consumed + slen as usize;
        if p > limit {
            return None;
        }
    }
    Some(p)
}

/// Ensure a specific public key has statement store allowance on the People chain.
///
/// Checks whether `:statement_allowance:<pubkey>` exists in chain storage.
/// If not, submits a `Sudo.sudo(System.set_storage)` extrinsic signed by Alice
/// to grant `(max_count=50, max_size=51200)`.
///
/// This is a blocking call that performs several network round-trips.
pub fn ensure_allowance_for_pubkey(pubkey: &[u8; 32]) -> Result<(), String> {
    let endpoint = SS_ENDPOINTS[0];

    log::info!("[ss] ensure_allowance: checking for pubkey {}", hex_encode(pubkey));

    // --- Step 2: Check existing allowance ---
    let allowance_key = build_allowance_storage_key(&pubkey);
    let check_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "state_getStorage",
        "params": [allowance_key]
    });
    let resp = ws_request(endpoint, &check_req.to_string())?;
    let body: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("parse storage response: {e}"))?;
    let existing = body.get("result");
    if existing.is_some() && existing != Some(&serde_json::Value::Null) {
        log::info!("[ss] allowance already exists: {existing:?}");
        return Ok(());
    }
    log::info!("[ss] no allowance found — will submit sudo extrinsic");

    // --- Step 3: Gather chain state for extrinsic construction ---
    let (spec_version, tx_version) = query_runtime_version(endpoint)?;
    let genesis_hash = query_block_hash(endpoint, Some(0))?;
    let (system_index, sudo_index) = query_pallet_indices(endpoint)?;
    let alice_nonce = query_nonce(endpoint)?;

    log::info!(
        "[ss] chain state: spec={spec_version} tx={tx_version} \
         system={system_index} sudo={sudo_index} nonce={alice_nonce}"
    );

    // --- Step 4: Build the call data ---
    // Allowance value: SCALE-encoded (max_count: u32, max_size: u32)
    let allowance_value = {
        let mut v = Vec::new();
        v.extend_from_slice(&50u32.to_le_bytes());    // max_count = 50
        v.extend_from_slice(&51200u32.to_le_bytes()); // max_size = 50KB
        v
    };

    // Raw storage key bytes (not hex — SCALE encodes the raw key bytes directly).
    let raw_key_bytes = build_allowance_key_bytes(&pubkey);

    // System.set_storage call:
    //   pallet_index: u8
    //   call_index: u8 (set_storage is call index 4 in System pallet on this chain)
    //   items: Vec<(Vec<u8>, Vec<u8>)>  = Compact(1) + key_bytes_scale + value_bytes_scale
    //
    // System call indices (verified from chain metadata):
    //   0=remark, 1=set_heap_pages, 2=set_code, 3=set_code_without_checks,
    //   4=set_storage, 5=kill_storage, 6=kill_prefix, 7=remark_with_event
    let set_storage_call = {
        let mut call = Vec::new();
        call.push(system_index);                         // System pallet index (always 0)
        call.push(4u8);                                  // set_storage call index = 4
        call.extend_from_slice(&encode_compact_u32(1));  // 1 item in the vec
        call.extend_from_slice(&scale_bytes(&raw_key_bytes));   // key
        call.extend_from_slice(&scale_bytes(&allowance_value)); // value
        call
    };

    // Sudo.sudo call:
    //   pallet_index: u8
    //   call_index: u8 (sudo is call 0 in Sudo pallet)
    //   call: Box<Call>  = just the inner call bytes (no length prefix — it's inlined)
    let call_data = {
        let mut call = Vec::new();
        call.push(sudo_index); // Sudo pallet index
        call.push(0u8);        // sudo call index = 0
        call.extend_from_slice(&set_storage_call);
        call
    };

    // --- Step 5: Build signed extrinsic ---
    let extrinsic = build_signed_extrinsic(
        &call_data,
        spec_version,
        tx_version,
        &genesis_hash,
        alice_nonce,
    )?;

    log::info!("[ss] submitting sudo extrinsic ({} bytes)", extrinsic.len());

    // --- Step 6: Submit extrinsic ---
    let submit_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "author_submitExtrinsic",
        "params": [hex_encode(&extrinsic)]
    });
    let submit_resp = ws_request(endpoint, &submit_req.to_string())?;
    let submit_body: serde_json::Value = serde_json::from_str(&submit_resp)
        .map_err(|e| format!("parse submit response: {e}"))?;

    if let Some(err) = submit_body.get("error") {
        return Err(format!("extrinsic submission failed: {err}"));
    }
    let tx_hash = submit_body
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or("(no hash)");
    log::info!("[ss] sudo extrinsic submitted, tx_hash={tx_hash}");

    // Wait briefly for the extrinsic to be included, then verify.
    std::thread::sleep(std::time::Duration::from_secs(6));

    let verify_resp = ws_request(endpoint, &check_req.to_string())?;
    let verify_body: serde_json::Value =
        serde_json::from_str(&verify_resp).map_err(|e| format!("parse verify response: {e}"))?;
    let after = verify_body.get("result");
    if after.is_some() && after != Some(&serde_json::Value::Null) {
        log::info!("[ss] allowance confirmed in storage: {after:?}");
        Ok(())
    } else {
        Err(format!(
            "extrinsic submitted (tx={tx_hash}) but allowance not found in storage after 6s. \
             The extrinsic may still be pending."
        ))
    }
}

/// Ensure the statement store signing key has allowance on the People chain.
pub fn ensure_allowance() -> Result<(), String> {
    let pubkey: [u8; 32] = {
        let st = store().lock().unwrap();
        let state = st
            .as_ref()
            .ok_or("statement store not initialized — call init() first")?;
        state.keypair.public.to_bytes()
    };
    ensure_allowance_for_pubkey(&pubkey)
}

/// Build the hex-encoded storage key for a statement allowance entry.
///
/// Key format: `0x` + hex(`:statement_allowance:`) + hex(pubkey_32_bytes)
fn build_allowance_storage_key(pubkey: &[u8; 32]) -> String {
    let prefix = b":statement_allowance:";
    let mut full_key = Vec::with_capacity(prefix.len() + 32);
    full_key.extend_from_slice(prefix);
    full_key.extend_from_slice(pubkey);
    hex_encode(&full_key)
}

/// Build the raw bytes of the allowance storage key (not hex, not prefixed with 0x).
fn build_allowance_key_bytes(pubkey: &[u8; 32]) -> Vec<u8> {
    let prefix = b":statement_allowance:";
    let mut key = Vec::with_capacity(prefix.len() + 32);
    key.extend_from_slice(prefix);
    key.extend_from_slice(pubkey);
    key
}

/// Query spec_version and transaction_version from state_getRuntimeVersion.
fn query_runtime_version(endpoint: &str) -> Result<(u32, u32), String> {
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "state_getRuntimeVersion",
        "params": []
    });
    let resp = ws_request(endpoint, &req.to_string())?;
    let body: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("parse runtime version: {e}"))?;
    let result = body.get("result").ok_or("no result in getRuntimeVersion")?;
    let spec = result
        .get("specVersion")
        .and_then(|v| v.as_u64())
        .ok_or("no specVersion")? as u32;
    let tx = result
        .get("transactionVersion")
        .and_then(|v| v.as_u64())
        .ok_or("no transactionVersion")? as u32;
    Ok((spec, tx))
}

/// Query a block hash. Pass `Some(block_number)` for a specific block, or `None` for
/// the current best block.
fn query_block_hash(endpoint: &str, block_number: Option<u64>) -> Result<[u8; 32], String> {
    let params = match block_number {
        Some(n) => serde_json::json!([n]),
        None => serde_json::json!([]),
    };
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_getBlockHash",
        "params": params
    });
    let resp = ws_request(endpoint, &req.to_string())?;
    let body: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("parse block hash: {e}"))?;
    let hex_str = body
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or("no result in getBlockHash")?;
    let bytes = hex_decode(hex_str).ok_or("invalid hex in block hash")?;
    if bytes.len() != 32 {
        return Err(format!("block hash wrong length: {}", bytes.len()));
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

/// Query the next account nonce for Alice using system_accountNextIndex.
fn query_nonce(endpoint: &str) -> Result<u32, String> {
    // Alice's SS58 address on Substrate (prefix 42 = generic).
    // Derived from her well-known public key.
    // 5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY
    let alice_ss58 = "5GrwvaEF5zXb26Fz9rcQpDWS57CtERHpNehXCPcNoHGKutQY";
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "system_accountNextIndex",
        "params": [alice_ss58]
    });
    let resp = ws_request(endpoint, &req.to_string())?;
    let body: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("parse nonce response: {e}"))?;
    let nonce = body
        .get("result")
        .and_then(|v| v.as_u64())
        .ok_or("no result in accountNextIndex")? as u32;
    Ok(nonce)
}

/// Build a signed Substrate extrinsic (v4 format) using Alice's sr25519 key.
///
/// The extrinsic bytes layout (PoP3 People chain, 19 signed extensions):
/// ```text
/// compact_total_length
/// 0x84  (signed bit | version 4)
/// 0x00  (MultiAddress::Id prefix)
/// alice_pubkey[32]
/// 0x01  (MultiSignature::Sr25519 prefix)
/// signature[64]
/// <extra bytes for all 19 signed extensions>
/// call_data[...]
/// ```
///
/// Signed extensions on PoP3 People chain (v15 metadata, order matters):
///   [0]  VerifyMultiSignature     ty=338  Variant{Signed,Disabled} → 0x01 (Disabled)
///   [1]  AsPerson                 ty=339  Option<AsPersonInfo>      → 0x00 (None)
///   [2]  AsProofOfInkParticipant  ty=342  Option<...>               → 0x00 (None)
///   [3]  ProvideForVoucherClaimer ty=345  unit struct               → (nothing)
///   [4]  ScoreAsParticipant       ty=346  Option<...>               → 0x00 (None)
///   [5]  GameAsInvited            ty=349  Option<...>               → 0x00 (None)
///   [6]  PeopleLiteAuth           ty=352  Option<...>               → 0x00 (None)
///   [7]  AsCoinage                ty=355  Option<...>               → 0x00 (None)
///   [8]  AuthorizeCall            ty=360  unit struct               → (nothing)
///   [9]  RestrictOrigins          ty=361  Bool                      → 0x00 (false)
///   [10] CheckNonZeroSender       ty=362  unit struct               → (nothing)
///   [11] CheckSpecVersion         ty=363  unit struct               → (nothing)
///   [12] CheckTxVersion           ty=364  unit struct               → (nothing)
///   [13] CheckGenesis             ty=365  unit struct               → (nothing)
///   [14] CheckMortality           ty=366  Era                       → 0x00 (Immortal)
///   [15] CheckNonce               ty=368  Compact<u32>              → compact(nonce)
///   [16] CheckWeight              ty=369  unit struct               → (nothing)
///   [17] ChargeAssetTxPayment     ty=370  {tip: Compact, asset_id: Option} → 0x00 + 0x00
///   [18] StorageWeightReclaim     ty=4    unit tuple                → (nothing)
///
/// Extra bytes total:
///   0x01  (VerifyMultiSignature::Disabled)
///   0x00  (AsPerson = None)
///   0x00  (AsProofOfInkParticipant = None)
///   (nothing for ProvideForVoucherClaimer)
///   0x00  (ScoreAsParticipant = None)
///   0x00  (GameAsInvited = None)
///   0x00  (PeopleLiteAuth = None)
///   0x00  (AsCoinage = None)
///   (nothing for AuthorizeCall)
///   0x00  (RestrictOrigins = false)
///   (nothing for CheckNonZeroSender)
///   (nothing for CheckSpecVersion)
///   (nothing for CheckTxVersion)
///   (nothing for CheckGenesis)
///   0x00  (CheckMortality = Immortal)
///   compact(nonce)  (CheckNonce)
///   (nothing for CheckWeight)
///   0x00  (ChargeAssetTxPayment tip = compact(0))
///   0x00  (ChargeAssetTxPayment asset_id = None)
///   (nothing for StorageWeightReclaim)
///
/// Additional signed (add_ty contributions, unit for all except standard 4):
///   spec_version(u32le) + tx_version(u32le) + genesis_hash[32] + genesis_hash[32]
///
/// Note: CheckMortality::additional_signed for Era::Immortal returns the genesis hash,
/// so genesis_hash appears twice (once for CheckGenesis, once for CheckMortality).
///
/// If the signing payload exceeds 256 bytes, sign blake2b_256(payload) instead.
fn build_signed_extrinsic(
    call_data: &[u8],
    spec_version: u32,
    tx_version: u32,
    genesis_hash: &[u8; 32],
    nonce: u32,
) -> Result<Vec<u8>, String> {
    let secret = schnorrkel::MiniSecretKey::from_bytes(&ALICE_MINI_SECRET)
        .map_err(|e| format!("invalid Alice mini-secret: {e}"))?;
    let alice_kp = secret.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
    let alice_pubkey = alice_kp.public.to_bytes();

    // Build the `extra` bytes for all 19 signed extensions on PoP3 People chain.
    // Each extension contributes its "implicit" (extra) bytes in declaration order.
    let mut extra = Vec::new();
    extra.push(0x01u8);                               // VerifyMultiSignature::Disabled (variant index 1)
    extra.push(0x00u8);                               // AsPerson = None
    extra.push(0x00u8);                               // AsProofOfInkParticipant = None
    // ProvideForVoucherClaimer — unit struct, 0 bytes
    extra.push(0x00u8);                               // ScoreAsParticipant = None
    extra.push(0x00u8);                               // GameAsInvited = None
    extra.push(0x00u8);                               // PeopleLiteAuth = None
    extra.push(0x00u8);                               // AsCoinage = None
    // AuthorizeCall — unit struct, 0 bytes
    extra.push(0x00u8);                               // RestrictOrigins = Bool(false)
    // CheckNonZeroSender — unit struct, 0 bytes
    // CheckSpecVersion — unit struct, 0 bytes
    // CheckTxVersion — unit struct, 0 bytes
    // CheckGenesis — unit struct, 0 bytes
    extra.push(0x00u8);                               // CheckMortality = Era::Immortal
    extra.extend_from_slice(&encode_compact_u32(nonce)); // CheckNonce = compact(nonce)
    // CheckWeight — unit struct, 0 bytes
    extra.push(0x00u8);                               // ChargeAssetTxPayment tip = Compact(0)
    extra.push(0x00u8);                               // ChargeAssetTxPayment asset_id = None
    // StorageWeightReclaim — unit tuple, 0 bytes

    // Additional signed data contributed by CheckSpecVersion, CheckTxVersion,
    // CheckGenesis, and CheckMortality (all other extensions have unit add_ty).
    //
    // For Era::Immortal, CheckMortality::additional_signed() returns the genesis hash,
    // so genesis_hash appears twice.
    let mut additional = Vec::new();
    additional.extend_from_slice(&spec_version.to_le_bytes());
    additional.extend_from_slice(&tx_version.to_le_bytes());
    additional.extend_from_slice(genesis_hash); // CheckGenesis
    additional.extend_from_slice(genesis_hash); // CheckMortality (immortal → genesis hash)

    // Signing payload = call_data + extra + additional_signed.
    let mut signing_payload = Vec::new();
    signing_payload.extend_from_slice(call_data);
    signing_payload.extend_from_slice(&extra);
    signing_payload.extend_from_slice(&additional);

    // If payload > 256 bytes, sign the blake2b_256 hash instead.
    let to_sign: Vec<u8> = if signing_payload.len() > 256 {
        blake2b_256(&signing_payload).to_vec()
    } else {
        signing_payload.clone()
    };

    let ctx = schnorrkel::signing_context(SIGNING_CTX);
    let sig = alice_kp.sign(ctx.bytes(&to_sign)).to_bytes();

    // Assemble the signed extrinsic body (without the outer length prefix).
    // Version byte: signed (bit 7 = 1) | version 4 (bits 0-6).
    // Despite the chain's unsigned inherents using 0x05 (v5), signed transactions use
    // the legacy extrinsic v4 format (0x84). Confirmed empirically: 0x84 + full 13-byte
    // extra produces "bad signature" (structure valid); 0x85 produces unreachable panic.
    let mut body = Vec::new();
    body.push(0x84u8);    // signed (bit 7 = 1) | version 4 (bits 0-6) = 0x84
    body.push(0x00u8);    // MultiAddress::Id prefix
    body.extend_from_slice(&alice_pubkey);
    body.push(0x01u8);    // MultiSignature::Sr25519 prefix
    body.extend_from_slice(&sig);
    body.extend_from_slice(&extra);
    body.extend_from_slice(call_data);

    // Outer compact-length prefix.
    let mut extrinsic = encode_compact_u32(body.len() as u32);
    extrinsic.extend_from_slice(&body);

    Ok(extrinsic)
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

    /// Decode an SS58-encoded address to a 32-byte account ID.
    /// Uses big-number base58 decoding.
    fn ss58_decode(addr: &str) -> Option<[u8; 32]> {
        const ALPHABET: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

        // Decode base58 to big integer represented as big-endian bytes.
        // We accumulate into a u8 vec using schoolbook multiplication.
        let mut result: Vec<u8> = Vec::new();
        for &c in addr.as_bytes() {
            let val = ALPHABET.iter().position(|&a| a == c)? as u16;
            // Multiply existing number by 58 and add val.
            let mut carry = val;
            for byte in result.iter_mut().rev() {
                let v = (*byte as u16) * 58 + carry;
                *byte = (v & 0xff) as u8;
                carry = v >> 8;
            }
            while carry > 0 {
                result.insert(0, (carry & 0xff) as u8);
                carry >>= 8;
            }
        }

        // Prepend leading zero bytes for leading '1's.
        let leading_ones = addr.as_bytes().iter().take_while(|&&b| b == b'1').count();
        let mut decoded = vec![0u8; leading_ones];
        decoded.extend_from_slice(&result);

        // decoded = prefix (1 or 2 bytes) + 32-byte pubkey + 2-byte checksum
        if decoded.len() == 35 {
            let mut account = [0u8; 32];
            account.copy_from_slice(&decoded[1..33]);
            Some(account)
        } else if decoded.len() == 36 {
            let mut account = [0u8; 32];
            account.copy_from_slice(&decoded[2..34]);
            Some(account)
        } else {
            println!("SS58 decode: unexpected length {} for {addr}", decoded.len());
            None
        }
    }

    /// Check whether the first sdchat pre-attested account has statement store
    /// allowance on PoP3 People chain.
    ///
    /// Two-pronged approach:
    ///   1. Query `:statement_allowance:<pubkey>` storage directly
    ///   2. Derive keypair from mnemonic and attempt statement submission
    ///
    /// Run with: cargo test -p epoca-chain -- --ignored test_sdchat_allowance --nocapture
    #[test]
    #[ignore]
    fn test_sdchat_allowance() {
        let _ = env_logger::try_init();

        let endpoint = SS_ENDPOINTS[0];

        // sdchat stable account #1
        let mnemonic_str = "reveal only slab nephew tuna faculty tuition upon someone index begin ceiling";
        let expected_ss58 = "5GnPAoP6E75xDu76qcJst3KKdx6Mv5QDozs3Wz9RMyy67CLq";

        // --- Derive sr25519 keypair from mnemonic ---
        // Try multiple derivation methods to find which one matches the SS58 address.
        let mnemonic = bip39::Mnemonic::parse(mnemonic_str).expect("valid mnemonic");

        // Decode expected pubkey from SS58
        let decoded = bs58::decode(expected_ss58).into_vec().expect("valid bs58");
        let mut expected_pubkey = [0u8; 32];
        expected_pubkey.copy_from_slice(&decoded[1..33]);
        println!("Expected pubkey (from SS58): {}", hex_encode(&expected_pubkey));

        // Method 1: BIP-39 standard PBKDF2 seed (first 32 bytes)
        let seed = mnemonic.to_seed("");
        let mut mini_bytes = [0u8; 32];
        mini_bytes.copy_from_slice(&seed[..32]);
        let secret1 = schnorrkel::MiniSecretKey::from_bytes(&mini_bytes).expect("valid");
        let kp1 = secret1.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
        println!("Method 1 (PBKDF2 seed[:32]): {}", hex_encode(&kp1.public.to_bytes()));

        // Method 2: Raw entropy (16 bytes for 12-word, zero-padded to 32)
        let entropy = mnemonic.to_entropy();
        println!("Entropy ({} bytes): {}", entropy.len(), hex_encode(&entropy));
        let mut entropy_padded = [0u8; 32];
        entropy_padded[..entropy.len()].copy_from_slice(&entropy);
        let secret2 = schnorrkel::MiniSecretKey::from_bytes(&entropy_padded).expect("valid");
        let kp2 = secret2.expand_to_keypair(schnorrkel::ExpansionMode::Ed25519);
        println!("Method 2 (entropy zero-padded): {}", hex_encode(&kp2.public.to_bytes()));

        // Method 3: BIP-39 PBKDF2 with Ed25519 expansion mode
        let secret3 = schnorrkel::MiniSecretKey::from_bytes(&mini_bytes).expect("valid");
        let kp3 = secret3.expand_to_keypair(schnorrkel::ExpansionMode::Uniform);
        println!("Method 3 (PBKDF2 seed[:32] Uniform): {}", hex_encode(&kp3.public.to_bytes()));

        // Method 4: entropy with Uniform expansion
        let secret4 = schnorrkel::MiniSecretKey::from_bytes(&entropy_padded).expect("valid");
        let kp4 = secret4.expand_to_keypair(schnorrkel::ExpansionMode::Uniform);
        println!("Method 4 (entropy Uniform): {}", hex_encode(&kp4.public.to_bytes()));

        // Find which method matches
        let keypair = if kp1.public.to_bytes() == expected_pubkey {
            println!("MATCH: Method 1 (PBKDF2 Ed25519)");
            kp1
        } else if kp2.public.to_bytes() == expected_pubkey {
            println!("MATCH: Method 2 (entropy Ed25519)");
            kp2
        } else if kp3.public.to_bytes() == expected_pubkey {
            println!("MATCH: Method 3 (PBKDF2 Uniform)");
            kp3
        } else if kp4.public.to_bytes() == expected_pubkey {
            println!("MATCH: Method 4 (entropy Uniform)");
            kp4
        } else {
            println!("WARNING: No derivation method matched the SS58 address!");
            println!("Using PBKDF2 Ed25519 (method 1) for submission test anyway.");
            kp1
        };
        let pubkey = keypair.public.to_bytes();

        // --- Step 1: Query allowance storage directly ---
        // The statement store uses well-known key `:statement_allowance:<account_id_32_bytes>`
        let prefix_str = ":statement_allowance:";
        let prefix_hex = hex_encode(prefix_str.as_bytes());

        // Query for the expected SS58 account (from CSV)
        let expected_hex: String = expected_pubkey.iter().map(|b| format!("{b:02x}")).collect();
        let expected_key = format!("{prefix_hex}{expected_hex}");
        println!("\n=== Step 1a: Query storage for SS58 account ===");
        println!("Storage key: {expected_key}");
        let check_req_expected = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "state_getStorage",
            "params": [expected_key]
        });
        match ws_request(endpoint, &check_req_expected.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                let value = body.get("result");
                println!("Response: {body}");
                if value.is_none() || value == Some(&serde_json::Value::Null) {
                    println!("=> Storage for SS58 account: NO allowance entry");
                } else {
                    println!("=> Storage for SS58 account: allowance exists: {value:?}");
                }
            }
            Err(e) => println!("Storage query error: {e}"),
        }

        // Also query for the derived account (may be different)
        let account_hex_raw: String = pubkey.iter().map(|b| format!("{b:02x}")).collect();
        let full_key = format!("{prefix_hex}{account_hex_raw}");
        if pubkey != expected_pubkey {
            println!("\n=== Step 1b: Query storage for derived account ===");
            println!("Storage key: {full_key}");
        }

        println!("\n=== Step 1: Query storage key ===");
        println!("Storage key: {full_key}");

        let check_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "state_getStorage",
            "params": [full_key]
        });
        match ws_request(endpoint, &check_req.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                let value = body.get("result");
                println!("Raw response: {body}");
                if value.is_none() || value == Some(&serde_json::Value::Null) {
                    println!("=> Storage: NO allowance entry (null)");
                } else {
                    println!("=> Storage: allowance entry exists: {value:?}");
                }
            }
            Err(e) => println!("Storage query error: {e}"),
        }

        // Also enumerate all keys with this prefix to see who HAS allowance
        println!("\n=== Enumerating all allowance keys ===");
        let keys_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "state_getKeysPaged",
            "params": [prefix_hex, 20]
        });
        match ws_request(endpoint, &keys_req.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                if let Some(err) = body.get("error") {
                    println!("RPC error: {err}");
                } else if let Some(keys) = body.get("result").and_then(|v| v.as_array()) {
                    println!("Found {} accounts with allowance", keys.len());
                    let prefix_byte_len = prefix_str.len();
                    for (i, key_val) in keys.iter().enumerate() {
                        if let Some(key_hex) = key_val.as_str() {
                            let raw = key_hex.strip_prefix("0x").unwrap_or(key_hex);
                            if raw.len() >= prefix_byte_len * 2 + 64 {
                                let acct = &raw[(prefix_byte_len * 2)..];
                                println!("  [{i}] 0x{acct}");
                                // Check if this matches our sdchat account
                                if acct == account_hex_raw {
                                    println!("       ^^^ THIS IS THE SDCHAT ACCOUNT!");
                                }
                            } else {
                                println!("  [{i}] {key_hex}");
                            }
                        }
                    }
                }
            }
            Err(e) => println!("Keys enumeration error: {e}"),
        }

        // --- Step 2: Try submitting a statement ---
        println!("\n=== Step 2: Submit test statement ===");
        let dk = string_to_topic("sdchat-allowance-test");
        let ch = string_to_topic("test-probe");
        let epoca_topic = string_to_topic("ss-epoca");
        let data = b"allowance probe";

        let sign_fn = |msg: &[u8]| -> [u8; 64] {
            let ctx = schnorrkel::signing_context(b"substrate");
            keypair.sign(ctx.bytes(msg)).to_bytes()
        };

        let encoded = encode_statement(
            Some(&dk),
            Some(&ch),
            1,
            &[epoca_topic],
            data,
            &pubkey,
            &sign_fn,
        );
        println!("Encoded statement: {} bytes", encoded.len());

        let submit_payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "statement_submit",
            "params": [hex_encode(&encoded)]
        });

        match ws_request(endpoint, &submit_payload.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                println!("Submit response: {body}");
                let result = body.get("result");
                // Result can be "ok" (string) or {"status":"rejected","reason":"noAllowance"} (object)
                if let Some(s) = result.and_then(|v| v.as_str()) {
                    match s {
                        "ok" => println!("=> RESULT: Statement ACCEPTED -- account HAS allowance"),
                        other => println!("=> RESULT: Statement rejected: {other}"),
                    }
                } else if let Some(obj) = result.and_then(|v| v.as_object()) {
                    let status = obj.get("status").and_then(|v| v.as_str()).unwrap_or("?");
                    let reason = obj.get("reason").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("=> RESULT: status={status}, reason={reason}");
                    if reason == "noAllowance" {
                        println!("=> CONFIRMED: Account has NO statement store allowance");
                    }
                } else if let Some(err) = body.get("error") {
                    println!("=> RESULT: RPC error: {err}");
                } else {
                    println!("=> RESULT: Unexpected response: {result:?}");
                }
            }
            Err(e) => println!("Submit error: {e}"),
        }
    }

    /// Check whether Alice's dev account has sudo access on PoP3 People chain.
    ///
    /// 1. Queries `Sudo.Key` storage to see who the sudo account is.
    /// 2. Queries `System.Account` for Alice to see if she has a balance.
    ///
    /// Run with: cargo test -p epoca-chain test_check_sudo -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_check_sudo() {
        let _ = env_logger::try_init();

        let endpoint = "wss://pop3-testnet.parity-lab.parity.io/people";

        // Alice's well-known public key (sr25519 on dev chains)
        let alice_pubkey: [u8; 32] = [
            0xd4, 0x35, 0x93, 0xc7, 0x15, 0xfd, 0xd3, 0x1c,
            0x61, 0x14, 0x1a, 0xbd, 0x04, 0xa9, 0x9f, 0xd6,
            0x82, 0x2c, 0x85, 0x58, 0x85, 0x4c, 0xcd, 0xe3,
            0x9a, 0x56, 0x84, 0xe7, 0xa5, 0x6d, 0xa2, 0x7d,
        ];
        println!("Alice pubkey: {}", hex_encode(&alice_pubkey));

        // =====================================================================
        // Approach 1: Query Sudo.Key
        // =====================================================================
        // twox128("Sudo") = 5c0d1176a568c1f92944340dbfed9e9c
        // twox128("Key")  = 530ebca703c85910e7164cb7d1c9e47b
        let sudo_key = "0x5c0d1176a568c1f92944340dbfed9e9c530ebca703c85910e7164cb7d1c9e47b";

        println!("\n=== Approach 1: Query Sudo.Key ===");
        println!("Storage key: {sudo_key}");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "state_getStorage",
            "params": [sudo_key]
        });

        match ws_request(endpoint, &req.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                println!("Raw response: {body}");

                let result = body.get("result");
                if result.is_none() || result == Some(&serde_json::Value::Null) {
                    println!("=> Sudo.Key storage is EMPTY (no sudo pallet or no key set)");
                } else if let Some(hex_str) = result.and_then(|v| v.as_str()) {
                    println!("=> Sudo.Key value: {hex_str}");
                    // Decode as Option<AccountId32>: 0x01 + 32 bytes (Some), or 0x00 (None)
                    if let Some(bytes) = hex_decode(hex_str) {
                        if bytes.is_empty() {
                            println!("=> Empty value");
                        } else if bytes[0] == 0x00 {
                            println!("=> Sudo.Key = None (no sudo account set)");
                        } else if bytes[0] == 0x01 && bytes.len() >= 33 {
                            let sudo_account = &bytes[1..33];
                            println!("=> Sudo account: {}", hex_encode(sudo_account));
                            if sudo_account == alice_pubkey {
                                println!("=> ALICE IS THE SUDO ACCOUNT!");
                            } else {
                                println!("=> Sudo account is NOT Alice");
                                println!("   Alice:  {}", hex_encode(&alice_pubkey));
                                println!("   Sudo:   {}", hex_encode(sudo_account));
                            }
                        } else {
                            // Maybe it's stored as raw AccountId32 without Option wrapper
                            if bytes.len() == 32 {
                                println!("=> Sudo account (raw, no Option wrapper): {}", hex_encode(&bytes));
                                if bytes.as_slice() == alice_pubkey {
                                    println!("=> ALICE IS THE SUDO ACCOUNT!");
                                } else {
                                    println!("=> Sudo account is NOT Alice");
                                }
                            } else {
                                println!("=> Unexpected format: {} bytes, first byte = 0x{:02x}",
                                    bytes.len(), bytes[0]);
                            }
                        }
                    }
                }
            }
            Err(e) => println!("Sudo.Key query error: {e}"),
        }

        // =====================================================================
        // Approach 2: Query System.Account for Alice
        // =====================================================================
        // twox128("System")  = 26aa394eea5630e07c48ae0c9558cef7
        // twox128("Account") = b99d880ec681799c0cf30e8886371da9
        // blake2_128_concat(alice_pubkey) = blake2_128(alice) ++ alice
        println!("\n=== Approach 2: Query System.Account for Alice ===");

        // Compute blake2_128 of Alice's pubkey
        use blake2::digest::consts::U16;
        type Blake2b128 = Blake2b<U16>;
        let mut hasher = <Blake2b128 as Digest>::new();
        hasher.update(&alice_pubkey);
        let hash_result = hasher.finalize();
        let mut blake2_128_hash = [0u8; 16];
        blake2_128_hash.copy_from_slice(&hash_result);

        // blake2_128_concat = blake2_128(key) ++ key
        let mut blake2_128_concat = Vec::with_capacity(16 + 32);
        blake2_128_concat.extend_from_slice(&blake2_128_hash);
        blake2_128_concat.extend_from_slice(&alice_pubkey);

        let system_account_prefix = "26aa394eea5630e07c48ae0c9558cef7b99d880ec681799c0cf30e8886371da9";
        let concat_hex: String = blake2_128_concat.iter().map(|b| format!("{b:02x}")).collect();
        let system_account_key = format!("0x{system_account_prefix}{concat_hex}");

        println!("Storage key: {system_account_key}");

        let req2 = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "state_getStorage",
            "params": [system_account_key]
        });

        match ws_request(endpoint, &req2.to_string()) {
            Ok(resp) => {
                let body: serde_json::Value = serde_json::from_str(&resp).expect("parse json");
                println!("Raw response: {body}");

                let result = body.get("result");
                if result.is_none() || result == Some(&serde_json::Value::Null) {
                    println!("=> System.Account for Alice: NOT FOUND (account does not exist on this chain)");
                } else if let Some(hex_str) = result.and_then(|v| v.as_str()) {
                    println!("=> System.Account for Alice exists!");
                    if let Some(bytes) = hex_decode(hex_str) {
                        println!("   Account data: {} bytes", bytes.len());
                        // AccountInfo layout (SCALE):
                        //   nonce: u32 (4 bytes)
                        //   consumers: u32 (4 bytes)
                        //   providers: u32 (4 bytes)
                        //   sufficients: u32 (4 bytes)
                        //   data: AccountData {
                        //     free: u128 (16 bytes)
                        //     reserved: u128 (16 bytes)
                        //     frozen: u128 (16 bytes)
                        //     flags: u128 (16 bytes)
                        //   }
                        if bytes.len() >= 64 {
                            let nonce = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
                            let consumers = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                            let providers = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
                            let sufficients = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
                            let free = u128::from_le_bytes(bytes[16..32].try_into().unwrap());
                            let reserved = u128::from_le_bytes(bytes[32..48].try_into().unwrap());
                            let frozen = u128::from_le_bytes(bytes[48..64].try_into().unwrap());

                            println!("   nonce: {nonce}");
                            println!("   consumers: {consumers}");
                            println!("   providers: {providers}");
                            println!("   sufficients: {sufficients}");
                            println!("   free balance: {free}");
                            println!("   reserved balance: {reserved}");
                            println!("   frozen: {frozen}");

                            if free > 0 || reserved > 0 {
                                println!("=> Alice HAS balance on this chain");
                            } else {
                                println!("=> Alice account exists but has ZERO balance");
                            }
                        } else {
                            println!("   (unexpected AccountInfo size: {} bytes)", bytes.len());
                        }
                    }
                }
            }
            Err(e) => println!("System.Account query error: {e}"),
        }

        println!("\n=== Done ===");
    }

    /// Grant statement store allowance to the current signing keypair using Alice sudo.
    ///
    /// Initializes the store with Alice's keypair, checks for existing allowance,
    /// and if missing submits a Sudo.sudo(System.set_storage) extrinsic.
    ///
    /// Run with: cargo test -p epoca-chain test_ensure_allowance -- --ignored --nocapture
    #[test]
    #[ignore]
    fn test_ensure_allowance() {
        let _ = env_logger::try_init();
        // Initialize the store (uses Alice's keypair).
        init();
        match ensure_allowance() {
            Ok(()) => println!("[test] ensure_allowance: OK — allowance is set"),
            Err(e) => panic!("[test] ensure_allowance failed: {e}"),
        }
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
