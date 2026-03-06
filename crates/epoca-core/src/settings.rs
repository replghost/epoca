use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

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
        let settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Self { settings, path }
    }

    fn settings_path() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home)
                .join("Library/Application Support/Epoca/settings.json")
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
}
