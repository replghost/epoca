use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Permission level for geolocation access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GeoPermission {
    None,
    Coarse,
    Fine,
}

/// Permission level for GPU access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GpuPermission {
    None,
    #[serde(rename = "2d")]
    TwoD,
    #[serde(rename = "3d")]
    ThreeD,
}

/// The permissions declared in an app manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppManifest {
    #[serde(default)]
    pub permissions: Permissions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Permissions {
    #[serde(default)]
    pub network: Vec<String>,
    #[serde(default = "default_geo")]
    pub geolocation: GeoPermission,
    #[serde(default)]
    pub camera: bool,
    #[serde(default = "default_gpu")]
    pub gpu: GpuPermission,
    #[serde(default)]
    pub storage: String,
    /// Whether the app can request transaction signing.
    #[serde(default)]
    pub sign: bool,
    /// Whether the app can use Statement Store.
    #[serde(default)]
    pub statement_store: bool,
    /// Media permissions for SPA tabs (e.g. ["camera", "audio"]).
    #[serde(default)]
    pub media: Vec<String>,
}

fn default_geo() -> GeoPermission {
    GeoPermission::None
}

fn default_gpu() -> GpuPermission {
    GpuPermission::None
}

/// Runtime permission state for an app — what the user has actually granted.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GrantedPermissions {
    pub network: Vec<String>,
    pub geolocation: GeoPermission,
    pub camera: bool,
    pub gpu: GpuPermission,
    pub storage_bytes: u64,
    /// Whether the user has granted transaction signing.
    #[serde(default)]
    pub sign: bool,
    /// Whether the user has granted Statement Store access.
    #[serde(default)]
    pub statement_store: bool,
    /// Granted media types (e.g. "camera", "audio").
    #[serde(default)]
    pub media: Vec<String>,
    /// Granted WebSocket URL patterns.
    #[serde(default)]
    pub websocket: Vec<String>,
}

impl Default for GeoPermission {
    fn default() -> Self {
        GeoPermission::None
    }
}

impl Default for GpuPermission {
    fn default() -> Self {
        GpuPermission::None
    }
}

/// The capability broker that mediates permissions between apps and the host.
pub struct CapabilityBroker {
    /// Per-app granted permissions, keyed by app ID.
    grants: HashMap<String, GrantedPermissions>,
    /// Per-app manifest permissions, keyed by app ID.
    manifests: HashMap<String, AppManifest>,
    /// Path to persist permission grants.
    storage_path: Option<String>,
}

impl CapabilityBroker {
    pub fn new() -> Self {
        Self {
            grants: HashMap::new(),
            manifests: HashMap::new(),
            storage_path: None,
        }
    }

    pub fn with_storage(mut self, path: String) -> Self {
        self.storage_path = Some(path);
        self.load_grants();
        self
    }

    /// Load a manifest for an app.
    pub fn load_manifest(&mut self, app_id: &str, manifest_toml: &str) -> Result<()> {
        let manifest: AppManifest = toml::from_str(manifest_toml)?;
        self.manifests.insert(app_id.to_string(), manifest);
        Ok(())
    }

    /// Load a manifest from a file path.
    pub fn load_manifest_file(&mut self, app_id: &str, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.load_manifest(app_id, &content)
    }

    /// Check if an app is allowed to access a specific network domain.
    pub fn check_network(&self, app_id: &str, url: &str) -> PermissionResult {
        let Some(manifest) = self.manifests.get(app_id) else {
            return PermissionResult::Denied("No manifest loaded".to_string());
        };

        // Extract domain from URL
        let domain = extract_domain(url);

        if manifest.permissions.network.iter().any(|allowed| domain_matches(&domain, allowed)) {
            // Check if user has granted network permission
            if let Some(grants) = self.grants.get(app_id) {
                if grants.network.iter().any(|g| domain_matches(&domain, g)) {
                    return PermissionResult::Allowed;
                }
            }
            PermissionResult::NeedsPrompt(format!(
                "App wants to access network: {}",
                domain
            ))
        } else {
            PermissionResult::Denied(format!(
                "Domain '{}' not in manifest allowed list",
                domain
            ))
        }
    }

    /// Check if an app is allowed to access geolocation.
    pub fn check_geolocation(&self, app_id: &str) -> PermissionResult {
        let Some(manifest) = self.manifests.get(app_id) else {
            return PermissionResult::Denied("No manifest loaded".to_string());
        };

        if manifest.permissions.geolocation == GeoPermission::None {
            return PermissionResult::Denied("Geolocation not requested in manifest".to_string());
        }

        if let Some(grants) = self.grants.get(app_id) {
            if grants.geolocation != GeoPermission::None {
                return PermissionResult::Allowed;
            }
        }

        PermissionResult::NeedsPrompt("App wants to access location".to_string())
    }

    /// Grant a network permission for an app.
    pub fn grant_network(&mut self, app_id: &str, domain: &str) {
        let grants = self.grants.entry(app_id.to_string()).or_default();
        if !grants.network.contains(&domain.to_string()) {
            grants.network.push(domain.to_string());
        }
        self.save_grants();
    }

    /// Grant geolocation permission for an app.
    pub fn grant_geolocation(&mut self, app_id: &str, level: GeoPermission) {
        let grants = self.grants.entry(app_id.to_string()).or_default();
        grants.geolocation = level;
        self.save_grants();
    }

    /// Revoke all permissions for an app.
    pub fn revoke_all(&mut self, app_id: &str) {
        self.grants.remove(app_id);
        self.save_grants();
    }

    fn load_grants(&mut self) {
        if let Some(path) = &self.storage_path {
            if let Ok(data) = std::fs::read_to_string(path) {
                if let Ok(grants) = serde_json::from_str(&data) {
                    self.grants = grants;
                }
            }
        }
    }

    fn save_grants(&self) {
        if let Some(path) = &self.storage_path {
            if let Ok(data) = serde_json::to_string_pretty(&self.grants) {
                let _ = std::fs::write(path, data);
            }
        }
    }
}

/// Result of a permission check.
#[derive(Debug, Clone, PartialEq)]
pub enum PermissionResult {
    Allowed,
    Denied(String),
    NeedsPrompt(String),
}

fn extract_domain(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn domain_matches(requested: &str, allowed: &str) -> bool {
    requested == allowed || requested.ends_with(&format!(".{}", allowed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MANIFEST: &str = r#"
[permissions]
network = ["api.weather.com", "cdn.weather.com"]
geolocation = "coarse"
camera = false
gpu = "2d"
storage = "1MB"
"#;

    #[test]
    fn test_parse_manifest() {
        let manifest: AppManifest = toml::from_str(TEST_MANIFEST).unwrap();
        assert_eq!(manifest.permissions.network.len(), 2);
        assert_eq!(manifest.permissions.geolocation, GeoPermission::Coarse);
        assert!(!manifest.permissions.camera);
        assert_eq!(manifest.permissions.gpu, GpuPermission::TwoD);
    }

    #[test]
    fn test_network_check() {
        let mut broker = CapabilityBroker::new();
        broker.load_manifest("weather", TEST_MANIFEST).unwrap();

        // Allowed domain but not yet granted
        let result = broker.check_network("weather", "https://api.weather.com/forecast");
        assert!(matches!(result, PermissionResult::NeedsPrompt(_)));

        // Grant it
        broker.grant_network("weather", "api.weather.com");
        let result = broker.check_network("weather", "https://api.weather.com/forecast");
        assert_eq!(result, PermissionResult::Allowed);

        // Blocked domain (not in manifest)
        let result = broker.check_network("weather", "https://evil.com/steal");
        assert!(matches!(result, PermissionResult::Denied(_)));
    }

    #[test]
    fn test_geolocation_check() {
        let mut broker = CapabilityBroker::new();
        broker.load_manifest("weather", TEST_MANIFEST).unwrap();

        let result = broker.check_geolocation("weather");
        assert!(matches!(result, PermissionResult::NeedsPrompt(_)));

        broker.grant_geolocation("weather", GeoPermission::Coarse);
        let result = broker.check_geolocation("weather");
        assert_eq!(result, PermissionResult::Allowed);
    }
}
