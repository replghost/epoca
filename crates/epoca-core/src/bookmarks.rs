//! Bookmarks — local bookmark storage backed by JSON on disk.
//!
//! On macOS: `~/Library/Application Support/Epoca/bookmarks.json`
//! On other platforms: `~/.epoca/bookmarks.json`
//!
//! All mutations go through the in-memory `BookmarkStore` to avoid
//! re-reading from disk on every render frame.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    /// Unix timestamp (seconds) when bookmarked.
    pub added_at: u64,
}

/// Returns the bookmarks file path (platform-aware, matches session.rs).
fn bookmarks_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join("Library/Application Support/Epoca/bookmarks.json")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();
        PathBuf::from(home).join(".epoca/bookmarks.json")
    }
}

/// Normalize a URL for deduplication.
/// Strips trailing slash, lowercases the scheme+host, removes fragment.
fn normalize_url(url: &str) -> String {
    let mut s = url.to_string();
    // Remove fragment
    if let Some(pos) = s.find('#') {
        s.truncate(pos);
    }
    // Lowercase scheme + host (everything before the first '/' after '://')
    if let Some(rest_start) = s.find("://").map(|i| i + 3) {
        let (before, rest) = s.split_at(rest_start);
        let host_end = rest.find('/').unwrap_or(rest.len());
        let (host, path) = rest.split_at(host_end);
        s = format!("{}{}{}", before.to_lowercase(), host.to_lowercase(), path);
    }
    // Strip trailing slash (but keep "/" for root paths like "https://example.com/")
    if s.ends_with('/') && s.len() > 1 {
        // Only strip if there's a path component (not just "https://example.com/")
        let after_scheme = s.find("://").map(|i| i + 3).unwrap_or(0);
        let has_path = s[after_scheme..].contains('/');
        if has_path {
            // Count slashes after scheme — if only one slash (the host separator), keep it
            let slash_count = s[after_scheme..].matches('/').count();
            if slash_count > 1 || !s.ends_with('/') {
                s = s.trim_end_matches('/').to_string();
            }
        }
    }
    s
}

/// In-memory bookmark store. Loaded once from disk, mutated in-place,
/// flushed to disk on each write.
static STORE: Mutex<Option<Vec<Bookmark>>> = Mutex::new(None);

/// Ensure the in-memory store is populated.
fn with_store<F, R>(f: F) -> R
where
    F: FnOnce(&mut Vec<Bookmark>) -> R,
{
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Some(load_from_disk());
    }
    f(guard.as_mut().unwrap())
}

/// Load bookmarks from disk (called once on first access).
fn load_from_disk() -> Vec<Bookmark> {
    let path = bookmarks_path();
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Flush the in-memory store to disk (atomic write).
fn flush(bookmarks: &[Bookmark]) {
    let path = bookmarks_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!("[bookmarks] Failed to create directory {}: {e}", parent.display());
            return;
        }
    }
    let json = match serde_json::to_string_pretty(bookmarks) {
        Ok(j) => j,
        Err(e) => {
            log::error!("[bookmarks] Failed to serialize bookmarks: {e}");
            return;
        }
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        log::error!("[bookmarks] Failed to write {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::error!("[bookmarks] Failed to rename {} → {}: {e}", tmp.display(), path.display());
    }
}

/// Get a snapshot of all bookmarks (from memory, no disk I/O).
pub fn list() -> Vec<Bookmark> {
    with_store(|bm| bm.clone())
}

/// Check if a URL is bookmarked (from memory, no disk I/O).
pub fn is_bookmarked(url: &str) -> bool {
    let norm = normalize_url(url);
    with_store(|bm| bm.iter().any(|b| normalize_url(&b.url) == norm))
}

/// Toggle a bookmark: add if absent, remove if present.
/// Returns the new bookmarked state (true = now bookmarked).
pub fn toggle(url: &str, title: &str) -> bool {
    let norm = normalize_url(url);
    with_store(|bm| {
        if let Some(pos) = bm.iter().position(|b| normalize_url(&b.url) == norm) {
            bm.remove(pos);
            flush(bm);
            false
        } else {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            bm.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
                added_at: now,
            });
            flush(bm);
            true
        }
    })
}

/// Remove a bookmark by URL. Returns updated list.
pub fn remove(url: &str) -> Vec<Bookmark> {
    let norm = normalize_url(url);
    with_store(|bm| {
        bm.retain(|b| normalize_url(&b.url) != norm);
        flush(bm);
        bm.clone()
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_fragment() {
        assert_eq!(normalize_url("https://example.com/page#section"), "https://example.com/page");
    }

    #[test]
    fn normalize_lowercases_host() {
        assert_eq!(normalize_url("https://Example.COM/Path"), "https://example.com/Path");
    }

    #[test]
    fn normalize_trailing_slash() {
        // Root URL keeps its slash
        assert_eq!(normalize_url("https://example.com/"), "https://example.com/");
        // Path URL strips trailing slash
        assert_eq!(normalize_url("https://example.com/page/"), "https://example.com/page");
    }

    #[test]
    fn normalize_preserves_query() {
        assert_eq!(
            normalize_url("https://example.com/search?q=rust"),
            "https://example.com/search?q=rust"
        );
    }

    #[test]
    fn bookmark_roundtrip_json() {
        let bm = Bookmark {
            url: "https://example.com".into(),
            title: "Example".into(),
            added_at: 1000,
        };
        let json = serde_json::to_string(&bm).unwrap();
        let parsed: Bookmark = serde_json::from_str(&json).unwrap();
        assert_eq!(bm, parsed);
    }

    #[test]
    fn empty_json_returns_empty_vec() {
        let result: Vec<Bookmark> = serde_json::from_str("[]").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn corrupted_json_returns_empty_vec() {
        let result: Vec<Bookmark> = serde_json::from_str("{not json}").unwrap_or_default();
        assert!(result.is_empty());
    }

    #[test]
    fn object_instead_of_array_returns_empty() {
        let result: Vec<Bookmark> = serde_json::from_str("{}").unwrap_or_default();
        assert!(result.is_empty());
    }

    #[test]
    fn normalize_deduplicates_trailing_slash_vs_not() {
        let a = normalize_url("https://example.com/page/");
        let b = normalize_url("https://example.com/page");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_deduplicates_case() {
        let a = normalize_url("https://Example.com/Page");
        let b = normalize_url("https://example.com/Page");
        assert_eq!(a, b);
    }

    #[test]
    fn normalize_deduplicates_fragment() {
        let a = normalize_url("https://example.com/page#top");
        let b = normalize_url("https://example.com/page");
        assert_eq!(a, b);
    }
}
