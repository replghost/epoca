use crate::lists::parser::{FilterRule, RuleAction};

/// Compile cosmetic filter rules into a single CSS string for injection.
pub fn compile_cosmetic_css(rules: &[FilterRule]) -> String {
    let selectors: Vec<String> = rules
        .iter()
        .filter_map(|r| {
            if let RuleAction::Cosmetic(selector) = &r.action {
                // Only include global rules (no domain restriction) for the universal script
                if r.if_domains.is_empty() {
                    return Some(selector.clone());
                }
            }
            None
        })
        .collect();

    if selectors.is_empty() {
        return String::new();
    }

    format!("{} {{ display: none !important; }}", selectors.join(", "))
}
