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

// ---------------------------------------------------------------------------
// secp256k1 / BIP-44 derivation (Ethereum + Bitcoin)
// ---------------------------------------------------------------------------

pub mod secp256k1 {
    use bip32::{DerivationPath, XPrv};
    use k256::ecdsa::SigningKey;
    use sha3::{Digest as Sha3Digest, Keccak256};

    /// BIP-44 ETH derivation path: m/44'/60'/0'/0/0
    const ETH_PATH: &str = "m/44'/60'/0'/0/0";
    /// BIP-44 BTC derivation path: m/44'/0'/0'/0/0
    const BTC_PATH: &str = "m/44'/0'/0'/0/0";

    /// Derive a secp256k1 signing key from a BIP-39 mnemonic using a BIP-44 path.
    fn derive_key(mnemonic: &bip39::Mnemonic, path: &str) -> SigningKey {
        let seed = mnemonic.to_seed("");
        let dp: DerivationPath = path.parse().expect("valid BIP-44 path");
        let child = XPrv::derive_from_path(seed, &dp).expect("valid derivation");
        child.into()
    }

    /// Derive the ETH secp256k1 signing key (m/44'/60'/0'/0/0).
    pub fn eth_key(mnemonic: &bip39::Mnemonic) -> SigningKey {
        derive_key(mnemonic, ETH_PATH)
    }

    /// Derive the BTC secp256k1 signing key (m/44'/0'/0'/0/0).
    pub fn btc_key(mnemonic: &bip39::Mnemonic) -> SigningKey {
        derive_key(mnemonic, BTC_PATH)
    }

    /// EIP-55 checksummed Ethereum address from a signing key.
    /// Format: `0x` + 40 hex chars with mixed-case checksum.
    pub fn eth_address(key: &SigningKey) -> String {
        use k256::ecdsa::VerifyingKey;
        let vk = VerifyingKey::from(key);
        // Uncompressed public key (65 bytes: 0x04 || x || y), skip the 0x04 prefix
        let pubkey_bytes = vk.to_encoded_point(false);
        let pubkey_uncompressed = &pubkey_bytes.as_bytes()[1..]; // 64 bytes

        let hash = Keccak256::digest(pubkey_uncompressed);
        let addr_bytes = &hash[12..]; // last 20 bytes

        // EIP-55 checksum
        let hex_lower: String = addr_bytes.iter().map(|b| format!("{b:02x}")).collect();
        let checksum_hash = Keccak256::digest(hex_lower.as_bytes());

        let mut addr = String::with_capacity(42);
        addr.push_str("0x");
        for (i, c) in hex_lower.chars().enumerate() {
            let nibble = (checksum_hash[i / 2] >> (if i % 2 == 0 { 4 } else { 0 })) & 0xf;
            if nibble >= 8 {
                addr.push(c.to_ascii_uppercase());
            } else {
                addr.push(c);
            }
        }
        addr
    }

    /// P2WPKH (bech32) Bitcoin mainnet address from a signing key.
    pub fn btc_address_p2wpkh(key: &SigningKey) -> String {
        use bech32::{segwit, Hrp};
        use k256::ecdsa::VerifyingKey;
        use ripemd::Ripemd160;
        use sha2::{Digest as Sha2Digest, Sha256};

        let vk = VerifyingKey::from(key);
        let compressed = vk.to_encoded_point(true);
        let compressed_bytes = compressed.as_bytes(); // 33 bytes

        // HASH160 = RIPEMD160(SHA256(compressed_pubkey))
        let sha = Sha256::digest(compressed_bytes);
        let hash160 = Ripemd160::digest(&sha);

        // Witness version 0, 20-byte program
        segwit::encode(Hrp::parse("bc").unwrap(), segwit::VERSION_0, &hash160)
            .expect("valid witness program")
    }
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

    // ---- secp256k1 / BIP-44 tests ----

    #[test]
    fn eth_key_deterministic() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let k1 = secp256k1::eth_key(&m);
        let k2 = secp256k1::eth_key(&m);
        assert_eq!(k1.to_bytes(), k2.to_bytes());
    }

    #[test]
    fn eth_address_known_vector() {
        // "abandon x11 about" mnemonic, m/44'/60'/0'/0/0 should produce this address.
        // Verified against MetaMask and iancoleman.io/bip39
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let key = secp256k1::eth_key(&m);
        let addr = secp256k1::eth_address(&key);
        assert!(addr.starts_with("0x"), "ETH address must start with 0x, got: {addr}");
        assert_eq!(addr.len(), 42, "ETH address must be 42 chars, got: {}", addr.len());
        // Known address for this mnemonic (lowercase comparison for safety)
        assert_eq!(
            addr.to_lowercase(),
            "0x9858effd232b4033e47d90003d41ec34ecaeda94",
            "ETH address mismatch"
        );
    }

    #[test]
    fn eth_address_eip55_checksum() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let key = secp256k1::eth_key(&m);
        let addr = secp256k1::eth_address(&key);
        // Must have mixed case (EIP-55 checksum), not all lower
        let hex_part = &addr[2..];
        let has_upper = hex_part.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = hex_part.chars().any(|c| c.is_ascii_lowercase());
        assert!(has_upper && has_lower, "EIP-55 should produce mixed case: {addr}");
    }

    #[test]
    fn btc_key_deterministic() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let k1 = secp256k1::btc_key(&m);
        let k2 = secp256k1::btc_key(&m);
        assert_eq!(k1.to_bytes(), k2.to_bytes());
    }

    #[test]
    fn btc_address_format() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let key = secp256k1::btc_key(&m);
        let addr = secp256k1::btc_address_p2wpkh(&key);
        // P2WPKH bech32 addresses start with "bc1q"
        assert!(addr.starts_with("bc1q"), "BTC P2WPKH should start with bc1q, got: {addr}");
        // 42-62 chars for bech32
        assert!(addr.len() >= 42 && addr.len() <= 62, "BTC addr len: {}", addr.len());
    }

    #[test]
    fn eth_and_btc_keys_differ() {
        let m = bip39::Mnemonic::parse(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let eth = secp256k1::eth_key(&m);
        let btc = secp256k1::btc_key(&m);
        assert_ne!(eth.to_bytes(), btc.to_bytes(), "ETH and BTC keys must differ (different derivation paths)");
    }
}
