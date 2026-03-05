use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tabs::TabKind;

const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTab {
    pub kind: TabKind,
    pub title: String,
    pub pinned: bool,
    pub favicon_url: Option<String>,
    pub context_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub version: u32,
    pub tabs: Vec<SessionTab>,
    pub active_tab_index: usize,
    pub next_tab_id: u64,
    pub active_context: Option<String>,
    pub isolated_tabs: bool,
}

/// Returns the path to `session.json` in the same directory as settings.json.
pub fn session_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/Epoca/session.json")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        PathBuf::from(home).join(".epoca/session.json")
    }
}

/// Atomically save session state: write to `.tmp`, then rename.
pub fn save_session(state: &SessionState) {
    let path = session_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
        Err(e) => {
            log::warn!("Failed to serialize session: {e}");
        }
    }
}

/// Load session from disk. Returns None on missing file, parse error,
/// or version mismatch.
pub fn load_session() -> Option<SessionState> {
    let path = session_path();
    let data = std::fs::read_to_string(&path).ok()?;
    let state: SessionState = serde_json::from_str(&data).ok()?;
    if state.version > CURRENT_VERSION {
        log::warn!(
            "Session version {} > current {}, skipping restore",
            state.version,
            CURRENT_VERSION
        );
        return None;
    }
    Some(state)
}

/// Returns true if a TabKind is restorable across sessions.
/// SandboxApp, FramebufferApp, and Welcome are not restorable.
pub fn is_restorable(kind: &TabKind) -> bool {
    matches!(
        kind,
        TabKind::WebView { .. }
            | TabKind::Settings
            | TabKind::CodeEditor { .. }
            | TabKind::DeclarativeApp { .. }
    )
}
