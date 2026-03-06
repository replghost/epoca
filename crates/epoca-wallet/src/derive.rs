//! Key derivation: mnemonic → root sr25519 keypair → per-app derived keypair.
//!
//! Uses Substrate-compatible hard derivation (`//epoca//app//<app_id>`) so that
//! a compromised child key cannot reconstruct sibling or parent keys.

use blake2::digest::{consts::U32, Digest};
use schnorrkel::{ExpansionMode, MiniSecretKey, SecretKey, Keypair, PublicKey};
use zeroize::Zeroize;

/// SS58 prefix byte for the "generic" Substrate network (prefix 42).
const SS58_PREFIX: u8 = 42;

/// Derive the root sr25519 keypair from a BIP-39 mnemonic.
///
/// The mnemonic is converted to a 64-byte seed via PBKDF2 (BIP-39 standard),
/// then the first 32 bytes are used as a `MiniSecretKey`.
pub fn root_keypair_from_mnemonic(mnemonic: &bip39::Mnemonic) -> Keypair {
    let seed = mnemonic.to_seed("");
    let mut mini_bytes = [0u8; 32];
    mini_bytes.copy_from_slice(&seed[..32]);
    let mini = MiniSecretKey::from_bytes(&mini_bytes).expect("32 bytes is valid");
    mini_bytes.zeroize();
    let secret = mini.expand(ExpansionMode::Ed25519);
    secret.to_keypair()
}

/// Derive a per-app keypair from the root keypair using hard derivation.
///
/// Path: `//epoca//app//<app_id>` — three hard junctions.
/// Each junction hashes the parent secret key with the junction label.
pub fn app_keypair(root: &Keypair, app_id: &str) -> Keypair {
    let k1 = hard_derive(&root.secret, b"epoca");
    let k2 = hard_derive(&k1, b"app");
    let k3 = hard_derive(&k2, app_id.as_bytes());
    k3.to_keypair()
}

/// Substrate-compatible hard key derivation.
///
/// Computes `BLAKE2b-256(secret_key_bytes || junction_bytes)` to produce
/// a new 32-byte `MiniSecretKey`, then expands it.
fn hard_derive(parent: &SecretKey, junction: &[u8]) -> SecretKey {
    let mut hasher = blake2::Blake2b::<U32>::new();
    hasher.update(b"SchnsHrd"); // Substrate HDKD magic prefix
    hasher.update(&parent.to_bytes());
    hasher.update(junction);
    let hash = hasher.finalize();
    let mut mini_bytes = [0u8; 32];
    mini_bytes.copy_from_slice(&hash);
    let mini = MiniSecretKey::from_bytes(&mini_bytes).expect("32 bytes is valid");
    mini_bytes.zeroize();
    mini.expand(ExpansionMode::Ed25519)
}

/// Encode an sr25519 public key as an SS58 address string.
///
/// Format: `base58check(prefix || pubkey || checksum[0..2])`
/// where checksum = `BLAKE2b-512(b"SS58PRE" || prefix || pubkey)`.
pub fn ss58_address(pubkey: &PublicKey) -> String {
    let pubkey_bytes = pubkey.to_bytes();
    let mut payload = Vec::with_capacity(35);
    payload.push(SS58_PREFIX);
    payload.extend_from_slice(&pubkey_bytes);

    // Compute checksum
    let mut hasher = blake2::Blake2b::<blake2::digest::consts::U64>::new();
    hasher.update(b"SS58PRE");
    hasher.update(&payload);
    let checksum = hasher.finalize();
    payload.extend_from_slice(&checksum[..2]);

    bs58::encode(payload).into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_root_derivation() {
        let m1 = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let kp1 = root_keypair_from_mnemonic(&m1);
        let kp2 = root_keypair_from_mnemonic(&m1);
        assert_eq!(kp1.public.to_bytes(), kp2.public.to_bytes());
    }

    #[test]
    fn deterministic_app_derivation() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let root = root_keypair_from_mnemonic(&m);
        let app1a = app_keypair(&root, "com.test.app1");
        let app1b = app_keypair(&root, "com.test.app1");
        let app2 = app_keypair(&root, "com.test.app2");

        // Same app_id → same key
        assert_eq!(app1a.public.to_bytes(), app1b.public.to_bytes());
        // Different app_id → different key
        assert_ne!(app1a.public.to_bytes(), app2.public.to_bytes());
        // App key differs from root
        assert_ne!(app1a.public.to_bytes(), root.public.to_bytes());
    }

    #[test]
    fn ss58_address_format() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let root = root_keypair_from_mnemonic(&m);
        let addr = ss58_address(&root.public);
        // SS58 addresses start with '5' for prefix 42
        assert!(addr.starts_with('5'), "got: {addr}");
        // Should be 47-48 chars
        assert!(addr.len() >= 46 && addr.len() <= 50, "len: {}", addr.len());
    }

    #[test]
    fn ss58_roundtrip_valid() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let root = root_keypair_from_mnemonic(&m);
        let addr = ss58_address(&root.public);
        // Decode and verify prefix + pubkey
        let decoded = bs58::decode(&addr).into_vec().unwrap();
        assert_eq!(decoded[0], 42); // generic prefix
        assert_eq!(&decoded[1..33], &root.public.to_bytes());
    }
}
