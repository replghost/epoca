use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

/// Top-level manifest parsed from `manifest.toml` inside a `.prod` bundle.
#[derive(Debug, Clone, Deserialize)]
pub struct ProdManifest {
    pub app: AppMeta,
    pub permissions: Option<PermissionsMeta>,
    pub sandbox: Option<SandboxMeta>,
}

/// Application metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct AppMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    /// "application", "extension", "widget"
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
}

/// Sandbox-specific settings.
#[derive(Debug, Clone, Deserialize)]
pub struct SandboxMeta {
    #[serde(default)]
    pub framebuffer: bool,
    pub max_gas_per_update: Option<u64>,
}

/// A loaded `.prod` bundle (ZIP archive containing manifest + binary + assets).
pub struct ProdBundle {
    pub manifest: ProdManifest,
    /// Contents of `app.polkavm` from the archive.
    pub program_bytes: Vec<u8>,
    /// Files under `assets/` in the archive, keyed by relative path (e.g. `doom1.wad`).
    pub assets: HashMap<String, Vec<u8>>,
}

impl ProdBundle {
    /// Load a `.prod` bundle from a ZIP file on disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .context("Failed to read ZIP archive")?;

        let mut manifest_bytes = None;
        let mut program_bytes = None;
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
            } else if let Some(asset_name) = name.strip_prefix("assets/") {
                if !asset_name.is_empty() {
                    assets.insert(asset_name.to_string(), buf);
                }
            }
        }

        let manifest_str = String::from_utf8(
            manifest_bytes.ok_or_else(|| anyhow!("Missing manifest.toml in .prod bundle"))?,
        )
        .context("manifest.toml is not valid UTF-8")?;

        let manifest: ProdManifest = toml::from_str(&manifest_str)
            .context("Failed to parse manifest.toml")?;

        let program_bytes = program_bytes
            .ok_or_else(|| anyhow!("Missing app.polkavm in .prod bundle"))?;

        Ok(Self {
            manifest,
            program_bytes,
            assets,
        })
    }
}
