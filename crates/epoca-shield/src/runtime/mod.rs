pub mod scripts;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use scripts::document_end_script;

/// A single compiled WKContentRuleList ready to pass to WebKit.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompiledRuleSet {
    /// Stable identifier used as the WKContentRuleList name.
    /// Format: "epoca-rules-NNN" or "epoca-cosmetic-NNN"
    pub identifier: String,
    /// WKContentRuleList JSON string — pass to WebKit's compiler.
    pub json: String,
    /// Blake3 hash of `json`; used to skip recompilation when unchanged.
    pub content_hash: [u8; 32],
}

/// The full compiled shield configuration for a browsing session.
/// Produced by the compiler, consumed by WebViewTab at construction.
#[derive(Clone, Debug, Default)]
pub struct ShieldConfig {
    /// All compiled network rule sets (split into ≤45k-rule buckets).
    pub rule_sets: Vec<CompiledRuleSet>,
    /// Cosmetic CSS string injected via WKUserScript (document_end).
    pub cosmetic_css: String,
    /// document_start JS blob: fingerprint protection + window.open override.
    pub document_start_script: String,
    /// document_end JS blob: cosmetic removal + overlay sweeper + consent dismiss.
    pub document_end_script: String,
}

/// Per-tab blocked request counters.
#[derive(Clone, Debug, Default)]
pub struct BlockedCounts {
    pub network_blocked: u32,
    pub cosmetic_hidden: u32,
    pub popups_blocked: u32,
    pub fingerprint_events: u32,
}

/// Per-site exception overrides.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SiteException {
    pub disable_network_rules: bool,
    pub disable_cosmetic: bool,
    pub disable_fingerprint: bool,
    pub disable_popup_block: bool,
}

/// The central blocking runtime — holds config + per-tab state.
pub struct ShieldManager {
    pub config: ShieldConfig,
    pub per_tab_counts: HashMap<u64, BlockedCounts>, // tab_id → counts
    pub exceptions: HashMap<String, SiteException>,  // hostname → exception
}

impl ShieldManager {
    pub fn new(config: ShieldConfig) -> Self {
        Self {
            config,
            per_tab_counts: HashMap::new(),
            exceptions: HashMap::new(),
        }
    }

    pub fn default_empty() -> Self {
        Self::new(ShieldConfig::default())
    }

    pub fn record_network_blocked(&mut self, tab_id: u64) {
        self.per_tab_counts
            .entry(tab_id)
            .or_default()
            .network_blocked += 1;
    }

    pub fn record_popup_blocked(&mut self, tab_id: u64) {
        self.per_tab_counts
            .entry(tab_id)
            .or_default()
            .popups_blocked += 1;
    }

    pub fn record_cosmetic_hidden(&mut self, tab_id: u64, count: u32) {
        self.per_tab_counts
            .entry(tab_id)
            .or_default()
            .cosmetic_hidden += count;
    }

    pub fn counts_for(&self, tab_id: u64) -> BlockedCounts {
        self.per_tab_counts
            .get(&tab_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn remove_tab(&mut self, tab_id: u64) {
        self.per_tab_counts.remove(&tab_id);
    }

    pub fn exception_for(&self, hostname: &str) -> Option<&SiteException> {
        self.exceptions.get(hostname)
    }

    pub fn is_fully_disabled_for(&self, hostname: &str) -> bool {
        self.exceptions
            .get(hostname)
            .map(|e| e.disable_network_rules && e.disable_cosmetic && e.disable_fingerprint)
            .unwrap_or(false)
    }
}
