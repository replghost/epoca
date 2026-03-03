//! Shield smoke test — validates all cached filter lists against WebKit restrictions.
//!
//! Run after Epoca downloads filter lists to catch regex translation errors
//! before they appear at runtime as "Error while parsing [regex]" in WKWebView.
//!
//! Usage:
//!   cargo run -p epoca-shield --bin shield-smoketest
//!   cargo run -p epoca-shield --bin shield-smoketest -- --list-dir /path/to/lists
//!
//! Output:
//!   Per-list stats, any invalid patterns, and a final pass/fail summary.
//!   Exit code 0 = all patterns valid, 1 = invalid patterns found.

use epoca_shield::lists::parser::{parse_filter_list, validate_webkit_pattern};
use std::path::PathBuf;
use std::process;

struct ListResult {
    name: String,
    total_rules: usize,
    network_rules: usize,
    invalid: Vec<InvalidPattern>,
}

struct InvalidPattern {
    pattern: String,
    reason: String,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list_dir = parse_list_dir_arg(&args);

    println!("Epoca Shield Smoke Test");
    println!("=======================");
    println!("Scanning: {}", list_dir.display());
    println!();

    let txt_files = match list_dir_entries(&list_dir) {
        Ok(v) if v.is_empty() => {
            println!(
                "No .txt files found in {}.",
                list_dir.display()
            );
            println!();
            println!("Run Epoca first to download filter lists, or point --list-dir at a directory");
            println!("containing EasyList/AdGuard .txt files.");
            process::exit(0);
        }
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error reading {}: {e}", list_dir.display());
            process::exit(1);
        }
    };

    let mut all_results: Vec<ListResult> = Vec::new();

    for path in &txt_files {
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("  SKIP {name}: {e}");
                continue;
            }
        };

        let rules = parse_filter_list(&text);
        let total_rules = rules.len();

        let mut invalid: Vec<InvalidPattern> = Vec::new();
        let mut network_rules = 0;

        for rule in &rules {
            use epoca_shield::lists::parser::RuleAction;
            if matches!(rule.action, RuleAction::Cosmetic(_)) {
                continue;
            }
            if rule.url_pattern.is_empty() {
                continue;
            }
            network_rules += 1;

            if let Err(reason) = validate_webkit_pattern(&rule.url_pattern) {
                invalid.push(InvalidPattern {
                    pattern: rule.url_pattern.clone(),
                    reason,
                });
            }
        }

        all_results.push(ListResult {
            name,
            total_rules,
            network_rules,
            invalid,
        });
    }

    // Print per-list summary
    let mut any_invalid = false;
    for result in &all_results {
        let status = if result.invalid.is_empty() { "PASS" } else { "FAIL" };
        println!(
            "[{status}] {name}  ({network} network rules, {total} total)",
            status = status,
            name = result.name,
            network = result.network_rules,
            total = result.total_rules,
        );

        if !result.invalid.is_empty() {
            any_invalid = true;
            println!("       Invalid patterns ({}):", result.invalid.len());
            for inv in &result.invalid {
                println!("         {:?}", inv.pattern);
                println!("           → {}", inv.reason);
            }
        }
    }

    println!();

    // Grand total
    let total_lists = all_results.len();
    let total_network: usize = all_results.iter().map(|r| r.network_rules).sum();
    let total_invalid: usize = all_results.iter().map(|r| r.invalid.len()).sum();

    println!("─────────────────────────────────────────────");
    println!("Lists scanned : {total_lists}");
    println!("Network rules : {total_network}");
    println!("Invalid       : {total_invalid}");

    if any_invalid {
        println!();
        println!("RESULT: FAIL — fix the patterns above to prevent WebKit errors at runtime.");
        process::exit(1);
    } else {
        println!();
        println!("RESULT: PASS — all patterns are WebKit-compatible.");
    }
}

fn parse_list_dir_arg(args: &[String]) -> PathBuf {
    // Accept --list-dir <path>
    for i in 0..args.len().saturating_sub(1) {
        if args[i] == "--list-dir" {
            return PathBuf::from(&args[i + 1]);
        }
    }
    // Default: ~/.epoca/content-rules/lists/
    dirs_next()
        .join(".epoca")
        .join("content-rules")
        .join("lists")
}

fn dirs_next() -> PathBuf {
    // home dir: $HOME env, or /tmp as fallback
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn list_dir_entries(dir: &PathBuf) -> std::io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
        .collect();
    files.sort();
    Ok(files)
}
