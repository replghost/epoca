/// A single parsed filter rule in our internal representation.
#[derive(Clone, Debug)]
pub struct FilterRule {
    /// The URL pattern (regex-ready string).
    pub url_pattern: String,
    /// Resource types this rule applies to (empty = all types).
    pub resource_types: Vec<ResourceType>,
    /// Whether this is a third-party-only rule.
    pub third_party: Option<bool>,
    /// If-domain restrictions (rule only fires on these domains).
    pub if_domains: Vec<String>,
    /// Unless-domain restrictions.
    pub unless_domains: Vec<String>,
    /// The action to take.
    pub action: RuleAction,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResourceType {
    Script,
    Image,
    Stylesheet,
    Object,
    XmlHttpRequest,
    SubDocument,
    Ping,
    Media,
    Font,
    Websocket,
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RuleAction {
    Block,
    BlockCookies,
    IgnorePreviousRules,
    /// Cosmetic rule — not a network rule; CSS selector to hide.
    Cosmetic(String),
}

/// Parse a filter list text into a list of FilterRules.
/// Silently skips unsupported or malformed rules.
pub fn parse_filter_list(text: &str) -> Vec<FilterRule> {
    let mut rules = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        // Skip comments and metadata
        if line.is_empty() || line.starts_with('!') || line.starts_with('[') {
            continue;
        }
        // Cosmetic rules: ##selector or #@#selector
        if let Some(idx) = line.find("##") {
            let rest = &line[idx + 2..];
            rules.push(FilterRule {
                url_pattern: String::new(),
                resource_types: vec![],
                third_party: None,
                if_domains: extract_domains_before_cosmetic(line),
                unless_domains: vec![],
                action: RuleAction::Cosmetic(rest.to_string()),
            });
            continue;
        }
        // Exception cosmetic rules: #@#selector — skip for now
        if line.contains("#@#") {
            continue;
        }
        // Network rules
        if let Some(rule) = parse_network_rule(line) {
            rules.push(rule);
        }
    }

    rules
}

fn extract_domains_before_cosmetic(line: &str) -> Vec<String> {
    if let Some(idx) = line.find("##") {
        let domain_part = &line[..idx];
        if domain_part.is_empty() {
            return vec![];
        }
        return domain_part
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    vec![]
}

fn parse_network_rule(line: &str) -> Option<FilterRule> {
    let mut is_exception = false;
    let mut raw = line;

    if raw.starts_with("@@") {
        is_exception = true;
        raw = &raw[2..];
    }

    // Split off options ($script,third-party,etc)
    let (pattern_part, options_part) = if let Some(dollar_idx) = raw.rfind('$') {
        (&raw[..dollar_idx], Some(&raw[dollar_idx + 1..]))
    } else {
        (raw, None)
    };

    // Convert ABP pattern to a rough regex
    let url_pattern = abp_pattern_to_regex(pattern_part);
    if url_pattern.is_empty() {
        return None;
    }

    let mut resource_types = Vec::new();
    let mut third_party: Option<bool> = None;
    let mut if_domains = Vec::new();
    let mut unless_domains = Vec::new();
    let mut block_cookies = false;

    if let Some(opts) = options_part {
        for opt in opts.split(',') {
            let opt = opt.trim();
            match opt {
                "script" => resource_types.push(ResourceType::Script),
                "image" => resource_types.push(ResourceType::Image),
                "stylesheet" => resource_types.push(ResourceType::Stylesheet),
                "object" => resource_types.push(ResourceType::Object),
                "xmlhttprequest" | "xhr" => resource_types.push(ResourceType::XmlHttpRequest),
                "subdocument" => resource_types.push(ResourceType::SubDocument),
                "ping" => resource_types.push(ResourceType::Ping),
                "media" => resource_types.push(ResourceType::Media),
                "font" => resource_types.push(ResourceType::Font),
                "websocket" => resource_types.push(ResourceType::Websocket),
                "third-party" => third_party = Some(true),
                "~third-party" | "first-party" => third_party = Some(false),
                "cookie" | "block-cookies" => block_cookies = true,
                _ if opt.starts_with("domain=") => {
                    for d in opt["domain=".len()..].split('|') {
                        let d = d.trim();
                        if d.starts_with('~') {
                            unless_domains.push(d[1..].to_string());
                        } else if !d.is_empty() {
                            if_domains.push(d.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let action = if is_exception {
        RuleAction::IgnorePreviousRules
    } else if block_cookies {
        RuleAction::BlockCookies
    } else {
        RuleAction::Block
    };

    Some(FilterRule {
        url_pattern,
        resource_types,
        third_party,
        if_domains,
        unless_domains,
        action,
    })
}

/// Validate that a regex pattern is compatible with WebKit's content blocker
/// regex engine, which supports a restricted subset of regular expressions.
/// Returns `Ok(())` if valid, or `Err(reason)` describing the problem.
pub fn validate_webkit_pattern(pattern: &str) -> Result<(), String> {
    // WebKit content blockers don't support these constructs:
    let unsupported: &[(&str, &str)] = &[
        ("(?<=", "lookbehind assertions"),
        ("(?<!", "negative lookbehind assertions"),
        ("(?=", "lookahead assertions"),
        ("(?!", "negative lookahead assertions"),
        ("\\b", "word boundary anchors"),
        ("\\B", "non-word boundary anchors"),
    ];
    for (token, desc) in unsupported {
        if pattern.contains(token) {
            return Err(format!("unsupported: {desc}"));
        }
    }
    // Backreferences: \1 through \9
    for i in 1..=9 {
        let backref = format!("\\{i}");
        if pattern.contains(&backref) {
            return Err("unsupported: backreferences".to_string());
        }
    }
    // Check that the pattern compiles as a valid regex at all
    if regex::Regex::new(pattern).is_err() {
        return Err("invalid regex syntax".to_string());
    }
    Ok(())
}

/// Convert an ABP-style URL pattern to a regex string.
fn abp_pattern_to_regex(pattern: &str) -> String {
    if pattern.is_empty() || pattern == "*" {
        return ".*".to_string();
    }

    let mut out = String::new();
    let mut chars = pattern.chars().peekable();

    // Handle || (domain anchor)
    let domain_anchor = pattern.starts_with("||");
    if domain_anchor {
        chars.next();
        chars.next(); // consume ||
        out.push_str("(https?://)?([^/]*\\.)?");
    } else if pattern.starts_with('|') {
        chars.next(); // consume leading |
        out.push('^');
    }

    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str(".*"),
            '^' => out.push_str("([/?#]|$)"),
            '|' => out.push('$'), // trailing anchor
            '.' | '+' | '?' | '{' | '}' | '[' | ']' | '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }

    out
}
