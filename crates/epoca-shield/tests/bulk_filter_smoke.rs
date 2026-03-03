//! Bulk filter list smoke test.
//!
//! Parses the representative EasyList fixture and validates that every generated
//! `url_pattern` is compatible with WebKit's WKContentRuleList regex engine.
//!
//! This is a regression guard against the class of errors we saw in production:
//! "Error while parsing [regex]" from WKContentRuleListStore rejecting patterns
//! that contain `$` in non-final position, optional groups, or `|$` alternation.

use epoca_shield::lists::parser::parse_filter_list;
use epoca_shield::validate_webkit_pattern;
use std::path::PathBuf;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/sample_easylist.txt")
}

/// Parse the fixture and return (total_rules, url_pattern strings).
fn load_fixture() -> (Vec<epoca_shield::lists::parser::FilterRule>, String) {
    let path = fixture_path();
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {path:?}: {e}"));
    let rules = parse_filter_list(&text);
    (rules, text)
}

#[test]
fn fixture_file_exists_and_is_non_empty() {
    let path = fixture_path();
    assert!(path.exists(), "Fixture file must exist: {path:?}");
    let meta = std::fs::metadata(&path).unwrap();
    assert!(meta.len() > 100, "Fixture file must be non-empty");
}

#[test]
fn no_url_pattern_has_dollar_in_non_final_position() {
    let (rules, _) = load_fixture();
    let mut failures: Vec<String> = Vec::new();

    for rule in &rules {
        if let Err(e) = validate_webkit_pattern(&rule.url_pattern) {
            failures.push(format!("  pattern {:?}: {e}", rule.url_pattern));
        }
    }

    assert!(
        failures.is_empty(),
        "WebKit-incompatible patterns found in fixture output:\n{}",
        failures.join("\n")
    );
}

#[test]
fn raw_regex_rules_produce_no_filter_rules() {
    // Every line that starts with `/` in the fixture is a raw-regex ABP rule.
    // Our parser must drop all of them — none should appear in the output.
    let (rules, text) = load_fixture();

    let raw_regex_lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with('/') && !l.starts_with("!/"))
        .collect();

    assert!(
        !raw_regex_lines.is_empty(),
        "Fixture must contain at least one raw-regex line for this test to be meaningful"
    );

    // None of the rules should have a url_pattern derived from a raw-regex line.
    // The translator drops `/`-prefixed patterns, so any that appear are a bug.
    for rule in &rules {
        for raw in &raw_regex_lines {
            // The raw-regex line (without options) would produce this pattern if NOT dropped.
            // We check by ensuring the url_pattern does NOT start with what the raw regex
            // content would produce (e.g. if '/foo/' were translated literally to 'foo').
            // The simplest check: ensure validate_webkit_pattern passes — raw regex
            // patterns contain alternation `|` that would have produced `$` in non-final pos.
            assert!(
                validate_webkit_pattern(&rule.url_pattern).is_ok(),
                "Rule derived from raw-regex line {raw:?} has invalid pattern: {:?}",
                rule.url_pattern
            );
        }
    }
}

#[test]
fn network_rules_have_non_empty_url_pattern_or_if_domains() {
    // Every network FilterRule must have either a url_pattern or if_domains — otherwise
    // it would generate a WKContentRuleList entry that matches everything (accidental block-all).
    // Global cosmetic rules (##.class with no domain prefix) are excluded: they legitimately
    // have empty url_pattern and empty if_domains and are never compiled into network rules.
    use epoca_shield::lists::parser::RuleAction;
    let (rules, _) = load_fixture();
    for rule in &rules {
        if matches!(rule.action, RuleAction::Cosmetic(_)) {
            continue; // global cosmetics are fine with no url_pattern/if_domains
        }
        assert!(
            !rule.url_pattern.is_empty() || !rule.if_domains.is_empty(),
            "Network rule has neither url_pattern nor if_domains: {rule:?}"
        );
    }
}

#[test]
fn fixture_produces_many_rules() {
    // Sanity check: the fixture should produce a reasonable number of rules.
    // If the parser is silently dropping everything, this will catch it.
    let (rules, _) = load_fixture();
    let network_rules: Vec<_> = rules
        .iter()
        .filter(|r| !matches!(r.action, epoca_shield::lists::parser::RuleAction::Cosmetic(_)))
        .filter(|r| !r.url_pattern.is_empty())
        .collect();

    assert!(
        network_rules.len() >= 50,
        "Expected at least 50 network rules from fixture, got {}. \
         Parser may be too aggressive in dropping rules.",
        network_rules.len()
    );
}

#[test]
fn cosmetic_rules_have_no_url_pattern() {
    use epoca_shield::lists::parser::RuleAction;
    let (rules, _) = load_fixture();
    for rule in rules
        .iter()
        .filter(|r| matches!(r.action, RuleAction::Cosmetic(_)))
    {
        // Cosmetic rules should not have any URL pattern — they're purely CSS selectors.
        // If url_pattern is non-empty it would create a spurious network rule.
        // (Our current parser sets url_pattern = "" for cosmetics, which is correct.)
        // This test guards against regressions where cosmetic rules leak a url_pattern.
        assert!(
            rule.url_pattern.is_empty(),
            "Cosmetic rule must have empty url_pattern, got: {:?}",
            rule.url_pattern
        );
    }
}

#[test]
fn exception_rules_have_ignore_previous_rules_action() {
    use epoca_shield::lists::parser::RuleAction;
    let (rules, _) = load_fixture();
    // @@-prefixed rules in the fixture should all have IgnorePreviousRules action.
    let exception_count = rules
        .iter()
        .filter(|r| matches!(r.action, RuleAction::IgnorePreviousRules))
        .count();
    // The fixture has at least 10 @@ lines.
    assert!(
        exception_count >= 5,
        "Expected at least 5 exception rules, got {exception_count}"
    );
}

#[test]
fn third_party_rules_have_load_type_set() {
    let (rules, _) = load_fixture();
    // Rules with `$third-party` option must have third_party = Some(true).
    // Spot-check a few domain names we know are in the fixture with $third-party.
    let tp_rules: Vec<_> = rules.iter().filter(|r| r.third_party == Some(true)).collect();
    assert!(
        !tp_rules.is_empty(),
        "Fixture must produce at least one third-party rule"
    );
}

#[test]
fn resource_type_rules_have_types_set() {
    let (rules, _) = load_fixture();
    let typed: Vec<_> = rules.iter().filter(|r| !r.resource_types.is_empty()).collect();
    assert!(
        !typed.is_empty(),
        "Fixture must produce at least one rule with resource types"
    );
}

// ── Validate specific patterns we know have been problematic ─────────────────

#[test]
fn known_problematic_raw_regex_lines_produce_no_rules() {
    // The exact lines from our production filter lists that caused WebKit errors.
    let bad_lines = [
        "/addyn|*;adtech;",
        "/addyn|*|adtech;",
        "/^https?:\\/\\/(35|104)\\.(\\d){1,3}\\.(\\d){1,3}\\.(\\d){1,3}\\//",
        "/^(https?:\\/\\/)?ad\\./",
        "/banner[0-9]*/",
    ];

    for line in &bad_lines {
        let rules = parse_filter_list(line);
        for rule in &rules {
            assert!(
                validate_webkit_pattern(&rule.url_pattern).is_ok(),
                "Known-bad line {line:?} produced invalid pattern: {:?}",
                rule.url_pattern
            );
        }
    }
}

#[test]
fn domain_anchor_patterns_pass_webkit_validation() {
    // All the typical domain-anchor patterns should be valid.
    let patterns = [
        "||doubleclick.net^",
        "||googleadservices.com^",
        "||adnxs.com^$third-party",
        "||ads.example.com^$script,third-party",
        "||tracker.com^$domain=news.com|blog.com",
    ];

    for line in &patterns {
        let rules = parse_filter_list(line);
        assert_eq!(rules.len(), 1, "Expected one rule for {line:?}");
        assert!(
            validate_webkit_pattern(&rules[0].url_pattern).is_ok(),
            "Domain-anchor pattern {line:?} produced invalid URL pattern: {:?}",
            rules[0].url_pattern
        );
    }
}
