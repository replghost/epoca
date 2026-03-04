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
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            isolated_tabs: false,
            shield_enabled: true,
            experimental_chain: false,
            enabled_chains: HashSet::new(),
            search_engine: SearchEngine::default(),
            open_links_in_background: true,
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
