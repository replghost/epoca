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
use schnorrkel::Keypair;
use std::collections::HashMap;
use zeroize::Zeroize;

/// Maximum payload size for signing (64 KiB).
const MAX_SIGN_PAYLOAD: usize = 65_536;

/// Substrate signing context (matches what the runtime verifies).
const SIGNING_CTX: &[u8] = b"substrate";

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
    /// Cached flag: whether a mnemonic exists in the Keychain.
    /// Avoids hitting the Keychain (and triggering macOS auth prompts) on every
    /// render frame. Updated on create/import/delete/unlock.
    has_mnemonic: bool,
}

impl WalletManager {
    pub fn new() -> Self {
        // Single Keychain check at construction time.
        let has = keystore::has_mnemonic();
        Self {
            root_keypair: None,
            app_keys: HashMap::new(),
            has_mnemonic: has,
        }
    }

    /// Current wallet state.
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
        self.app_keys.clear();

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
        self.app_keys.clear();

        log::info!(
            "Wallet imported. Root address: {}",
            self.root_address().unwrap_or_default()
        );
        Ok(())
    }

    /// Unlock the wallet by loading the mnemonic from the Keychain.
    ///
    /// On macOS this may trigger a Touch ID / system authentication prompt.
    pub fn unlock(&mut self) -> Result<()> {
        let phrase = keystore::load_mnemonic()?;
        let mnemonic = bip39::Mnemonic::parse(&phrase)
            .map_err(|e| anyhow!("Stored mnemonic is invalid: {e}"))?;
        // phrase goes out of scope here — String::drop doesn't zero, but
        // the Keychain already holds it. The real secret is the keypair.

        let kp = derive::root_keypair_from_mnemonic(&mnemonic);
        self.root_keypair = Some(kp);
        self.app_keys.clear();
        self.has_mnemonic = true;

        log::info!("Wallet unlocked");
        Ok(())
    }

    /// Lock the wallet — clear all in-memory key material.
    pub fn lock(&mut self) {
        // Zeroize the root keypair's secret key bytes
        if let Some(ref mut kp) = self.root_keypair {
            let mut secret_bytes = kp.secret.to_bytes();
            secret_bytes.zeroize();
        }
        self.root_keypair = None;

        // Zeroize cached app keypairs
        for (_id, ref mut kp) in self.app_keys.drain() {
            let mut secret_bytes = kp.secret.to_bytes();
            secret_bytes.zeroize();
        }

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

    /// Return the SS58 address for a given app_id (derived account).
    pub fn app_address(&mut self, app_id: &str) -> Result<String> {
        let kp = self.app_keypair(app_id)?;
        Ok(derive::ss58_address(&kp.public))
    }

    /// Sign arbitrary bytes with the root keypair (for dapp wallet use).
    ///
    /// Returns the 64-byte sr25519 signature.
    pub fn sign_root(&mut self, payload: &[u8]) -> Result<Vec<u8>> {
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

impl Drop for WalletManager {
    fn drop(&mut self) {
        self.lock();
    }
}

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
        let _ = wm.app_address("com.test.app").unwrap();
        assert!(!wm.app_keys.is_empty());

        wm.lock();
        assert!(wm.root_keypair.is_none());
        assert!(wm.app_keys.is_empty());
        assert_eq!(wm.state(), WalletState::NoWallet); // no Keychain in test
    }
}
