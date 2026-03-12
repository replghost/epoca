use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Fixed UUID for the "default" browsing context (non-isolated, non-experimental).
/// All regular tabs share this WKWebsiteDataStore so they share cookies/storage.
/// Derived from UUID v5 with the DNS namespace and "epoca.default.store".
pub const DEFAULT_STORE_UUID: [u8; 16] = [
    0x7a, 0x3b, 0x8c, 0x1d, 0x4e, 0x5f, 0x40, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29,
];

/// Generate a random 16-byte UUID suitable for `WKWebsiteDataStore.dataStoreForIdentifier`.
/// Uses OS CSPRNG via `getrandom` for collision resistance — these UUIDs are security
/// boundaries between browsing contexts, so predictable IDs would be a vulnerability.
pub fn generate_store_uuid() -> [u8; 16] {
    let mut uuid = [0u8; 16];
    getrandom::getrandom(&mut uuid).expect("OS random source unavailable");
    // Set version 4 (random) and variant 1 bits for RFC 4122 compliance.
    uuid[6] = (uuid[6] & 0x0f) | 0x40; // version 4
    uuid[8] = (uuid[8] & 0x3f) | 0x80; // variant 1
    uuid
}

/// Parse a hex-encoded 16-byte UUID string back to bytes.
pub fn parse_store_uuid(hex: &str) -> Option<[u8; 16]> {
    let hex = hex.replace('-', "");
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for i in 0..16 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

/// Encode a 16-byte UUID as a hex string (no dashes).
pub fn format_store_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchEngine {
    DuckDuckGo,
    Google,
    Brave,
    Startpage,
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self::DuckDuckGo
    }
}

impl SearchEngine {
    pub fn all() -> &'static [SearchEngine] {
        &[
            SearchEngine::DuckDuckGo,
            SearchEngine::Google,
            SearchEngine::Brave,
            SearchEngine::Startpage,
        ]
    }

    pub fn display_name(self) -> &'static str {
        match self {
            SearchEngine::DuckDuckGo => "DuckDuckGo",
            SearchEngine::Google => "Google",
            SearchEngine::Brave => "Brave",
            SearchEngine::Startpage => "Startpage",
        }
    }

    pub fn search_url(self, query: &str) -> String {
        match self {
            SearchEngine::DuckDuckGo => format!("https://duckduckgo.com/?q={query}"),
            SearchEngine::Google => format!("https://www.google.com/search?q={query}"),
            SearchEngine::Brave => format!("https://search.brave.com/search?q={query}"),
            SearchEngine::Startpage => format!("https://www.startpage.com/do/search?q={query}"),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HistoryRetention {
    SessionOnly,
    Hours8,
    Hours24,
    Days7,
    Days30,
}

impl Default for HistoryRetention {
    fn default() -> Self {
        Self::Hours24
    }
}

impl HistoryRetention {
    /// Returns the TTL in seconds, or `None` for `SessionOnly` (in-memory only).
    pub fn ttl_secs(&self) -> Option<u64> {
        match self {
            Self::SessionOnly => None,
            Self::Hours8 => Some(8 * 3600),
            Self::Hours24 => Some(24 * 3600),
            Self::Days7 => Some(7 * 24 * 3600),
            Self::Days30 => Some(30 * 24 * 3600),
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Self::SessionOnly => "Session Only",
            Self::Hours8 => "8 Hours",
            Self::Hours24 => "24 Hours",
            Self::Days7 => "7 Days",
            Self::Days30 => "30 Days",
        }
    }

    pub fn all() -> &'static [HistoryRetention] {
        &[
            HistoryRetention::SessionOnly,
            HistoryRetention::Hours8,
            HistoryRetention::Hours24,
            HistoryRetention::Days7,
            HistoryRetention::Days30,
        ]
    }
}

/// Preset colors for session contexts.
pub const DEFAULT_CONTEXT_COLORS: &[&str] = &[
    "#3b82f6", // blue
    "#22c55e", // green
    "#f59e0b", // amber
    "#ef4444", // red
    "#a855f7", // purple
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub id: String,
    pub name: String,
    pub color: String,
    /// Hex-encoded 16-byte UUID for `WKWebsiteDataStore.dataStoreForIdentifier`.
    /// Each context gets its own data store → its own WebContent process namespace.
    #[serde(default)]
    pub store_uuid: Option<String>,
}

impl SessionContext {
    /// Parse the stored hex UUID. Returns `None` if absent or malformed —
    /// callers must treat `None` as "isolation unavailable, fail closed."
    pub fn data_store_uuid(&self) -> Option<[u8; 16]> {
        self.store_uuid.as_deref().and_then(parse_store_uuid)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub isolated_tabs: bool,
    pub shield_enabled: bool,
    pub experimental_chain: bool,
    pub enabled_chains: HashSet<String>,
    #[serde(default)]
    pub search_engine: SearchEngine,
    #[serde(default = "default_true")]
    pub open_links_in_background: bool,
    #[serde(default)]
    pub experimental_contexts: bool,
    #[serde(default)]
    pub contexts: Vec<SessionContext>,
    #[serde(default)]
    pub history_retention: HistoryRetention,
    #[serde(default)]
    pub experimental_wallet: bool,
    #[serde(default)]
    pub experimental_eth: bool,
    #[serde(default)]
    pub experimental_btc: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            isolated_tabs: true,
            shield_enabled: true,
            experimental_chain: false,
            enabled_chains: HashSet::new(),
            search_engine: SearchEngine::default(),
            open_links_in_background: true,
            experimental_contexts: false,
            contexts: Vec::new(),
            history_retention: HistoryRetention::default(),
            experimental_wallet: false,
            experimental_eth: false,
            experimental_btc: false,
        }
    }
}

pub struct SettingsGlobal {
    pub settings: AppSettings,
    path: PathBuf,
}

impl gpui::Global for SettingsGlobal {}

impl SettingsGlobal {
    pub fn load() -> Self {
        let path = Self::settings_path();
        let mut settings: AppSettings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();

        // Migration: assign or repair store_uuid for any context that lacks a valid one.
        let mut migrated = false;
        for ctx in &mut settings.contexts {
            let valid = ctx
                .store_uuid
                .as_deref()
                .and_then(parse_store_uuid)
                .is_some();
            if !valid {
                ctx.store_uuid = Some(format_store_uuid(&generate_store_uuid()));
                migrated = true;
            }
        }

        let global = Self { settings, path };
        if migrated {
            global.save();
        }
        global
    }

    fn settings_path() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join("Library/Application Support/Epoca/settings.json")
        }
        #[cfg(not(target_os = "macos"))]
        {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default();
            PathBuf::from(home).join(".epoca/settings.json")
        }
    }

    pub fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&self.settings) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_retention_default_is_24h() {
        assert_eq!(HistoryRetention::default(), HistoryRetention::Hours24);
    }

    #[test]
    fn history_retention_ttl_values_are_correct() {
        assert_eq!(HistoryRetention::SessionOnly.ttl_secs(), None);
        assert_eq!(HistoryRetention::Hours8.ttl_secs(), Some(8 * 3600));
        assert_eq!(HistoryRetention::Hours24.ttl_secs(), Some(24 * 3600));
        assert_eq!(HistoryRetention::Days7.ttl_secs(), Some(7 * 24 * 3600));
        assert_eq!(HistoryRetention::Days30.ttl_secs(), Some(30 * 24 * 3600));
    }

    #[test]
    fn history_retention_serde_roundtrip() {
        for &variant in HistoryRetention::all() {
            let json = serde_json::to_string(&variant).unwrap();
            let back: HistoryRetention = serde_json::from_str(&json).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn app_settings_default_has_history_retention() {
        let s = AppSettings::default();
        assert_eq!(s.history_retention, HistoryRetention::Hours24);
    }

    #[test]
    fn app_settings_deserializes_without_history_retention_field() {
        let json = r#"{"isolated_tabs":true,"shield_enabled":true,"experimental_chain":false,"enabled_chains":[],"search_engine":"DuckDuckGo","open_links_in_background":true}"#;
        let s: AppSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.history_retention, HistoryRetention::Hours24);
    }

    // ── Process isolation / data store UUID tests ────────────────────

    #[test]
    fn test_generate_store_uuid_produces_valid_v4() {
        let uuid = generate_store_uuid();
        assert_eq!(uuid[6] >> 4, 4, "version nibble must be 4");
        assert_eq!(uuid[8] >> 6, 2, "variant bits must be 10");
    }

    #[test]
    fn test_generate_store_uuid_is_unique() {
        let a = generate_store_uuid();
        let b = generate_store_uuid();
        assert_ne!(a, b);
    }

    #[test]
    fn test_format_and_parse_store_uuid_roundtrip() {
        let uuid = generate_store_uuid();
        let hex = format_store_uuid(&uuid);
        assert_eq!(hex.len(), 32);
        let back = parse_store_uuid(&hex).expect("parse should succeed");
        assert_eq!(back, uuid);
    }

    #[test]
    fn test_parse_store_uuid_rejects_short_hex() {
        assert!(parse_store_uuid("abcdef").is_none());
    }

    #[test]
    fn test_parse_store_uuid_rejects_invalid_hex() {
        assert!(parse_store_uuid("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_none());
    }

    #[test]
    fn test_parse_store_uuid_accepts_dashes() {
        let hex = "7a3b8c1d-4e5f-40a1-b2c3-d4e5f6071829";
        let parsed = parse_store_uuid(hex).expect("should accept dashes");
        assert_eq!(parsed, DEFAULT_STORE_UUID);
    }

    #[test]
    fn test_default_store_uuid_is_constant() {
        let hex = format_store_uuid(&DEFAULT_STORE_UUID);
        assert_eq!(hex, "7a3b8c1d4e5f40a1b2c3d4e5f6071829");
    }

    #[test]
    fn test_session_context_data_store_uuid_returns_stored() {
        let uuid = generate_store_uuid();
        let ctx = SessionContext {
            id: "ctx-1".into(),
            name: "Test".into(),
            color: "#ff0000".into(),
            store_uuid: Some(format_store_uuid(&uuid)),
        };
        assert_eq!(ctx.data_store_uuid(), Some(uuid));
    }

    #[test]
    fn test_session_context_data_store_uuid_none_when_missing() {
        let ctx = SessionContext {
            id: "ctx-1".into(),
            name: "Test".into(),
            color: "#ff0000".into(),
            store_uuid: None,
        };
        assert_eq!(ctx.data_store_uuid(), None);
    }

    #[test]
    fn test_session_context_data_store_uuid_none_when_malformed() {
        let ctx = SessionContext {
            id: "ctx-1".into(),
            name: "Test".into(),
            color: "#ff0000".into(),
            store_uuid: Some("not-valid-hex!!".into()),
        };
        assert_eq!(ctx.data_store_uuid(), None);
    }

    #[test]
    fn test_session_context_deserializes_without_store_uuid() {
        let json = r##"{"id":"ctx-1","name":"Work","color":"#3b82f6"}"##;
        let ctx: SessionContext = serde_json::from_str(json).unwrap();
        assert!(ctx.store_uuid.is_none());
    }

    #[test]
    fn test_session_context_serde_roundtrip_with_store_uuid() {
        let ctx = SessionContext {
            id: "ctx-1".into(),
            name: "Work".into(),
            color: "#3b82f6".into(),
            store_uuid: Some(format_store_uuid(&generate_store_uuid())),
        };
        let json = serde_json::to_string(&ctx).unwrap();
        let back: SessionContext = serde_json::from_str(&json).unwrap();
        assert_eq!(back.store_uuid, ctx.store_uuid);
    }

    #[test]
    fn test_app_settings_contexts_preserve_store_uuid() {
        let mut s = AppSettings::default();
        s.contexts.push(SessionContext {
            id: "ctx-1".into(),
            name: "Work".into(),
            color: "#3b82f6".into(),
            store_uuid: Some("aabbccdd11223344aabbccdd11223344".into()),
        });
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.contexts[0].store_uuid.as_deref(),
            Some("aabbccdd11223344aabbccdd11223344")
        );
    }
}
