//! App Library — manages installed `.prod` bundles on disk.
//!
//! Apps are extracted to `~/.epoca/apps/{app_id}/` on first open.
//! Subsequent launches load directly from disk (no ZIP re-extraction).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Metadata persisted alongside an installed app.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub app_id: String,
    pub name: String,
    pub version: String,
    pub app_type: String,
    pub installed_at: String,
    pub last_launched: String,
    /// Whether the sandbox uses framebuffer mode.
    pub framebuffer: bool,
}

/// Returns the root directory for installed apps.
pub fn apps_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".epoca").join("apps")
}

/// List all installed apps by reading `meta.json` from each subdirectory.
pub fn list_installed() -> Vec<InstalledApp> {
    let dir = apps_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut apps = Vec::new();
    for entry in entries.flatten() {
        let meta_path = entry.path().join("meta.json");
        if let Ok(data) = std::fs::read_to_string(&meta_path) {
            if let Ok(app) = serde_json::from_str::<InstalledApp>(&data) {
                apps.push(app);
            }
        }
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    apps
}

/// Install a `.prod` bundle: extract to `~/.epoca/apps/{app_id}/` and write `meta.json`.
/// Returns the install directory path.
pub fn install_prod(prod_path: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(prod_path)
        .map_err(|e| format!("Failed to open {}: {e}", prod_path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("Failed to read ZIP: {e}"))?;

    // Read manifest first to get app_id.
    let manifest_str = {
        let mut entry = archive.by_name("manifest.toml")
            .map_err(|_| "Missing manifest.toml in .prod bundle".to_string())?;
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut entry, &mut buf)
            .map_err(|e| format!("Failed to read manifest.toml: {e}"))?;
        buf
    };

    #[derive(Deserialize)]
    struct Manifest {
        app: AppSection,
        sandbox: Option<SandboxSection>,
    }
    #[derive(Deserialize)]
    struct AppSection {
        id: String,
        name: String,
        version: String,
        #[serde(default = "default_type")]
        app_type: String,
    }
    fn default_type() -> String { "application".into() }
    #[derive(Deserialize)]
    struct SandboxSection {
        #[serde(default)]
        framebuffer: bool,
    }

    let manifest: Manifest = toml::from_str(&manifest_str)
        .map_err(|e| format!("Failed to parse manifest.toml: {e}"))?;

    let app_id = &manifest.app.id;
    let install_dir = apps_dir().join(app_id);
    let assets_dir = install_dir.join("assets");

    // Create directories.
    std::fs::create_dir_all(&assets_dir)
        .map_err(|e| format!("Failed to create {}: {e}", assets_dir.display()))?;

    // Extract all files.
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)
            .map_err(|e| format!("Failed to read ZIP entry {i}: {e}"))?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }

        let dest = install_dir.join(&name);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut buf = Vec::with_capacity(entry.size() as usize);
        std::io::Read::read_to_end(&mut entry, &mut buf)
            .map_err(|e| format!("Failed to read {name}: {e}"))?;
        std::fs::write(&dest, &buf)
            .map_err(|e| format!("Failed to write {}: {e}", dest.display()))?;
    }

    // Write meta.json.
    let now = chrono_now();
    let meta = InstalledApp {
        app_id: app_id.clone(),
        name: manifest.app.name.clone(),
        version: manifest.app.version.clone(),
        app_type: manifest.app.app_type.clone(),
        installed_at: now.clone(),
        last_launched: now,
        framebuffer: manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer),
    };
    let meta_json = serde_json::to_string_pretty(&meta).unwrap_or_default();
    std::fs::write(install_dir.join("meta.json"), &meta_json)
        .map_err(|e| format!("Failed to write meta.json: {e}"))?;

    log::info!("[app-library] Installed {} to {}", app_id, install_dir.display());
    Ok(install_dir)
}

/// Load a ProdBundle from an installed app directory (no ZIP extraction needed).
pub fn load_installed(app_id: &str) -> Result<epoca_sandbox::ProdBundle, String> {
    let dir = apps_dir().join(app_id);
    let manifest_path = dir.join("manifest.toml");
    let manifest_str = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("Failed to read manifest: {e}"))?;
    let manifest: epoca_sandbox::ProdManifest = toml::from_str(&manifest_str)
        .map_err(|e| format!("Failed to parse manifest: {e}"))?;

    let program_bytes = if manifest.app.app_type != "spa" {
        let polkavm_path = dir.join("app.polkavm");
        Some(std::fs::read(&polkavm_path)
            .map_err(|e| format!("Failed to read app.polkavm: {e}"))?)
    } else {
        None
    };

    let mut assets = HashMap::new();
    let assets_dir = dir.join("assets");
    if assets_dir.is_dir() {
        load_assets_recursive(&assets_dir, &assets_dir, &mut assets);
    }

    Ok(epoca_sandbox::ProdBundle {
        manifest,
        program_bytes,
        assets,
    })
}

fn load_assets_recursive(base: &Path, dir: &Path, assets: &mut HashMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            load_assets_recursive(base, &path, assets);
        } else if let Ok(data) = std::fs::read(&path) {
            if let Ok(rel) = path.strip_prefix(base) {
                let key = rel.to_string_lossy().to_string();
                assets.insert(key, data);
            }
        }
    }
}

/// Update last_launched timestamp for an installed app.
pub fn touch_last_launched(app_id: &str) {
    let meta_path = apps_dir().join(app_id).join("meta.json");
    if let Ok(data) = std::fs::read_to_string(&meta_path) {
        if let Ok(mut meta) = serde_json::from_str::<InstalledApp>(&data) {
            meta.last_launched = chrono_now();
            if let Ok(json) = serde_json::to_string_pretty(&meta) {
                let _ = std::fs::write(&meta_path, json);
            }
        }
    }
}

/// Remove an installed app.
pub fn uninstall(app_id: &str) -> Result<(), String> {
    let dir = apps_dir().join(app_id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("Failed to remove {}: {e}", dir.display()))?;
        log::info!("[app-library] Uninstalled {}", app_id);
    }
    Ok(())
}

/// Check if an app is installed.
pub fn is_installed(app_id: &str) -> bool {
    apps_dir().join(app_id).join("meta.json").exists()
}

fn chrono_now() -> String {
    // Simple ISO-8601 timestamp without pulling in chrono crate.
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}
