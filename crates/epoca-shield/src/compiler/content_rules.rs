use crate::lists::parser::{FilterRule, ResourceType, RuleAction};
use crate::runtime::CompiledRuleSet;
use blake3::Hasher;
use serde_json::{json, Value};

const MAX_RULES_PER_BUCKET: usize = 45_000;

/// Compile a list of FilterRules into WKContentRuleList JSON buckets.
/// WebKit limits each list to ~50k rules; we split at 45k with headroom.
pub fn compile_to_rule_sets(rules: &[FilterRule]) -> Vec<CompiledRuleSet> {
    let json_rules: Vec<Value> = rules
        .iter()
        .filter(|r| !matches!(r.action, RuleAction::Cosmetic(_)))
        .filter_map(filter_rule_to_json)
        .collect();

    // Split into buckets
    let mut sets = Vec::new();
    for (idx, chunk) in json_rules.chunks(MAX_RULES_PER_BUCKET).enumerate() {
        let json_str = serde_json::to_string(chunk).unwrap_or_else(|_| "[]".to_string());

        let mut hasher = Hasher::new();
        hasher.update(json_str.as_bytes());
        let hash = *hasher.finalize().as_bytes();

        sets.push(CompiledRuleSet {
            identifier: format!("epoca-rules-{idx:03}"),
            json: json_str,
            content_hash: hash,
        });
    }

    sets
}

fn filter_rule_to_json(rule: &FilterRule) -> Option<Value> {
    if rule.url_pattern.is_empty() && rule.if_domains.is_empty() {
        return None;
    }

    let mut trigger = json!({
        "url-filter": if rule.url_pattern.is_empty() { ".*" } else { &rule.url_pattern }
    });

    // Resource types
    if !rule.resource_types.is_empty() {
        let types: Vec<&str> = rule
            .resource_types
            .iter()
            .map(|t| match t {
                ResourceType::Script => "script",
                ResourceType::Image => "image",
                ResourceType::Stylesheet => "style-sheet",
                ResourceType::Object => "media",
                ResourceType::XmlHttpRequest => "fetch",
                ResourceType::SubDocument => "document",
                ResourceType::Ping => "ping",
                ResourceType::Media => "media",
                ResourceType::Font => "font",
                ResourceType::Websocket => "websocket",
                ResourceType::Other => "other",
            })
            .collect();
        trigger["resource-type"] = json!(types);
    }

    // Third-party
    if let Some(tp) = rule.third_party {
        trigger["load-type"] = json!(if tp { ["third-party"] } else { ["first-party"] });
    }

    // Domain restrictions
    if !rule.if_domains.is_empty() {
        trigger["if-domain"] = json!(rule.if_domains);
    }
    if !rule.unless_domains.is_empty() {
        trigger["unless-domain"] = json!(rule.unless_domains);
    }

    let action = match &rule.action {
        RuleAction::Block => json!({"type": "block"}),
        RuleAction::BlockCookies => json!({"type": "block-cookies"}),
        RuleAction::IgnorePreviousRules => json!({"type": "ignore-previous-rules"}),
        RuleAction::Cosmetic(_) => return None,
    };

    Some(json!({
        "trigger": trigger,
        "action": action
    }))
}
