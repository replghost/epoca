//! QR-based mobile wallet pairing over the Substrate Statement Store.
//!
//! The handshake:
//!   1. Host generates an X25519 ephemeral keypair and a 16-byte nonce.
//!   2. Derives a rendezvous topic = blake2b_256(nonce), encodes as QR URI.
//!   3. Subscribes to the rendezvous topic via injected Statement Store callback.
//!   4. Mobile wallet scans QR, posts encrypted response to the topic.
//!   5. Response: M_pk (32B) || chacha_nonce (12B) || ciphertext.
//!   6. Host decrypts with ChaCha20-Poly1305, parses { address, display_name }.
//!   7. Stores session_key + rendezvous in Keychain for future sign requests.
//!
//! This module is dependency-free with respect to epoca-core — Statement Store
//! operations are provided via injected callbacks in `PairingConfig`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blake2::digest::{consts::U32, Digest};
use chacha20poly1305::aead::{Aead, AeadCore};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use rand_core::{OsRng, RngCore};
use x25519_dalek::{EphemeralSecret, PublicKey};

/// Pairing handshake state, observed by the UI.
#[derive(Debug, Clone)]
pub enum PairingState {
    /// QR code ready to scan. Contains the URI to encode into the QR image.
    AwaitingScan { qr_uri: String },
    /// Mobile wallet responded — pairing established.
    Established {
        address: String,
        display_name: String,
    },
    /// Pairing failed or timed out.
    Failed(String),
}

/// Handle returned by `start_pairing`. Lets the caller receive state updates
/// and cancel the session if the user dismisses the pairing UI.
pub struct PairingSession {
    /// Receive state updates from the background thread.
    pub state_rx: mpsc::Receiver<PairingState>,
    /// Set to true to cancel the in-progress pairing.
    pub cancel: Arc<AtomicBool>,
}

/// Configuration for starting a pairing session.
///
/// Contains injected callbacks for Statement Store operations so this module
/// does not depend on `epoca-core`.
pub struct PairingConfig {
    /// Subscribe to a Statement Store channel (full channel name, not namespaced).
    ///
    /// Returns `(subscription_id, receiver)`. The receiver yields statements as
    /// `(author, data)` pairs where `data` is a hex-encoded string.
    pub subscribe: Box<
        dyn FnOnce(&str) -> Result<(u64, mpsc::Receiver<(String, String)>), String> + Send,
    >,
    /// Unsubscribe from a Statement Store channel using the subscription ID.
    pub unsubscribe: Box<dyn FnOnce(u64) + Send>,
    /// Network identifier embedded in the QR URI (e.g. `"people-paseo"`).
    pub network: String,
}

/// Start a new pairing session.
///
/// Spawns a background thread that runs the full handshake. Returns a
/// `PairingSession` handle immediately; callers poll `state_rx` for updates.
///
/// The background thread will emit exactly one terminal state
/// (`Established` or `Failed`) after emitting `AwaitingScan`.
pub fn start_pairing(config: PairingConfig) -> PairingSession {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();

    // Bounded channel: only a few states are ever sent.
    let (state_tx, state_rx) = mpsc::sync_channel::<PairingState>(8);

    std::thread::spawn(move || {
        run_pairing(config, cancel_clone, state_tx);
    });

    PairingSession { state_rx, cancel }
}

/// Core pairing logic — runs entirely on the background thread.
fn run_pairing(
    config: PairingConfig,
    cancel: Arc<AtomicBool>,
    state_tx: mpsc::SyncSender<PairingState>,
) {
    // Generate X25519 ephemeral keypair.
    let h_sk = EphemeralSecret::random_from_rng(OsRng);
    let h_pk = PublicKey::from(&h_sk);

    // 16-byte random nonce for the QR URI (not the ChaCha nonce).
    let mut qr_nonce = [0u8; 16];
    OsRng.fill_bytes(&mut qr_nonce);

    // Rendezvous topic = blake2b_256(qr_nonce).
    let rendezvous_bytes: [u8; 32] = {
        let mut hasher = blake2::Blake2b::<U32>::new();
        hasher.update(&qr_nonce);
        hasher.finalize().into()
    };
    let rendezvous_hex = hex::encode(rendezvous_bytes);

    // Unix timestamp in seconds (for replay-attack window on the mobile side).
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let qr_uri = format!(
        "polkadot://pair?pk={}&topic={}&network={}&ts={}",
        hex::encode(h_pk.as_bytes()),
        rendezvous_hex,
        config.network,
        ts,
    );

    // Emit AwaitingScan — UI should render the QR immediately.
    let _ = state_tx.send(PairingState::AwaitingScan {
        qr_uri: qr_uri.clone(),
    });
    log::info!("[pairing] QR ready, topic={}", &rendezvous_hex[..16]);

    // Subscribe to the rendezvous topic.
    let (sub_id, stmt_rx) = match (config.subscribe)(&rendezvous_hex) {
        Ok(pair) => pair,
        Err(e) => {
            let _ = state_tx.send(PairingState::Failed(format!("subscribe failed: {e}")));
            return;
        }
    };

    // Wait up to 120s (60 × 2s iterations).
    let result = wait_for_response(&stmt_rx, &cancel, 60, Duration::from_secs(2));

    // Always unsubscribe regardless of outcome.
    (config.unsubscribe)(sub_id);

    match result {
        Err(e) => {
            let _ = state_tx.send(PairingState::Failed(e));
        }
        Ok(raw_hex) => {
            match decrypt_response(h_sk, &raw_hex) {
                Err(e) => {
                    let _ = state_tx.send(PairingState::Failed(e));
                }
                Ok((address, display_name, session_key)) => {
                    // Store in Keychain.  Non-fatal on error — caller can retry.
                    if let Err(e) =
                        crate::keystore::store_paired_data(&address, &session_key, &rendezvous_bytes)
                    {
                        log::warn!("[pairing] Keychain store failed: {e}");
                        let _ = state_tx.send(PairingState::Failed(format!(
                            "Keychain store failed: {e}"
                        )));
                        return;
                    }
                    log::info!("[pairing] established with address={address}");
                    let _ = state_tx.send(PairingState::Established {
                        address,
                        display_name,
                    });
                }
            }
        }
    }
}

/// Poll the receiver until a statement arrives, cancellation is requested,
/// or the iteration limit is exhausted.
fn wait_for_response(
    rx: &mpsc::Receiver<(String, String)>,
    cancel: &Arc<AtomicBool>,
    max_iterations: u32,
    interval: Duration,
) -> Result<String, String> {
    for _ in 0..max_iterations {
        if cancel.load(Ordering::Relaxed) {
            return Err("pairing cancelled by user".into());
        }
        match rx.recv_timeout(interval) {
            Ok((_author, data)) => return Ok(data),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("subscription channel closed unexpectedly".into());
            }
        }
    }
    Err("pairing timed out after 120 seconds".into())
}

// ---------------------------------------------------------------------------
// Paired wallet sign request
// ---------------------------------------------------------------------------

/// Configuration for a paired sign request.
pub struct PairedSignConfig {
    /// Write a statement to the Statement Store.
    pub write: Box<dyn FnOnce(&str, &str) -> Result<(), String> + Send>,
    /// Subscribe to a channel. Returns (sub_id, receiver of (author, data)).
    pub subscribe: Box<dyn FnOnce(&str) -> Result<(u64, mpsc::Receiver<(String, String)>), String> + Send>,
    /// Unsubscribe.
    pub unsubscribe: Box<dyn FnOnce(u64) + Send>,
}

/// Send a sign request to the paired mobile wallet and wait for the response.
///
/// Spawns a background thread. The result (signature bytes or error string)
/// is sent via `result_tx` when the request completes or times out.
pub fn sign_via_paired(
    payload: Vec<u8>,
    config: PairedSignConfig,
    result_tx: mpsc::SyncSender<Result<Vec<u8>, String>>,
) {
    std::thread::spawn(move || {
        let result = run_paired_sign(payload, config);
        let _ = result_tx.send(result);
    });
}

/// Core paired-sign logic — runs on the background thread.
///
/// Protocol:
/// 1. Load session_key + rendezvous from Keychain.
/// 2. Generate a random request_id (16 hex chars).
/// 3. Build JSON: `{"request_id":"<id>","payload_hex":"<hex>","ts":<secs>}`.
/// 4. Encrypt with ChaCha20-Poly1305 using session_key, random 12-byte nonce.
/// 5. Hex-encode: nonce (12 B) || ciphertext and publish to `sign/<rendezvous_hex>`.
/// 6. Subscribe to `sign_response/<rendezvous_hex>`.
/// 7. Wait up to 60 s (30 × 2 s) for a response.
/// 8. Decrypt, verify request_id, hex-decode signature, return Ok(bytes).
fn run_paired_sign(payload: Vec<u8>, config: PairedSignConfig) -> Result<Vec<u8>, String> {
    // Step 1: load secrets from Keychain.
    let (_address, session_key, rendezvous) = crate::keystore::load_paired_data()
        .ok_or_else(|| "no paired wallet data — please re-pair".to_string())?;

    let rendezvous_hex = hex::encode(rendezvous);

    // Step 2: random request_id.
    let mut id_bytes = [0u8; 8];
    OsRng.fill_bytes(&mut id_bytes);
    let request_id = hex::encode(id_bytes); // 16 hex chars

    // Step 3: build plaintext JSON.
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let payload_hex = hex::encode(&payload);
    let plaintext = serde_json::json!({
        "request_id": request_id,
        "payload_hex": payload_hex,
        "ts": ts,
    })
    .to_string();

    // Step 4: encrypt with ChaCha20-Poly1305.
    let cipher = ChaCha20Poly1305::new_from_slice(&session_key)
        .map_err(|e| format!("cipher init failed: {e}"))?;
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| "encryption failed".to_string())?;

    // Step 5: hex-encode nonce || ciphertext and publish.
    let mut wire = nonce.to_vec();
    wire.extend_from_slice(&ciphertext);
    let wire_hex = hex::encode(&wire);
    let sign_channel = format!("sign/{rendezvous_hex}");
    (config.write)(&sign_channel, &wire_hex)?;

    // Step 6: subscribe to response channel.
    let response_channel = format!("sign_response/{rendezvous_hex}");
    let (sub_id, resp_rx) = (config.subscribe)(&response_channel)?;

    // Step 7: wait up to 60 s for a response.
    let result = wait_for_response(&resp_rx, &Arc::new(AtomicBool::new(false)), 30, Duration::from_secs(2));

    // Always unsubscribe.
    (config.unsubscribe)(sub_id);

    let raw_hex = result?;

    // Step 8: decrypt and verify.
    let raw = hex::decode(&raw_hex).map_err(|e| format!("response hex decode failed: {e}"))?;
    if raw.len() < 12 + 16 {
        return Err(format!("response too short: {} bytes", raw.len()));
    }

    let resp_nonce_bytes: [u8; 12] = raw[..12]
        .try_into()
        .map_err(|_| "response nonce slice error")?;
    let resp_nonce = chacha20poly1305::Nonce::from(resp_nonce_bytes);
    let plaintext_bytes = cipher
        .decrypt(&resp_nonce, &raw[12..])
        .map_err(|_| "response decryption failed (bad key or corrupted data)")?;

    let json: serde_json::Value = serde_json::from_slice(&plaintext_bytes)
        .map_err(|e| format!("response JSON parse failed: {e}"))?;

    let resp_id = json
        .get("request_id")
        .and_then(|v| v.as_str())
        .ok_or("missing 'request_id' in sign response")?;
    if resp_id != request_id {
        return Err(format!(
            "request_id mismatch: expected {request_id}, got {resp_id}"
        ));
    }

    let sig_hex_raw = json
        .get("signature_hex")
        .and_then(|v| v.as_str())
        .ok_or("missing 'signature_hex' in sign response")?;
    let sig_hex = sig_hex_raw.strip_prefix("0x").unwrap_or(sig_hex_raw);

    let sig_bytes = hex::decode(sig_hex).map_err(|e| format!("signature hex decode failed: {e}"))?;
    Ok(sig_bytes)
}

/// Decode and decrypt the mobile wallet's response.
///
/// Wire format (hex-encoded in Statement Store):
///   bytes 0..32   — mobile X25519 public key (M_pk)
///   bytes 32..44  — ChaCha20-Poly1305 nonce (12 bytes)
///   bytes 44..    — ciphertext (JSON payload + 16-byte Poly1305 tag)
///
/// Returns `(address, display_name, session_key)` on success.
fn decrypt_response(
    h_sk: EphemeralSecret,
    raw_hex: &str,
) -> Result<(String, String, [u8; 32]), String> {
    let raw = hex::decode(raw_hex).map_err(|e| format!("hex decode failed: {e}"))?;

    if raw.len() < 44 + 16 {
        return Err(format!(
            "response too short: {} bytes (need at least 60)",
            raw.len()
        ));
    }

    // Parse M_pk.
    let m_pk_bytes: [u8; 32] = raw[..32]
        .try_into()
        .map_err(|_| "M_pk slice error")?;
    let m_pk = x25519_dalek::PublicKey::from(m_pk_bytes);

    // ChaCha nonce.
    let chacha_nonce_bytes: [u8; 12] = raw[32..44]
        .try_into()
        .map_err(|_| "nonce slice error")?;
    let nonce = chacha20poly1305::Nonce::from(chacha_nonce_bytes);

    // Shared key via X25519 DH.
    let shared = h_sk.diffie_hellman(&m_pk);
    let session_key: [u8; 32] = *shared.as_bytes();

    // Decrypt.
    let cipher = ChaCha20Poly1305::new_from_slice(&session_key)
        .map_err(|e| format!("cipher init failed: {e}"))?;
    let plaintext = cipher
        .decrypt(&nonce, &raw[44..])
        .map_err(|_| "decryption failed (bad key or corrupted data)")?;

    // Parse JSON payload.
    let json: serde_json::Value =
        serde_json::from_slice(&plaintext).map_err(|e| format!("JSON parse failed: {e}"))?;

    let address = json
        .get("address")
        .and_then(|v| v.as_str())
        .ok_or("missing 'address' field in pairing payload")?
        .to_string();

    let display_name = json
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if address.is_empty() {
        return Err("pairing payload contains empty address".into());
    }

    Ok((address, display_name, session_key))
}
