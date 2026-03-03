use super::{builtin_lists, ListMeta};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ListFetcher {
    cache_dir: PathBuf,
}

impl ListFetcher {
    pub fn new(cache_dir: PathBuf) -> Self {
        fs::create_dir_all(&cache_dir).ok();
        Self { cache_dir }
    }

    pub fn default_cache_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(home)
            .join(".epoca")
            .join("content-rules")
            .join("lists")
    }

    /// Returns the local path for a list's raw text file.
    pub fn list_path(&self, name: &str) -> PathBuf {
        self.cache_dir.join(format!("{name}.txt"))
    }

    /// Returns the local path for a list's metadata (etag, etc).
    fn meta_path(&self, name: &str) -> PathBuf {
        self.cache_dir.join(format!("{name}.meta.toml"))
    }

    /// Fetch a single list, respecting ETag / If-Modified-Since.
    /// Returns true if the list was updated.
    pub fn fetch_list(&self, list: &mut ListMeta) -> Result<bool, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| e.to_string())?;

        let mut req = client.get(&list.url);
        if let Some(etag) = &list.etag {
            req = req.header("If-None-Match", etag);
        }
        if let Some(lm) = &list.last_modified {
            req = req.header("If-Modified-Since", lm);
        }

        let resp = req.send().map_err(|e| e.to_string())?;

        if resp.status().as_u16() == 304 {
            log::debug!("List '{}' not modified (304)", list.name);
            return Ok(false);
        }

        if !resp.status().is_success() {
            return Err(format!("HTTP {} for {}", resp.status(), list.url));
        }

        // Update metadata from response headers
        if let Some(etag) = resp.headers().get("etag") {
            list.etag = Some(etag.to_str().unwrap_or("").to_string());
        }
        if let Some(lm) = resp.headers().get("last-modified") {
            list.last_modified = Some(lm.to_str().unwrap_or("").to_string());
        }

        let text = resp.text().map_err(|e| e.to_string())?;
        fs::write(self.list_path(&list.name), &text).map_err(|e| e.to_string())?;

        list.last_fetched = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );

        // Persist metadata
        let meta_str = toml::to_string(list).map_err(|e| e.to_string())?;
        fs::write(self.meta_path(&list.name), meta_str).map_err(|e| e.to_string())?;

        log::info!("Updated list '{}'", list.name);
        Ok(true)
    }

    /// Fetch all builtin lists, returns the names of lists that were updated.
    pub fn fetch_all(&self) -> Vec<String> {
        let mut updated = Vec::new();
        let mut lists = builtin_lists();

        // Load persisted metadata if available
        for list in &mut lists {
            let meta_path = self.meta_path(&list.name);
            if let Ok(text) = fs::read_to_string(&meta_path) {
                if let Ok(meta) = toml::from_str::<ListMeta>(&text) {
                    *list = meta;
                }
            }
        }

        for list in &mut lists {
            match self.fetch_list(list) {
                Ok(true) => updated.push(list.name.clone()),
                Ok(false) => {}
                Err(e) => log::warn!("Failed to fetch '{}': {}", list.name, e),
            }
        }

        updated
    }

    /// Read a list's raw text from the local cache.
    pub fn read_cached(&self, name: &str) -> Option<String> {
        fs::read_to_string(self.list_path(name)).ok()
    }
}
