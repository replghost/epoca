use gpui::*;
use epoca_shield::{ShieldConfig, ShieldManager, bootstrap};
use std::sync::{LazyLock, RwLock};

/// Newtype wrapper so `ShieldManager` can be registered as a GPUI global.
/// The orphan rule requires either the trait or the type to be local; this
/// wrapper is local to `epoca-core`.
pub struct ShieldGlobal(pub ShieldManager);
impl Global for ShieldGlobal {}

/// Initialize the shield in a background thread and update the GPUI global
/// when compilation is complete.
/// Call this from Workbench::new or the app entry point.
pub fn init_shield(cx: &mut App) {
    // Register an empty manager immediately so try_global() never panics
    // during the brief startup window before compilation finishes.
    cx.set_global(ShieldGlobal(ShieldManager::default_empty()));

    // Spawn background compilation (blocking I/O + CPU work off the main thread).
    // When done, store the compiled config in COMPILED_CONFIG so newly opened
    // tabs get the full ruleset. Already-open tabs keep their scripts from
    // startup (typically empty at t=0).
    std::thread::spawn(move || {
        log::info!("Shield: starting bootstrap (list fetch + compile)...");
        let config = bootstrap(None);
        log::info!(
            "Shield: compiled {} rule sets, {}b fingerprint script, {}b end script",
            config.rule_sets.len(),
            config.document_start_script.len(),
            config.document_end_script.len(),
        );
        // Store in the static slot so WebViewTab can pull it on next open.
        // We cannot update the GPUI global from a non-GPUI thread directly;
        // the compiled config lives here and is read by current_config().
        COMPILED_CONFIG
            .write()
            .map(|mut guard| *guard = Some(config))
            .ok();
    });
}

/// Global slot for the compiled ShieldConfig, written by the background thread
/// and read by WebViewTab::new.
static COMPILED_CONFIG: LazyLock<RwLock<Option<ShieldConfig>>> =
    LazyLock::new(|| RwLock::new(None));

/// Retrieve the compiled ShieldConfig if available.
/// Returns an empty default if compilation hasn't finished yet.
pub fn current_config() -> ShieldConfig {
    COMPILED_CONFIG
        .read()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}
