use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Per-file asset entry in the `[assets]` manifest table.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntry {
    pub sha256: String,
    pub size: u64,
}

/// Top-level manifest parsed from `manifest.toml` inside a `.prod` bundle.
#[derive(Debug, Clone, Deserialize)]
pub struct ProdManifest {
    pub app: AppMeta,
    pub permissions: Option<PermissionsMeta>,
    pub sandbox: Option<SandboxMeta>,
    pub webapp: Option<WebAppMeta>,
    /// Per-file asset manifest with SHA-256 hashes and sizes.
    #[serde(default)]
    pub assets: Option<HashMap<String, AssetEntry>>,
}

/// Application metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct AppMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    /// "application", "extension", "widget", "spa"
    #[serde(default = "default_app_type")]
    pub app_type: String,
    pub icon: Option<String>,
}

fn default_app_type() -> String {
    "application".into()
}

/// Permission declarations (mirrors epoca-broker Permissions shape).
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionsMeta {
    pub network: Option<Vec<String>>,
    /// Whether the app can request transaction signing.
    #[serde(default)]
    pub sign: bool,
    /// Whether the app can use statements pub/sub.
    #[serde(default, alias = "statement_store")]
    pub statements: bool,
    /// Whether the app can use chain query/submit APIs.
    #[serde(default)]
    pub chain: bool,
    /// Whether the app can open P2P data connections.
    #[serde(default)]
    pub data: bool,
    /// Media permissions (e.g. ["camera", "audio"]).
    #[serde(default)]
    pub media: Vec<String>,
}

/// Sandbox-specific settings.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxMeta {
    #[serde(default)]
    pub framebuffer: bool,
    pub max_gas_per_update: Option<u64>,
    /// Optional controls hint shown as an overlay (dismissed on first keypress).
    pub controls_hint: Option<String>,
}

/// Web app-specific settings (for `type = "spa"` bundles).
#[derive(Debug, Clone, Deserialize)]
pub struct WebAppMeta {
    /// Entry HTML file inside `assets/` (e.g. "index.html").
    pub entry: String,
    /// Sandbox mode: "strict" blocks all network except host APIs.
    #[serde(default = "default_webapp_sandbox")]
    pub sandbox: String,
    /// Target chain for `epoca.chain.*` APIs (e.g. "paseo-asset-hub", "polkadot-asset-hub").
    /// Defaults to "paseo-asset-hub" if unset.
    #[serde(default = "default_webapp_chain")]
    pub chain: String,
}

fn default_webapp_chain() -> String {
    "paseo-asset-hub".into()
}

fn default_webapp_sandbox() -> String {
    "strict".into()
}

/// A loaded `.prod` bundle (ZIP or CARv1 archive containing manifest + binary + assets).
pub struct ProdBundle {
    pub manifest: ProdManifest,
    /// Contents of `app.polkavm` from the archive. `None` for `type = "spa"` bundles.
    pub program_bytes: Option<Vec<u8>>,
    /// Files under `assets/` in the archive, keyed by relative path (e.g. `doom1.wad`).
    pub assets: HashMap<String, Vec<u8>>,
    /// IPFS CID for lazy asset loading. When set, SPA tabs fetch assets on demand
    /// from the IPFS gateway instead of requiring all assets in memory.
    pub ipfs_cid: Option<String>,
}

/// ZIP magic bytes: PK\x03\x04
const ZIP_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

impl ProdBundle {
    /// Load a `.prod` bundle from disk. Detects format automatically:
    /// ZIP (legacy) or CARv1 (IPFS-native).
    pub fn from_file(path: &Path) -> Result<Self> {
        let data = std::fs::read(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        if data.len() >= 4 && data[..4] == ZIP_MAGIC {
            Self::from_zip_bytes(&data)
        } else if crate::car::is_car_file(&data) {
            Self::from_car_bytes(&data)
        } else {
            Err(anyhow!("Unrecognized .prod format (not ZIP or CAR)"))
        }
    }

    /// Load from raw bytes (auto-detect format).
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() >= 4 && data[..4] == ZIP_MAGIC {
            Self::from_zip_bytes(data)
        } else if crate::car::is_car_file(data) {
            Self::from_car_bytes(data)
        } else {
            Err(anyhow!("Unrecognized .prod format (not ZIP or CAR)"))
        }
    }

    fn from_zip_bytes(data: &[u8]) -> Result<Self> {
        let cursor = std::io::Cursor::new(data);
        let mut archive = zip::ZipArchive::new(cursor)
            .context("Failed to read ZIP archive")?;

        let mut manifest_bytes = None;
        let mut program_bytes = None;
        let mut signature_bytes = None;
        let mut assets = HashMap::new();

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)
                .with_context(|| format!("Failed to read ZIP entry {i}"))?;
            let name = entry.name().to_string();

            if entry.is_dir() {
                continue;
            }

            let mut buf = Vec::with_capacity(entry.size() as usize);
            entry.read_to_end(&mut buf)
                .with_context(|| format!("Failed to read entry {name}"))?;

            if name == "manifest.toml" {
                manifest_bytes = Some(buf);
            } else if name == "app.polkavm" {
                program_bytes = Some(buf);
            } else if name == "signature.toml" {
                signature_bytes = Some(buf);
            } else if let Some(asset_name) = name.strip_prefix("assets/") {
                if !asset_name.is_empty()
                    && !asset_name.contains("..")
                    && !asset_name.starts_with('/')
                {
                    assets.insert(asset_name.to_string(), buf);
                }
            }
        }

        Self::finish_with_sig(manifest_bytes, program_bytes, assets, signature_bytes)
    }

    fn from_car_bytes(data: &[u8]) -> Result<Self> {
        let mut all_files = crate::car::parse_car_to_assets(data)
            .map_err(|e| anyhow!("CAR parse error: {e}"))?;

        let manifest_bytes = all_files.remove("manifest.toml")
            .ok_or_else(|| anyhow!("Missing manifest.toml in CAR bundle"))?;

        let program_bytes = all_files.remove("app.polkavm");
        let signature_bytes = all_files.remove("signature.toml");

        // Remaining files are assets. Strip "assets/" prefix if present
        // (CAR bundles may or may not use the assets/ directory convention).
        let mut assets = HashMap::new();
        for (name, data) in all_files {
            let asset_name = name.strip_prefix("assets/").unwrap_or(&name);
            if !asset_name.is_empty()
                && !asset_name.contains("..")
                && !asset_name.starts_with('/')
            {
                assets.insert(asset_name.to_string(), data);
            }
        }

        Self::finish_with_sig(Some(manifest_bytes), program_bytes, assets, signature_bytes)
    }

    fn finish_with_sig(
        manifest_bytes: Option<Vec<u8>>,
        program_bytes: Option<Vec<u8>>,
        assets: HashMap<String, Vec<u8>>,
        signature_bytes: Option<Vec<u8>>,
    ) -> Result<Self> {
        let raw_manifest =
            manifest_bytes.ok_or_else(|| anyhow!("Missing manifest.toml in .prod bundle"))?;
        let manifest_str = String::from_utf8(raw_manifest.clone())
            .context("manifest.toml is not valid UTF-8")?;

        let manifest: ProdManifest = toml::from_str(&manifest_str)
            .context("Failed to parse manifest.toml")?;

        // Web apps don't require app.polkavm; all other types do.
        if manifest.app.app_type != "spa" && program_bytes.is_none() {
            return Err(anyhow!("Missing app.polkavm in .prod bundle"));
        }

        // Verify bundle signature if present.
        if let Some(sig_bytes) = signature_bytes {
            verify_bundle_signature(&raw_manifest, program_bytes.as_deref(), &sig_bytes)?;
        }

        Ok(Self {
            manifest,
            program_bytes,
            assets,
            ipfs_cid: None,
        })
    }
}

/// TOML structure for `signature.toml`.
#[derive(Deserialize)]
struct SignatureFile {
    /// Hex-encoded ed25519 public key (32 bytes = 64 hex chars).
    pubkey: String,
    /// Hex-encoded ed25519 signature (64 bytes = 128 hex chars).
    signature: String,
}

/// Verify an ed25519 bundle signature.
///
/// The signed message is `sha256(manifest.toml) || sha256(app.polkavm)`.
/// If the bundle has no program (SPA), the program hash is `sha256("")`
/// (the SHA-256 of an empty byte string).
///
/// NOTE: This is a self-signed model — the public key is embedded in the bundle
/// itself. Verification proves integrity (the bundle wasn't tampered with after
/// signing) but not authenticity (anyone can sign their own bundle). A future
/// trusted-publisher allowlist is needed to provide authenticity guarantees.
fn verify_bundle_signature(
    manifest_bytes: &[u8],
    program_bytes: Option<&[u8]>,
    sig_toml_bytes: &[u8],
) -> Result<()> {
    let sig_str = String::from_utf8(sig_toml_bytes.to_vec())
        .context("signature.toml is not valid UTF-8")?;
    let sig_file: SignatureFile =
        toml::from_str(&sig_str).context("Failed to parse signature.toml")?;

    let pubkey_bytes =
        hex::decode(&sig_file.pubkey).context("invalid hex in signature.toml pubkey")?;
    let sig_bytes =
        hex::decode(&sig_file.signature).context("invalid hex in signature.toml signature")?;

    if pubkey_bytes.len() != 32 {
        return Err(anyhow!("pubkey must be 32 bytes, got {}", pubkey_bytes.len()));
    }
    if sig_bytes.len() != 64 {
        return Err(anyhow!("signature must be 64 bytes, got {}", sig_bytes.len()));
    }

    // Build the signed message: sha256(manifest) || sha256(program or empty).
    let manifest_hash = Sha256::digest(manifest_bytes);
    let program_hash = match program_bytes {
        Some(p) => Sha256::digest(p),
        None => Sha256::digest([]), // SPA bundles: sha256 of empty byte string
    };

    let mut message = [0u8; 64];
    message[..32].copy_from_slice(&manifest_hash);
    message[32..].copy_from_slice(&program_hash);

    let vk = ed25519_zebra::VerificationKey::try_from(pubkey_bytes.as_slice())
        .map_err(|e| anyhow!("invalid ed25519 public key: {e}"))?;
    let sig = ed25519_zebra::Signature::from({
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&sig_bytes);
        arr
    });

    vk.verify(&sig, &message)
        .map_err(|e| anyhow!("bundle signature verification failed: {e}"))
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rand_core::OsRng;

    /// Helper: generate a valid signature for given manifest + program bytes.
    fn sign_bundle(manifest: &[u8], program: Option<&[u8]>) -> (String, String) {
        use ed25519_zebra::SigningKey;
        use sha2::Digest;

        let mut rng = OsRng;
        let sk = SigningKey::new(&mut rng);
        let vk = ed25519_zebra::VerificationKey::from(&sk);

        let manifest_hash = Sha256::digest(manifest);
        let program_hash = match program {
            Some(p) => Sha256::digest(p),
            None => Sha256::digest([]),
        };
        let mut message = [0u8; 64];
        message[..32].copy_from_slice(&manifest_hash);
        message[32..].copy_from_slice(&program_hash);

        let sig = sk.sign(&message);

        let pubkey_hex = hex::encode(<[u8; 32]>::from(vk));
        let sig_hex = hex::encode(<[u8; 64]>::from(sig));
        (pubkey_hex, sig_hex)
    }

    fn make_sig_toml(pubkey: &str, signature: &str) -> Vec<u8> {
        format!("pubkey = \"{pubkey}\"\nsignature = \"{signature}\"\n")
            .into_bytes()
    }

    #[test]
    fn valid_signature_accepted() {
        let manifest = b"[app]\nid = \"test\"\nname = \"Test\"\nversion = \"1.0\"";
        let program = b"fake polkavm bytes";
        let (pk, sig) = sign_bundle(manifest, Some(program));
        let sig_toml = make_sig_toml(&pk, &sig);

        let result = verify_bundle_signature(manifest, Some(program), &sig_toml);
        assert!(result.is_ok(), "valid signature should be accepted: {result:?}");
    }

    #[test]
    fn valid_signature_spa_no_program() {
        let manifest = b"[app]\nid = \"test\"\nname = \"Test\"\nversion = \"1.0\"";
        let (pk, sig) = sign_bundle(manifest, None);
        let sig_toml = make_sig_toml(&pk, &sig);

        let result = verify_bundle_signature(manifest, None, &sig_toml);
        assert!(result.is_ok(), "SPA signature should be accepted: {result:?}");
    }

    #[test]
    fn wrong_key_rejected() {
        let manifest = b"[app]\nid = \"test\"";
        let program = b"bytes";

        // Sign with one key
        let (_pk1, sig) = sign_bundle(manifest, Some(program));
        // Verify with a different key
        let (pk2, _sig2) = sign_bundle(manifest, Some(program));

        let sig_toml = make_sig_toml(&pk2, &sig);
        let result = verify_bundle_signature(manifest, Some(program), &sig_toml);
        assert!(result.is_err(), "wrong key should be rejected");
        assert!(
            format!("{result:?}").contains("verification failed"),
            "error should mention verification failure"
        );
    }

    #[test]
    fn tampered_manifest_rejected() {
        let manifest = b"[app]\nid = \"test\"";
        let program = b"bytes";
        let (pk, sig) = sign_bundle(manifest, Some(program));
        let sig_toml = make_sig_toml(&pk, &sig);

        // Tamper with manifest
        let tampered = b"[app]\nid = \"evil\"";
        let result = verify_bundle_signature(tampered, Some(program), &sig_toml);
        assert!(result.is_err(), "tampered manifest should be rejected");
    }

    #[test]
    fn tampered_program_rejected() {
        let manifest = b"[app]\nid = \"test\"";
        let program = b"bytes";
        let (pk, sig) = sign_bundle(manifest, Some(program));
        let sig_toml = make_sig_toml(&pk, &sig);

        let tampered = b"evil bytes";
        let result = verify_bundle_signature(manifest, Some(tampered), &sig_toml);
        assert!(result.is_err(), "tampered program should be rejected");
    }

    #[test]
    fn pubkey_wrong_length_rejected() {
        let sig_toml = make_sig_toml(
            &hex::encode([0u8; 31]), // 31 bytes, not 32
            &hex::encode([0u8; 64]),
        );
        let result = verify_bundle_signature(b"manifest", Some(b"program"), &sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("pubkey must be 32 bytes"));
    }

    #[test]
    fn signature_wrong_length_rejected() {
        let sig_toml = make_sig_toml(
            &hex::encode([0u8; 32]),
            &hex::encode([0u8; 63]), // 63 bytes, not 64
        );
        let result = verify_bundle_signature(b"manifest", Some(b"program"), &sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("signature must be 64 bytes"));
    }

    #[test]
    fn invalid_hex_pubkey_rejected() {
        let sig_toml = make_sig_toml("not_hex_at_all!", &hex::encode([0u8; 64]));
        let result = verify_bundle_signature(b"manifest", Some(b"program"), &sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("invalid hex"));
    }

    #[test]
    fn invalid_hex_signature_rejected() {
        let sig_toml = make_sig_toml(&hex::encode([0u8; 32]), "zzz_not_hex");
        let result = verify_bundle_signature(b"manifest", Some(b"program"), &sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("invalid hex"));
    }

    #[test]
    fn malformed_toml_rejected() {
        let sig_toml = b"this is not toml {{{";
        let result = verify_bundle_signature(b"manifest", Some(b"program"), sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("Failed to parse"));
    }

    #[test]
    fn missing_fields_rejected() {
        let sig_toml = b"pubkey = \"aabb\"\n"; // missing signature field
        let result = verify_bundle_signature(b"manifest", Some(b"program"), sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("Failed to parse"));
    }

    #[test]
    fn non_utf8_rejected() {
        let sig_toml: &[u8] = &[0xff, 0xfe, 0xfd];
        let result = verify_bundle_signature(b"manifest", Some(b"program"), sig_toml);
        assert!(result.is_err());
        assert!(format!("{result:?}").contains("not valid UTF-8"));
    }
}
