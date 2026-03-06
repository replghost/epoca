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
    pub webapp: Option<WebAppMeta>,
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
    /// Whether the app can use Statement Store.
    #[serde(default)]
    pub statement_store: bool,
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
}

/// Web app-specific settings (for `type = "spa"` bundles).
#[derive(Debug, Clone, Deserialize)]
pub struct WebAppMeta {
    /// Entry HTML file inside `assets/` (e.g. "index.html").
    pub entry: String,
    /// Sandbox mode: "strict" blocks all network except host APIs.
    #[serde(default = "default_webapp_sandbox")]
    pub sandbox: String,
}

fn default_webapp_sandbox() -> String {
    "strict".into()
}

/// A loaded `.prod` bundle (ZIP archive containing manifest + binary + assets).
pub struct ProdBundle {
    pub manifest: ProdManifest,
    /// Contents of `app.polkavm` from the archive. `None` for `type = "spa"` bundles.
    pub program_bytes: Option<Vec<u8>>,
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
                if !asset_name.is_empty()
                    && !asset_name.contains("..")
                    && !asset_name.starts_with('/')
                {
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

        // Web apps don't require app.polkavm; all other types do.
        if manifest.app.app_type != "spa" && program_bytes.is_none() {
            return Err(anyhow!("Missing app.polkavm in .prod bundle"));
        }

        Ok(Self {
            manifest,
            program_bytes,
            assets,
        })
    }
}
