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
