pub mod compiler;
pub mod fingerprint;
pub mod lists;
pub mod runtime;

pub use compiler::compile_all;
pub use lists::builtin_lists;
pub use lists::fetcher::ListFetcher;
pub use runtime::{BlockedCounts, CompiledRuleSet, ShieldConfig, ShieldManager, SiteException};

use std::path::PathBuf;

/// Bootstrap the shield: fetch updated lists (if needed) and compile.
/// This is a blocking call — run it on a background thread at startup.
pub fn bootstrap(cache_dir: Option<PathBuf>) -> ShieldConfig {
    let cache_dir = cache_dir.unwrap_or_else(ListFetcher::default_cache_dir);
    let fetcher = ListFetcher::new(cache_dir.clone());

    // Fetch any outdated lists
    let updated = fetcher.fetch_all();
    if !updated.is_empty() {
        log::info!("Updated lists: {:?}", updated);
    }

    // Read all cached lists
    let mut list_texts: Vec<(String, String)> = Vec::new();
    for list in builtin_lists() {
        if let Some(text) = fetcher.read_cached(&list.name) {
            list_texts.push((list.name, text));
        }
    }

    if list_texts.is_empty() {
        log::warn!("No cached filter lists found; shield is inactive");
        return ShieldConfig::default();
    }

    // Generate a session seed from current time
    let session_seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let refs: Vec<(&str, &str)> = list_texts
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();

    compile_all(&refs, session_seed)
}
