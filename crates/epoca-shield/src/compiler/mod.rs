pub mod content_rules;
pub mod cosmetic;

use crate::fingerprint;
use crate::lists::parser::{parse_filter_list, FilterRule, RuleAction, ResourceType};
use crate::runtime::scripts::document_end_script;
use crate::runtime::ShieldConfig;
pub use content_rules::compile_to_rule_sets;
pub use cosmetic::compile_cosmetic_css;

/// Hardcoded block rules for consent CDN scripts and YouTube ad endpoints.
fn builtin_block_rules() -> Vec<FilterRule> {
    let block = |pattern: &str, types: Vec<ResourceType>| FilterRule {
        url_pattern: pattern.to_string(),
        resource_types: types,
        third_party: None,
        if_domains: vec![],
        unless_domains: vec![],
        action: RuleAction::Block,
    };

    vec![
        // Cookie consent CDN scripts — block before they can render banners
        block(".*cdn\\.cookielaw\\.org.*", vec![ResourceType::Script]),
        block(".*optanon\\.blob\\.core\\.windows\\.net.*", vec![ResourceType::Script]),
        block(".*cdn\\.cookiebot\\.com.*", vec![ResourceType::Script]),
        block(".*consentcdn\\.cookiebot\\.com.*", vec![ResourceType::Script]),
        block(".*cdn-cookieyes\\.com.*", vec![ResourceType::Script]),
        block(".*cookiepro\\.com.*", vec![ResourceType::Script]),
        block(".*privacy-mgmt\\.com.*", vec![ResourceType::Script]),
        block(".*quantcast\\.mgr\\.consensu\\.org.*", vec![ResourceType::Script]),
        block(".*trustarc\\.com.*", vec![ResourceType::Script, ResourceType::SubDocument]),
        block(".*iubenda\\.com.*cs\\.js.*", vec![ResourceType::Script]),
        // YouTube ad telemetry and segment endpoints
        block(".*youtube\\.com/api/stats/ads.*", vec![ResourceType::XmlHttpRequest]),
        block(".*youtube\\.com/api/stats/qoe.*ads.*", vec![ResourceType::XmlHttpRequest]),
        block(".*googlevideo\\.com/ptracking.*", vec![ResourceType::XmlHttpRequest]),
        block(".*youtube\\.com/pagead/.*", vec![ResourceType::Script, ResourceType::XmlHttpRequest]),
        block(".*youtube\\.com/get_midroll_.*", vec![ResourceType::XmlHttpRequest]),
        block(".*doubleclick\\.net/pagead/.*", vec![ResourceType::Script, ResourceType::XmlHttpRequest, ResourceType::Image]),
    ]
}

/// Compile all cached filter list text files into a ShieldConfig.
pub fn compile_all(list_texts: &[(&str, &str)], session_seed: u64) -> ShieldConfig {
    // Parse all rules from all lists
    let mut all_rules: Vec<FilterRule> = Vec::new();
    for (_name, text) in list_texts {
        all_rules.extend(parse_filter_list(text));
    }

    // Hardcoded consent CDN + YouTube ad endpoint blocks
    all_rules.extend(builtin_block_rules());

    log::info!("Parsed {} total filter rules", all_rules.len());

    let rule_sets = compile_to_rule_sets(&all_rules);
    let cosmetic_css = compile_cosmetic_css(&all_rules);
    let document_start_script = fingerprint::generate_script(session_seed);
    let document_end_js = document_end_script(&cosmetic_css);

    log::info!(
        "Compiled {} rule sets ({} network rules buckets), {} cosmetic bytes",
        rule_sets.len(),
        rule_sets.len(),
        cosmetic_css.len(),
    );

    ShieldConfig {
        rule_sets,
        cosmetic_css,
        document_start_script,
        document_end_script: document_end_js,
    }
}
