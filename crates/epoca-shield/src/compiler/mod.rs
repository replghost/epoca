pub mod content_rules;
pub mod cosmetic;

use crate::fingerprint;
use crate::lists::parser::{parse_filter_list, FilterRule};
use crate::runtime::scripts::document_end_script;
use crate::runtime::ShieldConfig;
pub use content_rules::compile_to_rule_sets;
pub use cosmetic::compile_cosmetic_css;

/// Compile all cached filter list text files into a ShieldConfig.
pub fn compile_all(list_texts: &[(&str, &str)], session_seed: u64) -> ShieldConfig {
    // Parse all rules from all lists
    let mut all_rules: Vec<FilterRule> = Vec::new();
    for (_name, text) in list_texts {
        all_rules.extend(parse_filter_list(text));
    }

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
