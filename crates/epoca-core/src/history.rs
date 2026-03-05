use crate::settings::{HistoryRetention, SettingsGlobal};
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Returns true if the URL uses http:// or https:// (i.e. worth recording in history).
pub fn is_http_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// A single browsing history entry.
#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    pub last_visited: i64,
}

/// Manages an SQLite-backed browsing history database.
pub struct HistoryManager {
    conn: Arc<Mutex<Connection>>,
    retention: HistoryRetention,
}

impl HistoryManager {
    /// Open the history database at the default platform path.
    /// `SessionOnly` uses an in-memory database.
    pub fn open(retention: HistoryRetention) -> Self {
        if retention == HistoryRetention::SessionOnly {
            return Self::open_in_memory(retention);
        }
        let path = Self::default_path();
        Self::open_at(path, retention)
    }

    /// Open the history database at a specific path (for testing).
    pub fn open_at(path: PathBuf, retention: HistoryRetention) -> Self {
        if retention == HistoryRetention::SessionOnly {
            return Self::open_in_memory(retention);
        }
        match Self::try_open_file(&path) {
            Ok(mgr) => {
                Self::set_file_permissions(&path);
                HistoryManager {
                    conn: Arc::new(Mutex::new(mgr)),
                    retention,
                }
            }
            Err(e) => {
                log::warn!("History DB corrupt or failed ({e}), recreating");
                // Delete and retry
                let _ = std::fs::remove_file(&path);
                match Self::try_open_file(&path) {
                    Ok(mgr) => {
                        Self::set_file_permissions(&path);
                        HistoryManager {
                            conn: Arc::new(Mutex::new(mgr)),
                            retention,
                        }
                    }
                    Err(e2) => {
                        log::warn!("History DB retry failed ({e2}), falling back to in-memory");
                        Self::open_in_memory(retention)
                    }
                }
            }
        }
    }

    fn open_in_memory(retention: HistoryRetention) -> Self {
        let conn = Connection::open_in_memory().expect("in-memory SQLite never fails");
        Self::init_schema(&conn).ok();
        HistoryManager {
            conn: Arc::new(Mutex::new(conn)),
            retention,
        }
    }

    fn try_open_file(path: &PathBuf) -> Result<Connection, rusqlite::Error> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(conn)
    }

    fn init_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA auto_vacuum=INCREMENTAL;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                url TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL DEFAULT '',
                visit_count INTEGER NOT NULL DEFAULT 1,
                last_visited INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_last_visited ON history(last_visited);",
        )?;
        Ok(())
    }

    #[cfg(unix)]
    fn set_file_permissions(path: &PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    fn set_file_permissions(_path: &PathBuf) {}

    fn default_path() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join("Library/Application Support/Epoca/history.db")
        }
        #[cfg(not(target_os = "macos"))]
        {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default();
            PathBuf::from(home).join(".epoca/history.db")
        }
    }

    fn now_epoch() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    /// Record a page visit. Upserts: increments visit_count on conflict.
    pub fn record_visit(&self, url: &str, title: &str) {
        let now = Self::now_epoch();
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO history (url, title, visit_count, last_visited)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET
                visit_count = visit_count + 1,
                title = CASE WHEN ?2 = '' THEN title ELSE ?2 END,
                last_visited = ?3",
            rusqlite::params![url, title, now],
        );
    }

    /// Update the title for an existing URL without bumping visit_count.
    pub fn update_title(&self, url: &str, title: &str) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "UPDATE history SET title = ?1 WHERE url = ?2",
            rusqlite::params![title, url],
        );
    }

    /// Delete entries older than the configured TTL. No-op for SessionOnly.
    pub fn cleanup_expired(&self) {
        let Some(ttl) = self.retention.ttl_secs() else {
            return;
        };
        let cutoff = Self::now_epoch() - ttl as i64;
        let conn = self.conn.lock().unwrap();
        let deleted: i64 = conn
            .execute(
                "DELETE FROM history WHERE last_visited < ?1",
                rusqlite::params![cutoff],
            )
            .unwrap_or(0) as i64;
        if deleted > 0 {
            let _ = conn.execute_batch("PRAGMA incremental_vacuum(100);");
            log::info!("History cleanup: deleted {deleted} expired entries");
        }
    }

    /// Search history by URL or title substring. Returns up to `limit` results
    /// ordered by frecency (visit_count * recency_weight). Empty query returns empty vec.
    pub fn search(&self, query: &str, limit: usize) -> Vec<HistoryEntry> {
        if query.is_empty() {
            return Vec::new();
        }
        let conn = self.conn.lock().unwrap();
        // Escape LIKE wildcards in user input
        let escaped = query.replace('%', "\\%").replace('_', "\\_");
        let pattern = format!("%{escaped}%");
        let now = Self::now_epoch();
        let mut stmt = match conn.prepare(
            "SELECT url, title, visit_count, last_visited FROM history
             WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
             ORDER BY (visit_count * 1.0 / MAX(1, (?2 - last_visited) / 3600 + 1)) DESC
             LIMIT ?3",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map(rusqlite::params![pattern, now, limit as i64], |row| {
            Ok(HistoryEntry {
                url: row.get(0)?,
                title: row.get(1)?,
                visit_count: row.get(2)?,
                last_visited: row.get(3)?,
            })
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Get visit count for a URL (for testing).
    #[cfg(test)]
    fn get_visit_count(&self, url: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT visit_count FROM history WHERE url = ?1",
            rusqlite::params![url],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// Insert with a specific timestamp (for testing TTL).
    #[cfg(test)]
    fn record_visit_at(&self, url: &str, title: &str, timestamp: i64) {
        let conn = self.conn.lock().unwrap();
        let _ = conn.execute(
            "INSERT INTO history (url, title, visit_count, last_visited)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET
                visit_count = visit_count + 1,
                title = CASE WHEN ?2 = '' THEN title ELSE ?2 END,
                last_visited = ?3",
            rusqlite::params![url, title, timestamp],
        );
    }
}

/// GPUI global wrapper for HistoryManager.
pub struct HistoryGlobal {
    pub manager: HistoryManager,
}

impl gpui::Global for HistoryGlobal {}

/// Initialize the history subsystem. Reads retention from settings, opens DB,
/// runs initial cleanup, and sets the GPUI global.
pub fn init_history(cx: &mut gpui::App) {
    let retention = cx
        .try_global::<SettingsGlobal>()
        .map(|g| g.settings.history_retention)
        .unwrap_or_default();
    let manager = HistoryManager::open(retention);
    manager.cleanup_expired();
    cx.set_global(HistoryGlobal { manager });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem_manager(retention: HistoryRetention) -> HistoryManager {
        HistoryManager::open_in_memory(retention)
    }

    // ── Core correctness ──────────────────────────────────────────────

    #[test]
    fn record_and_search_url_match() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://example.com", "Example");
        let results = m.search("example", 8);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://example.com");
    }

    #[test]
    fn record_and_search_title_match() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://foo.com", "My Fancy Page");
        let results = m.search("Fancy", 8);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "My Fancy Page");
    }

    #[test]
    fn record_increments_visit_count() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://x.com", "X");
        m.record_visit("https://x.com", "X");
        m.record_visit("https://x.com", "X");
        assert_eq!(m.get_visit_count("https://x.com"), 3);
    }

    #[test]
    fn record_same_url_does_not_duplicate() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://x.com", "X");
        m.record_visit("https://x.com", "X updated");
        let results = m.search("x.com", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "X updated");
    }

    #[test]
    fn update_title_no_count_bump() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://a.com", "Old");
        m.update_title("https://a.com", "New Title");
        assert_eq!(m.get_visit_count("https://a.com"), 1);
        let r = m.search("a.com", 1);
        assert_eq!(r[0].title, "New Title");
    }

    #[test]
    fn search_returns_empty_on_no_match() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://a.com", "Alpha");
        let r = m.search("zzzzz", 8);
        assert!(r.is_empty());
    }

    #[test]
    fn search_limit_is_respected() {
        let m = mem_manager(HistoryRetention::Hours24);
        for i in 0..20 {
            m.record_visit(&format!("https://site{i}.com"), &format!("Site {i}"));
        }
        let r = m.search("site", 5);
        assert_eq!(r.len(), 5);
    }

    #[test]
    fn session_only_opens_in_memory() {
        let m = mem_manager(HistoryRetention::SessionOnly);
        m.record_visit("https://mem.com", "Mem");
        let r = m.search("mem", 8);
        assert_eq!(r.len(), 1);
    }

    // ── TTL / cleanup ─────────────────────────────────────────────────

    #[test]
    fn cleanup_expired_deletes_old_entries() {
        let m = mem_manager(HistoryRetention::Hours8);
        let now = HistoryManager::now_epoch();
        // Insert an entry 10 hours ago (beyond 8h TTL)
        m.record_visit_at("https://old.com", "Old", now - 36001);
        // Insert a fresh entry
        m.record_visit("https://new.com", "New");
        m.cleanup_expired();
        let old = m.search("old.com", 8);
        let new = m.search("new.com", 8);
        assert!(old.is_empty(), "expired entry should be deleted");
        assert_eq!(new.len(), 1, "fresh entry should survive");
    }

    #[test]
    fn cleanup_expired_noop_on_fresh_entries() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://fresh.com", "Fresh");
        m.cleanup_expired();
        let r = m.search("fresh", 8);
        assert_eq!(r.len(), 1);
    }

    // ── Frecency ordering ─────────────────────────────────────────────

    #[test]
    fn search_orders_by_frecency_not_insertion() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://rare.com", "Rare");
        m.record_visit("https://popular.com", "Popular");
        for _ in 0..10 {
            m.record_visit("https://popular.com", "Popular");
        }
        let r = m.search(".com", 8);
        assert_eq!(r[0].url, "https://popular.com");
    }

    #[test]
    fn search_orders_recent_over_stale_same_count() {
        let m = mem_manager(HistoryRetention::Days30);
        let now = HistoryManager::now_epoch();
        // Old entry: 20 days ago, 1 visit
        m.record_visit_at("https://old.com", "Old", now - 20 * 86400);
        // Recent entry: just now, 1 visit
        m.record_visit("https://recent.com", "Recent");
        let r = m.search(".com", 8);
        assert_eq!(r[0].url, "https://recent.com");
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn record_visit_empty_title_is_valid() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://notitle.com", "");
        let r = m.search("notitle", 8);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "");
    }

    #[test]
    fn record_visit_unicode_url_and_title() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://例え.jp/パス", "日本語タイトル");
        let r = m.search("例え", 8);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "日本語タイトル");
    }

    #[test]
    fn record_visit_url_with_single_quote() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://example.com/it's", "Quote Page");
        let r = m.search("it's", 8);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://a.com", "A");
        let r = m.search("", 8);
        assert!(r.is_empty());
    }

    #[test]
    fn search_special_chars_in_query_do_not_crash() {
        let m = mem_manager(HistoryRetention::Hours24);
        m.record_visit("https://a.com", "A");
        // These should not panic or error — just return empty/partial results
        let _ = m.search("%", 8);
        let _ = m.search("_", 8);
        let _ = m.search("'; DROP TABLE history; --", 8);
        let _ = m.search("\\", 8);
    }

    // ── Disk mode ─────────────────────────────────────────────────────

    #[test]
    fn open_disk_mode_creates_db_file() {
        let dir = std::env::temp_dir().join(format!("epoca_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_history.db");
        let _ = std::fs::remove_file(&path);
        let m = HistoryManager::open_at(path.clone(), HistoryRetention::Hours24);
        m.record_visit("https://disk.com", "Disk");
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("epoca_perm_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("perm_test.db");
        let _ = std::fs::remove_file(&path);
        let _m = HistoryManager::open_at(path.clone(), HistoryRetention::Hours24);
        let perms = std::fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_corrupt_db_recreates() {
        let dir = std::env::temp_dir().join(format!("epoca_corrupt_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("corrupt.db");
        // Write garbage to simulate corruption
        std::fs::write(&path, b"not a valid sqlite database").unwrap();
        let m = HistoryManager::open_at(path.clone(), HistoryRetention::Hours24);
        // Should work fine after recreation
        m.record_visit("https://recovered.com", "Recovered");
        let r = m.search("recovered", 8);
        assert_eq!(r.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_with_directory_path_does_not_panic() {
        let dir = std::env::temp_dir().join(format!("epoca_dirtest_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        // Pass a directory as the "db path" — should fall back to in-memory
        let m = HistoryManager::open_at(dir.clone(), HistoryRetention::Hours24);
        m.record_visit("https://fallback.com", "Fallback");
        let r = m.search("fallback", 8);
        assert_eq!(r.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── is_http_url ───────────────────────────────────────────────────

    #[test]
    fn is_http_url_accepts_http_and_https() {
        assert!(is_http_url("http://example.com"));
        assert!(is_http_url("https://example.com/path?q=1"));
        assert!(is_http_url("https://localhost:3000"));
    }

    #[test]
    fn is_http_url_rejects_file_and_about() {
        assert!(!is_http_url("file:///tmp/index.html"));
        assert!(!is_http_url("about:blank"));
        assert!(!is_http_url("data:text/html,hello"));
        assert!(!is_http_url(""));
    }
}
