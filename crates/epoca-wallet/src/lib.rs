//! Epoca wallet — sr25519 key management with macOS Keychain storage.
//!
//! The wallet holds a BIP-39 mnemonic and derives per-app sr25519 keypairs
//! using Substrate-compatible hard derivation (`//epoca//app//<app_id>`).
//!
//! Private key material never leaves this crate. SPAs interact through
//! `get_address(app_id)` and `sign(app_id, payload)`.

pub mod derive;
pub mod keystore;

use anyhow::{anyhow, Result};
use k256::ecdsa::SigningKey;
use schnorrkel::Keypair;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};


/// Flag set by the macOS sleep notification observer.
/// `WalletManager::check_auto_lock` checks and clears this.
static SYSTEM_SLEEP: AtomicBool = AtomicBool::new(false);

/// Maximum payload size for signing (64 KiB).
const MAX_SIGN_PAYLOAD: usize = 65_536;

/// Substrate signing context (matches what the runtime verifies).
const SIGNING_CTX: &[u8] = b"substrate";

/// Default auto-lock timeout: 15 minutes of inactivity.
const DEFAULT_AUTO_LOCK_SECS: u64 = 15 * 60;

/// The wallet's current state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletState {
    /// No mnemonic exists — user needs to create or import.
    NoWallet,
    /// Mnemonic exists but wallet is locked (keypair not in memory).
    Locked,
    /// Wallet is unlocked — keypair loaded, ready to sign.
    Unlocked {
        /// The root SS58 address (for display in Settings).
        root_address: String,
    },
}

/// Wallet manager — holds in-memory key material when unlocked.
///
/// Create one instance and store it in a GPUI global (`WalletGlobal`).
pub struct WalletManager {
    /// Root keypair — only present when unlocked. Zeroed on lock.
    root_keypair: Option<Keypair>,
    /// Cached per-app keypairs — cleared on lock.
    app_keys: HashMap<String, Keypair>,
    /// ETH secp256k1 signing key (BIP-44 m/44'/60'/0'/0/0). ZeroizeOnDrop.
    eth_key: Option<SigningKey>,
    /// BTC secp256k1 signing key (BIP-44 m/44'/0'/0'/0/0). ZeroizeOnDrop.
    btc_key: Option<SigningKey>,
    /// Cached flag: whether a mnemonic exists in the Keychain.
    /// Avoids hitting the Keychain (and triggering macOS auth prompts) on every
    /// render frame. Updated on create/import/delete/unlock.
    has_mnemonic: bool,
    /// Last time the wallet was used (unlock, sign, get address).
    /// Auto-lock fires when `Instant::now() - last_activity > auto_lock_timeout`.
    last_activity: Option<Instant>,
    /// Auto-lock timeout. `None` = never auto-lock.
    auto_lock_timeout: Option<Duration>,
}

impl WalletManager {
    pub fn new() -> Self {
        // Single Keychain check at construction time.
        let has = keystore::has_mnemonic();
        Self {
            root_keypair: None,
            app_keys: HashMap::new(),
            eth_key: None,
            btc_key: None,
            has_mnemonic: has,
            last_activity: None,
            auto_lock_timeout: Some(Duration::from_secs(DEFAULT_AUTO_LOCK_SECS)),
        }
    }

    /// Current wallet state (read-only — does not advance the state machine).
    /// Call `tick()` once per frame to check auto-lock before reading state.
    pub fn state(&self) -> WalletState {
        if self.root_keypair.is_some() {
            WalletState::Unlocked {
                root_address: self.root_address().unwrap_or_default(),
            }
        } else if self.has_mnemonic {
            WalletState::Locked
        } else {
            WalletState::NoWallet
        }
    }

    /// Advance the state machine — check auto-lock timeout and system sleep.
    /// Call once per render frame from the workbench tick loop. Returns `true`
    /// if the wallet was locked (caller should `cx.notify()`).
    pub fn tick(&mut self) -> bool {
        self.check_auto_lock()
    }

    /// Check if the auto-lock timeout has elapsed or system slept, and lock if so.
    /// Returns `true` if the wallet was locked.
    fn check_auto_lock(&mut self) -> bool {
        if self.root_keypair.is_none() {
            // Still clear a stale sleep flag
            SYSTEM_SLEEP.swap(false, Ordering::Relaxed);
            return false;
        }
        if SYSTEM_SLEEP.swap(false, Ordering::Relaxed) {
            log::info!("Wallet locked due to system sleep");
            self.lock();
            return true;
        }
        if let (Some(timeout), Some(last)) = (self.auto_lock_timeout, self.last_activity) {
            if last.elapsed() > timeout {
                log::info!("Wallet auto-locked after {} seconds of inactivity", timeout.as_secs());
                self.lock();
                return true;
            }
        }
        false
    }

    /// Record wallet activity (resets the auto-lock timer).
    fn touch(&mut self) {
        self.last_activity = Some(Instant::now());
    }

    /// Create a new wallet with a fresh 12-word mnemonic.
    ///
    /// Returns the mnemonic phrase so the user can write it down.
    /// The wallet is automatically unlocked after creation.
    pub fn create(&mut self) -> Result<String> {
        let mnemonic = bip39::Mnemonic::generate(12)
            .map_err(|e| anyhow!("Mnemonic generation failed: {e}"))?;
        let phrase = mnemonic.to_string();

        keystore::store_mnemonic(&phrase)?;
        self.has_mnemonic = true;

        let kp = derive::root_keypair_from_mnemonic(&mnemonic);
        self.root_keypair = Some(kp);
        self.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));
        self.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));
        self.app_keys.clear();
        self.touch();

        log::info!(
            "Wallet created. Root address: {}",
            self.root_address().unwrap_or_default()
        );
        Ok(phrase)
    }

    /// Import an existing mnemonic phrase.
    ///
    /// The wallet is automatically unlocked after import.
    pub fn import(&mut self, phrase: &str) -> Result<()> {
        let mnemonic = bip39::Mnemonic::parse(phrase)
            .map_err(|e| anyhow!("Invalid mnemonic: {e}"))?;

        keystore::store_mnemonic(phrase)?;
        self.has_mnemonic = true;

        let kp = derive::root_keypair_from_mnemonic(&mnemonic);
        self.root_keypair = Some(kp);
        self.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));
        self.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));
        self.app_keys.clear();
        self.touch();

        log::info!(
            "Wallet imported. Root address: {}",
            self.root_address().unwrap_or_default()
        );
        Ok(())
    }

    /// Unlock the wallet by loading the mnemonic from the Keychain.
    ///
    /// On macOS this triggers a Touch ID prompt (or passcode fallback).
    pub fn unlock(&mut self) -> Result<()> {
        let phrase = zeroize::Zeroizing::new(keystore::load_mnemonic()?);
        let mnemonic = bip39::Mnemonic::parse(phrase.as_str())
            .map_err(|e| anyhow!("Stored mnemonic is invalid: {e}"))?;

        let kp = derive::root_keypair_from_mnemonic(&mnemonic);
        self.root_keypair = Some(kp);
        self.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));
        self.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));
        self.app_keys.clear();
        self.touch();
        // Set has_mnemonic after parse succeeded — if parse fails above,
        // the flag stays unchanged so state() doesn't report Locked for a
        // corrupted mnemonic.
        self.has_mnemonic = true;

        log::info!("Wallet unlocked");
        Ok(())
    }

    /// Lock the wallet — clear all in-memory key material.
    /// schnorrkel's `Keypair` and `SecretKey` both implement `ZeroizeOnDrop`,
    /// so dropping them zeros the secret bytes in place.
    pub fn lock(&mut self) {
        self.root_keypair = None;
        self.app_keys.clear();
        self.eth_key = None;
        self.btc_key = None;
        self.last_activity = None;
        log::info!("Wallet locked");
    }

    /// Return the root SS58 address (for display in Settings).
    pub fn root_address(&self) -> Result<String> {
        let kp = self
            .root_keypair
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;
        Ok(derive::ss58_address(&kp.public))
    }

    /// Return the checksummed EIP-55 Ethereum address.
    pub fn eth_address(&self) -> Result<String> {
        let key = self
            .eth_key
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;
        Ok(derive::secp256k1::eth_address(key))
    }

    /// Return the P2WPKH (bech32) Bitcoin mainnet address.
    pub fn btc_address(&self) -> Result<String> {
        let key = self
            .btc_key
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;
        Ok(derive::secp256k1::btc_address_p2wpkh(key))
    }

    /// Sign arbitrary bytes with the ETH private key (EIP-191 personal_sign).
    /// Returns 65-byte signature: `r (32) || s (32) || v (1)`.
    pub fn eth_sign_personal(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.touch();
        if data.len() > MAX_SIGN_PAYLOAD {
            return Err(anyhow!(
                "Payload too large ({} bytes, max {})",
                data.len(),
                MAX_SIGN_PAYLOAD,
            ));
        }

        let key = self
            .eth_key
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;

        // EIP-191 personal_sign prefix
        use sha3::{Digest, Keccak256};
        let prefix = format!("\x19Ethereum Signed Message:\n{}", data.len());
        let mut hasher = Keccak256::new();
        hasher.update(prefix.as_bytes());
        hasher.update(data);
        let digest = hasher.finalize();

        let (sig, recid) = key
            .sign_prehash_recoverable(&digest)
            .map_err(|e| anyhow!("ETH signing failed: {e}"))?;
        let mut out = Vec::with_capacity(65);
        out.extend_from_slice(&sig.to_bytes());
        out.push(recid.to_byte() + 27); // Ethereum v = recid + 27
        Ok(out)
    }

    /// Sign a raw 32-byte digest with the BTC private key (ECDSA, no prefix).
    /// The caller is responsible for hashing: BTC transactions use SHA256d
    /// (double-SHA256 of the serialized transaction).
    /// Returns the DER-encoded signature.
    pub fn btc_sign_raw(&mut self, digest: &[u8; 32]) -> Result<Vec<u8>> {
        self.touch();
        let key = self
            .btc_key
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;

        let (sig, _recid) = key
            .sign_prehash_recoverable(digest)
            .map_err(|e| anyhow!("BTC signing failed: {e}"))?;
        Ok(sig.to_der().to_bytes().to_vec())
    }

    /// Sign a UTF-8 message using BIP-137 (Bitcoin Signed Message).
    /// Returns a 65-byte recoverable signature encoded as Base64.
    /// Digest: SHA256d of `"\x18Bitcoin Signed Message:\n" + varint(len) + msg`.
    /// Header byte: 39 + recid (native SegWit P2WPKH, Electrum convention).
    pub fn btc_sign_message(&mut self, msg: &[u8]) -> Result<String> {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use sha2::{Digest, Sha256};

        self.touch();
        if msg.len() > MAX_SIGN_PAYLOAD {
            return Err(anyhow!(
                "Message too large ({} bytes, max {})",
                msg.len(),
                MAX_SIGN_PAYLOAD,
            ));
        }
        let key = self
            .btc_key
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;

        // BIP-137 message prefix
        let prefix = b"\x18Bitcoin Signed Message:\n";
        let msg_len = encode_varint(msg.len() as u64);
        let mut preimage = Vec::with_capacity(prefix.len() + msg_len.len() + msg.len());
        preimage.extend_from_slice(prefix);
        preimage.extend_from_slice(&msg_len);
        preimage.extend_from_slice(msg);

        // SHA256d (double SHA-256)
        let digest: [u8; 32] = Sha256::digest(Sha256::digest(&preimage)).into();

        let (sig, recid) = key
            .sign_prehash_recoverable(&digest)
            .map_err(|e| anyhow!("BTC signing failed: {e}"))?;

        // Header: 39 + recid for native SegWit P2WPKH (BIP-84)
        let header = 39u8 + recid.to_byte();
        let mut out = Vec::with_capacity(65);
        out.push(header);
        out.extend_from_slice(&sig.to_bytes());
        Ok(STANDARD.encode(&out))
    }

    /// Return the SS58 address for a given app_id (derived account).
    pub fn app_address(&mut self, app_id: &str) -> Result<String> {
        self.touch();
        let kp = self.app_keypair(app_id)?;
        Ok(derive::ss58_address(&kp.public))
    }

    /// Sign arbitrary bytes with the root keypair (for dapp wallet use).
    ///
    /// Returns the 64-byte sr25519 signature.
    pub fn sign_root(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
        self.touch();
        if payload.len() > MAX_SIGN_PAYLOAD {
            return Err(anyhow!(
                "Payload too large ({} bytes, max {})",
                payload.len(),
                MAX_SIGN_PAYLOAD,
            ));
        }

        let kp = self
            .root_keypair
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;
        let ctx = schnorrkel::signing_context(SIGNING_CTX);
        let sig = kp.sign(ctx.bytes(payload));
        Ok(sig.to_bytes().to_vec())
    }

    /// Sign arbitrary bytes as the given app's derived account.
    ///
    /// Returns the 64-byte sr25519 signature.
    pub fn sign(&mut self, app_id: &str, payload: &[u8]) -> Result<Vec<u8>> {
        self.touch();
        if payload.len() > MAX_SIGN_PAYLOAD {
            return Err(anyhow!(
                "Payload too large ({} bytes, max {})",
                payload.len(),
                MAX_SIGN_PAYLOAD,
            ));
        }

        let kp = self.app_keypair(app_id)?;
        let ctx = schnorrkel::signing_context(SIGNING_CTX);
        let sig = kp.sign(ctx.bytes(payload));
        Ok(sig.to_bytes().to_vec())
    }

    /// Delete the wallet entirely — removes the mnemonic from Keychain.
    pub fn delete(&mut self) -> Result<()> {
        self.lock();
        keystore::delete_mnemonic()?;
        self.has_mnemonic = false;
        log::info!("Wallet deleted");
        Ok(())
    }

    /// Get or derive the keypair for a given app_id.
    fn app_keypair(&mut self, app_id: &str) -> Result<&Keypair> {
        let root = self
            .root_keypair
            .as_ref()
            .ok_or_else(|| anyhow!("Wallet is locked"))?;

        if !self.app_keys.contains_key(app_id) {
            let kp = derive::app_keypair(root, app_id);
            self.app_keys.insert(app_id.to_string(), kp);
        }

        Ok(self.app_keys.get(app_id).unwrap())
    }
}

/// Bitcoin varint encoding (CompactSize).
fn encode_varint(n: u64) -> Vec<u8> {
    if n < 0xfd {
        vec![n as u8]
    } else if n <= 0xffff {
        let mut v = vec![0xfd];
        v.extend_from_slice(&(n as u16).to_le_bytes());
        v
    } else if n <= 0xffff_ffff {
        let mut v = vec![0xfe];
        v.extend_from_slice(&(n as u32).to_le_bytes());
        v
    } else {
        let mut v = vec![0xff];
        v.extend_from_slice(&n.to_le_bytes());
        v
    }
}

impl Drop for WalletManager {
    fn drop(&mut self) {
        self.lock();
    }
}

/// Register a macOS observer that sets the `SYSTEM_SLEEP` flag when the
/// system sleeps or the screen locks. Call once at app startup.
/// `WalletManager::check_auto_lock` (called from `state()`) reads and
/// clears this flag, triggering an immediate lock.
#[cfg(target_os = "macos")]
pub fn register_sleep_observer() {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder};
    use std::sync::OnceLock;

    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        unsafe {
            // Build a tiny ObjC class with a callback method
            static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
            let cls = CLASS.get_or_init(|| {
                if let Some(c) = AnyClass::get("EpocaSleepObserver") {
                    return c;
                }
                let superclass = AnyClass::get("NSObject").unwrap();
                let mut builder = ClassBuilder::new("EpocaSleepObserver", superclass).unwrap();

                unsafe extern "C" fn on_sleep(
                    _this: *mut AnyObject,
                    _sel: objc2::runtime::Sel,
                    _note: *mut AnyObject,
                ) {
                    SYSTEM_SLEEP.store(true, Ordering::Relaxed);
                }
                builder.add_method(
                    objc2::sel!(onSleep:),
                    on_sleep as unsafe extern "C" fn(_, _, _),
                );
                builder.register()
            });

            let observer: *mut AnyObject = msg_send![*cls, new];
            // Prevent deallocation — leaked intentionally for process lifetime.
            let _: *mut AnyObject = msg_send![observer, retain];

            // NSWorkspace.sharedWorkspace
            let ws_cls = AnyClass::get("NSWorkspace").unwrap();
            let shared_ws: *mut AnyObject = msg_send![ws_cls, sharedWorkspace];
            let nc: *mut AnyObject = msg_send![shared_ws, notificationCenter];

            // Register for NSWorkspaceWillSleepNotification
            let sleep_name: *mut AnyObject = msg_send![
                AnyClass::get("NSString").unwrap(),
                stringWithUTF8String: b"NSWorkspaceWillSleepNotification\0".as_ptr() as *const i8
            ];
            let _: () = msg_send![
                nc,
                addObserver: observer
                selector: objc2::sel!(onSleep:)
                name: sleep_name
                object: std::ptr::null::<AnyObject>()
            ];

            // Register for NSWorkspaceScreensDidSleepNotification (screen lock)
            let screen_sleep_name: *mut AnyObject = msg_send![
                AnyClass::get("NSString").unwrap(),
                stringWithUTF8String: b"NSWorkspaceScreensDidSleepNotification\0".as_ptr() as *const i8
            ];
            let _: () = msg_send![
                nc,
                addObserver: observer
                selector: objc2::sel!(onSleep:)
                name: screen_sleep_name
                object: std::ptr::null::<AnyObject>()
            ];
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn register_sleep_observer() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_address() {
        let mut wm = WalletManager::new();
        // Import a known mnemonic (can't test create() without Keychain)
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let kp = derive::root_keypair_from_mnemonic(&mnemonic);
        wm.root_keypair = Some(kp);

        let root_addr = wm.root_address().unwrap();
        assert!(root_addr.starts_with('5'));

        let app_addr = wm.app_address("com.test.app").unwrap();
        assert!(app_addr.starts_with('5'));
        assert_ne!(root_addr, app_addr);
    }

    #[test]
    fn sign_and_verify() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));

        let sig_bytes = wm.sign("com.test.app", b"hello world").unwrap();
        assert_eq!(sig_bytes.len(), 64);

        // Verify the signature
        let kp = derive::app_keypair(wm.root_keypair.as_ref().unwrap(), "com.test.app");
        let sig = schnorrkel::Signature::from_bytes(&sig_bytes).unwrap();
        let ctx = schnorrkel::signing_context(SIGNING_CTX);
        assert!(kp.public.verify(ctx.bytes(b"hello world"), &sig).is_ok());
    }

    #[test]
    fn payload_too_large() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));

        let big = vec![0u8; MAX_SIGN_PAYLOAD + 1];
        assert!(wm.sign("com.test.app", &big).is_err());
    }

    #[test]
    fn lock_clears_keys() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));
        let _ = wm.app_address("com.test.app").unwrap();
        assert!(!wm.app_keys.is_empty());

        wm.lock();
        assert!(wm.root_keypair.is_none());
        assert!(wm.app_keys.is_empty());
        assert!(wm.eth_key.is_none());
        assert!(wm.btc_key.is_none());
        // After lock, state is either NoWallet (no keychain) or Locked (keychain present)
        assert!(!matches!(wm.state(), WalletState::Unlocked { .. }));
    }

    #[test]
    fn eth_address_when_unlocked() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let eth = wm.eth_address().unwrap();
        assert!(eth.starts_with("0x"));
        assert_eq!(eth.len(), 42);

        let btc = wm.btc_address().unwrap();
        assert!(btc.starts_with("bc1q"));
    }

    #[test]
    fn eth_address_fails_when_locked() {
        let wm = WalletManager::new();
        assert!(wm.eth_address().is_err());
        assert!(wm.btc_address().is_err());
    }

    #[test]
    fn eth_sign_personal_produces_65_bytes() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));

        let sig = wm.eth_sign_personal(b"hello ethereum").unwrap();
        assert_eq!(sig.len(), 65, "ETH personal_sign must be 65 bytes (r+s+v)");
        // v should be 27 or 28
        assert!(sig[64] == 27 || sig[64] == 28, "v must be 27 or 28, got {}", sig[64]);
    }

    #[test]
    fn eth_sign_personal_recovers_to_correct_address() {
        use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
        use sha3::{Digest, Keccak256};

        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.eth_key = Some(derive::secp256k1::eth_key(&mnemonic));

        let data = b"hello ethereum";
        let sig_bytes = wm.eth_sign_personal(data).unwrap();

        // Re-hash with EIP-191 prefix
        let prefix = format!("\x19Ethereum Signed Message:\n{}", data.len());
        let digest = Keccak256::new()
            .chain_update(prefix.as_bytes())
            .chain_update(data)
            .finalize();

        let sig = Signature::from_slice(&sig_bytes[..64]).unwrap();
        let recid = RecoveryId::from_byte(sig_bytes[64] - 27).unwrap();
        let recovered = VerifyingKey::recover_from_prehash(&digest, &sig, recid).unwrap();

        let expected_addr = wm.eth_address().unwrap();
        let recovered_addr = derive::secp256k1::eth_address_from_verifying_key(&recovered);
        assert_eq!(recovered_addr, expected_addr, "Recovered signer must match wallet ETH address");
    }

    #[test]
    fn encode_varint_boundary_values() {
        assert_eq!(encode_varint(0), vec![0]);
        assert_eq!(encode_varint(1), vec![1]);
        assert_eq!(encode_varint(0xfc), vec![0xfc]);
        assert_eq!(encode_varint(0xfd), vec![0xfd, 0xfd, 0x00]);
        assert_eq!(encode_varint(0xffff), vec![0xfd, 0xff, 0xff]);
        assert_eq!(encode_varint(0x10000), vec![0xfe, 0x00, 0x00, 0x01, 0x00]);
        assert_eq!(
            encode_varint(0x1_0000_0000),
            vec![0xff, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn btc_sign_message_produces_base64_65_bytes() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let b64 = wm.btc_sign_message(b"Hello Bitcoin!").unwrap();
        let raw = STANDARD.decode(&b64).unwrap();
        assert_eq!(raw.len(), 65, "BIP-137 sig must be 65 bytes");
        // Header byte for native SegWit P2WPKH: 39..42
        assert!(
            (39..=42).contains(&raw[0]),
            "header must be 39+recid, got {}",
            raw[0]
        );
    }

    #[test]
    fn btc_sign_message_recovers_to_correct_key() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
        use sha2::{Digest, Sha256};

        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let msg = b"test message for recovery";
        let b64 = wm.btc_sign_message(msg).unwrap();
        let raw = STANDARD.decode(&b64).unwrap();

        // Reconstruct the digest
        let prefix = b"\x18Bitcoin Signed Message:\n";
        let msg_len = encode_varint(msg.len() as u64);
        let mut preimage = Vec::new();
        preimage.extend_from_slice(prefix);
        preimage.extend_from_slice(&msg_len);
        preimage.extend_from_slice(msg);
        let digest: [u8; 32] = Sha256::digest(Sha256::digest(&preimage)).into();

        let recid = RecoveryId::from_byte(raw[0] - 39).unwrap();
        let sig = Signature::from_slice(&raw[1..]).unwrap();
        let recovered = VerifyingKey::recover_from_prehash(&digest, &sig, recid).unwrap();

        let expected_vk = VerifyingKey::from(wm.btc_key.as_ref().unwrap());
        assert_eq!(recovered, expected_vk, "Recovered key must match BTC key");
    }

    #[test]
    fn btc_sign_message_fails_when_locked() {
        let mut wm = WalletManager::new();
        assert!(wm.btc_sign_message(b"test").is_err());
    }

    #[test]
    fn btc_sign_message_rejects_oversized() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let big = vec![0u8; MAX_SIGN_PAYLOAD + 1];
        assert!(wm.btc_sign_message(&big).is_err());
    }

    #[test]
    fn btc_sign_message_deterministic() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let sig1 = wm.btc_sign_message(b"deterministic test").unwrap();
        let sig2 = wm.btc_sign_message(b"deterministic test").unwrap();
        assert_eq!(sig1, sig2, "RFC 6979 nonces must produce identical signatures");
    }

    #[test]
    fn btc_sign_message_empty_string() {
        use base64::{engine::general_purpose::STANDARD, Engine};

        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let b64 = wm.btc_sign_message(b"").unwrap();
        let raw = STANDARD.decode(&b64).unwrap();
        assert_eq!(raw.len(), 65);
    }

    #[test]
    fn btc_sign_raw_produces_der() {
        let mut wm = WalletManager::new();
        let mnemonic = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        wm.root_keypair = Some(derive::root_keypair_from_mnemonic(&mnemonic));
        wm.btc_key = Some(derive::secp256k1::btc_key(&mnemonic));

        let digest = [0x42u8; 32];
        let sig = wm.btc_sign_raw(&digest).unwrap();
        // DER-encoded ECDSA signatures are 70-72 bytes typically
        assert!(sig.len() >= 68 && sig.len() <= 73, "DER sig len: {}", sig.len());
        // DER starts with 0x30
        assert_eq!(sig[0], 0x30, "DER encoding must start with 0x30");
    }
}
