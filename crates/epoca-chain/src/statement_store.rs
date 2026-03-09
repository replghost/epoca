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

/// WebSocket endpoints for PoP testnet (statement store enabled).
const SS_ENDPOINTS: &[&str] = &[
    "wss://pop-testnet.parity-lab.parity.io:443/9910",
    "wss://pop3-testnet.parity-lab.parity.io:443/7910",
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
// ---------------------------------------------------------------------------

const PROOF_MASK: u8 = 0x01;
const DECRYPTION_KEY_MASK: u8 = 0x02;
const CHANNEL_MASK: u8 = 0x04;
const PRIORITY_MASK: u8 = 0x08;
const TOPIC_MASK: u8 = 0x10;
const DATA_MASK: u8 = 0x20;

/// Proof type bytes.
const PROOF_SR25519: u8 = 1;

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

/// Encode a statement into the sp-statement-store binary format.
///
/// The `sr25519_pubkey` and `sr25519_sign` callback are used to produce
/// the proof. The signature is over `blake2b_256(plain_data)` where
/// `plain_data` is all encoded fields except the mask byte and proof.
pub fn encode_statement(
    decryption_key: Option<&Topic>,
    channel: Option<&Topic>,
    priority: u32,
    topics: &[Topic],
    data: &[u8],
    sr25519_pubkey: &[u8; 32],
    sr25519_sign: &dyn Fn(&[u8]) -> [u8; 64],
) -> Vec<u8> {
    // Build mask.
    let mut mask: u8 = PROOF_MASK; // always include proof
    if decryption_key.is_some() {
        mask |= DECRYPTION_KEY_MASK;
    }
    if channel.is_some() {
        mask |= CHANNEL_MASK;
    }
    if priority != 0 {
        mask |= PRIORITY_MASK;
    }
    if !topics.is_empty() {
        mask |= TOPIC_MASK;
    }
    if !data.is_empty() {
        mask |= DATA_MASK;
    }

    // Encode plain_data (everything except mask and proof).
    let mut plain = Vec::new();
    if let Some(dk) = decryption_key {
        plain.extend_from_slice(dk);
    }
    if let Some(ch) = channel {
        plain.extend_from_slice(ch);
    }
    if priority != 0 {
        plain.extend_from_slice(&priority.to_le_bytes());
    }
    if !topics.is_empty() {
        assert!(topics.len() <= 255, "too many topics (max 255)");
        plain.push(topics.len() as u8);
        for t in topics {
            plain.extend_from_slice(t);
        }
    }
    if !data.is_empty() {
        plain.extend_from_slice(&(data.len() as u32).to_le_bytes());
        plain.extend_from_slice(data);
    }

    // Sign blake2b_256(plain_data).
    let hash = blake2b_256(&plain);
    let signature = sr25519_sign(&hash);

    // Assemble: mask || proof || plain_data.
    let mut out = Vec::with_capacity(1 + 1 + 32 + 64 + plain.len());
    out.push(mask);
    // Proof: type(1) + pubkey(32) + sig(64) = 97 bytes
    out.push(PROOF_SR25519);
    out.extend_from_slice(sr25519_pubkey);
    out.extend_from_slice(&signature);
    out.extend_from_slice(&plain);

    out
}

/// Decode a statement from its binary encoding.
pub fn decode_statement(encoded: &[u8]) -> Result<Statement, String> {
    if encoded.is_empty() {
        return Err("empty statement".into());
    }

    let mask = encoded[0];
    let mut pos = 1;

    // Skip proof.
    let mut proof_pubkey = None;
    if mask & PROOF_MASK != 0 {
        if pos >= encoded.len() {
            return Err("truncated proof type".into());
        }
        let proof_type = encoded[pos];
        pos += 1;
        let key_sig_len = match proof_type {
            PROOF_SR25519 | 2 => 32 + 64, // sr25519 or ed25519
            _ => return Err(format!("unknown proof type: {proof_type}")),
        };
        if pos + key_sig_len > encoded.len() {
            return Err("truncated proof".into());
        }
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&encoded[pos..pos + 32]);
        proof_pubkey = Some(pk);
        pos += key_sig_len;
    }

    let mut decryption_key = None;
    if mask & DECRYPTION_KEY_MASK != 0 {
        if pos + 32 > encoded.len() {
            return Err("truncated decryption_key".into());
        }
        let mut dk = [0u8; 32];
        dk.copy_from_slice(&encoded[pos..pos + 32]);
        decryption_key = Some(dk);
        pos += 32;
    }

    let mut channel = None;
    if mask & CHANNEL_MASK != 0 {
        if pos + 32 > encoded.len() {
            return Err("truncated channel".into());
        }
        let mut ch = [0u8; 32];
        ch.copy_from_slice(&encoded[pos..pos + 32]);
        channel = Some(ch);
        pos += 32;
    }

    let mut priority = 0u32;
    if mask & PRIORITY_MASK != 0 {
        if pos + 4 > encoded.len() {
            return Err("truncated priority".into());
        }
        priority = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]);
        pos += 4;
    }

    let mut topics = Vec::new();
    if mask & TOPIC_MASK != 0 {
        if pos >= encoded.len() {
            return Err("truncated topic count".into());
        }
        let count = encoded[pos] as usize;
        pos += 1;
        for _ in 0..count {
            if pos + 32 > encoded.len() {
                return Err("truncated topic".into());
            }
            let mut t = [0u8; 32];
            t.copy_from_slice(&encoded[pos..pos + 32]);
            topics.push(t);
            pos += 32;
        }
    }

    let mut data = Vec::new();
    if mask & DATA_MASK != 0 {
        if pos + 4 > encoded.len() {
            return Err("truncated data length".into());
        }
        let data_len = u32::from_le_bytes([
            encoded[pos],
            encoded[pos + 1],
            encoded[pos + 2],
            encoded[pos + 3],
        ]) as usize;
        pos += 4;
        if pos + data_len > encoded.len() {
            return Err("truncated data".into());
        }
        data = encoded[pos..pos + data_len].to_vec();
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
        "method": "statement_store_submit",
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
                log::info!("[ss] statement submitted successfully");
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
        "method": "statement_store_broadcasts",
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

    // Connect with timeout.
    let tcp = TcpStream::connect_timeout(
        &format!("{host}:{port}")
            .parse()
            .map_err(|e| format!("addr parse: {e}"))?,
        Duration::from_secs(10),
    )
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
    use std::net::TcpStream;
    use std::time::Duration;
    use tungstenite::client::IntoClientRequest;

    for endpoint in SS_ENDPOINTS {
        let request = match endpoint.into_client_request() {
            Ok(r) => r,
            Err(_) => continue,
        };
        let host = match request.uri().host() {
            Some(h) => h.to_string(),
            None => continue,
        };
        let port = request.uri().port_u16().unwrap_or(443);

        // TCP connect with 3s timeout — just checks reachability
        let addr: std::net::SocketAddr = match format!("{host}:{port}").parse() {
            Ok(a) => a,
            Err(_) => continue,
        };
        match TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(_) => return Ok(endpoint.to_string()),
            Err(e) => {
                log::warn!("[ss] ping failed for {endpoint}: {e}");
                continue;
            }
        }
    }

    Err(format!(
        "statement store unreachable — tried {} endpoints",
        SS_ENDPOINTS.len()
    ))
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

/// Background poll loop — fetches new statements every 2 seconds.
fn poll_loop(running: Arc<AtomicBool>) {
    let epoca_topic = string_to_topic(EPOCA_TOPIC);
    // Dedup key = hash(decryption_key || channel || data). Value = priority.
    let mut seen: HashMap<[u8; 32], u32> = HashMap::new();
    let mut seen_order: Vec<[u8; 32]> = Vec::new();

    while running.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_secs(2));

        if !running.load(Ordering::Relaxed) {
            break;
        }

        match rpc_get_broadcasts(&[epoca_topic]) {
            Ok(statements) => {
                for stmt in statements {
                    // Dedup key includes namespace fields, not just data.
                    let dedup_input = [
                        stmt.decryption_key.unwrap_or([0; 32]).as_slice(),
                        stmt.channel.unwrap_or([0; 32]).as_slice(),
                        &stmt.data,
                    ]
                    .concat();
                    let hash = blake2b_256(&dedup_input);

                    // Skip if we've already seen this at equal or higher priority.
                    if let Some(&prev) = seen.get(&hash) {
                        if stmt.priority <= prev {
                            continue;
                        }
                    }
                    if !seen.contains_key(&hash) {
                        seen_order.push(hash);
                    }
                    seen.insert(hash, stmt.priority);

                    // Evict oldest half when cache exceeds limit.
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
            Err(e) => {
                log::warn!("[ss] poll failed: {e}");
            }
        }
    }

    log::info!("[ss] poll loop stopped");
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
}
