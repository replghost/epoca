use gpui::*;
use gpui_component::Root;
use std::path::PathBuf;
use epoca_core::workbench::{
    Workbench, WorkbenchRef, NewTab, CloseActiveTab, FocusUrlBar, Reload, HardReload,
    OpenSettings, OpenAppLibrary, OpenApp, FindInPage, ToggleReaderMode, OpenBookmarks, AddBookmark,
};
use epoca_core::settings::SettingsGlobal;
use epoca_core::chain::ChainGlobal;
use epoca_core::wallet::WalletGlobal;
use epoca_chain::ChainClient;

// App-level actions (not workbench-scoped)
actions!(epoca, [Quit, NewWindow, OpenTestSpa]);

/// Determines what to open based on the CLI argument.
enum OpenTarget {
    PolkaVM(PathBuf),
    ProdBundle(PathBuf),
    Declarative(PathBuf),
    DeclarativeDev(PathBuf),
    WebView(String),
}

fn new_window_opts() -> WindowOptions {
    use std::sync::atomic::{AtomicU32, Ordering};
    static WINDOW_COUNT: AtomicU32 = AtomicU32::new(0);
    let n = WINDOW_COUNT.fetch_add(1, Ordering::Relaxed);
    let offset = (n % 20) as f32 * 22.0; // cascade, wrap after 20

    WindowOptions {
        titlebar: Some(TitlebarOptions {
            appears_transparent: true,
            traffic_light_position: Some(point(px(18.0), px(12.0))),
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(100.0 + offset), px(100.0 + offset)),
            size(px(1280.0), px(800.0)),
        ))),
        ..Default::default()
    }
}

fn main() {
    // Crash reporting (opt-in). Set SENTRY_DSN env var to enable.
    let _sentry = std::env::var("SENTRY_DSN").ok().map(|dsn| {
        sentry::init((
            dsn,
            sentry::ClientOptions {
                release: sentry::release_name!(),
                ..Default::default()
            },
        ))
    });

    env_logger::init();

    let app = Application::new().with_assets(gpui_component_assets::Assets);

    // Parse CLI arguments
    let args: Vec<String> = std::env::args().collect();
    let dev_mode = args.iter().any(|a| a == "--dev");
    let file_arg = args.iter().skip(1).find(|a| !a.starts_with("--"));

    let open_target: Option<OpenTarget> = file_arg.and_then(|arg| {
        if arg.starts_with("http://") || arg.starts_with("https://") {
            return Some(OpenTarget::WebView(arg.clone()));
        }
        let p = PathBuf::from(arg);
        match p.extension().and_then(|e| e.to_str()) {
            Some("polkavm") => Some(OpenTarget::PolkaVM(p)),
            Some("prod") => Some(OpenTarget::ProdBundle(p)),
            Some("toml") | Some("zml") => {
                if dev_mode {
                    Some(OpenTarget::DeclarativeDev(p))
                } else {
                    Some(OpenTarget::Declarative(p))
                }
            }
            _ => None,
        }
    });

    app.run(move |cx| {
        gpui_component::init(cx);

        // Install NSApp-level event monitor so Cmd+key shortcuts work even
        // when WKWebView is first responder.
        #[cfg(target_os = "macos")]
        epoca_core::shield::install_key_monitor();

        // ── Keyboard shortcuts ───────────────────────────────────────────────
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewWindow, None),
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseActiveTab, None),
            KeyBinding::new("cmd-l", FocusUrlBar, None),
            KeyBinding::new("cmd-r", Reload, None),
            KeyBinding::new("cmd-shift-r", HardReload, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-f", FindInPage, None),
            KeyBinding::new("cmd-shift-m", ToggleReaderMode, None),
            KeyBinding::new("cmd-d", AddBookmark, None),
            KeyBinding::new("cmd-shift-b", OpenBookmarks, None),
            KeyBinding::new("cmd-shift-d", OpenTestSpa, None),
            // Global clipboard bindings — allows Edit menu to show shortcuts
            // and enables clipboard forwarding to WKWebView via Workbench handlers.
            KeyBinding::new("cmd-c", gpui_component::input::Copy, None),
            KeyBinding::new("cmd-x", gpui_component::input::Cut, None),
            KeyBinding::new("cmd-v", gpui_component::input::Paste, None),
            KeyBinding::new("cmd-a", gpui_component::input::SelectAll, None),
            KeyBinding::new("cmd-z", gpui_component::input::Undo, None),
            KeyBinding::new("cmd-shift-z", gpui_component::input::Redo, None),
        ]);

        // ── macOS menu bar ───────────────────────────────────────────────────
        cx.set_menus(vec![
            Menu {
                name: "Epoca".into(),
                items: vec![
                    MenuItem::action("Settings", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::os_submenu("Services", SystemMenuType::Services),
                    MenuItem::separator(),
                    MenuItem::action("Quit Epoca", Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Tab", NewTab),
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::separator(),
                    MenuItem::action("Open App...", OpenApp),
                    MenuItem::action("App Library", OpenAppLibrary),
                    MenuItem::action("Bookmarks", OpenBookmarks),
                    MenuItem::action("Add Bookmark", AddBookmark),
                    MenuItem::separator(),
                    MenuItem::action("Close Tab", CloseActiveTab),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Undo", gpui_component::input::Undo, OsAction::Undo),
                    MenuItem::os_action("Redo", gpui_component::input::Redo, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", gpui_component::input::Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", gpui_component::input::Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", gpui_component::input::Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", gpui_component::input::SelectAll, OsAction::SelectAll),
                    MenuItem::separator(),
                    MenuItem::action("Find", FindInPage),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Focus URL Bar", FocusUrlBar),
                    MenuItem::separator(),
                    MenuItem::action("Reload Page", Reload),
                    MenuItem::action("Hard Reload", HardReload),
                    MenuItem::separator(),
                    MenuItem::action("Reader Mode", ToggleReaderMode),
                ],
            },
        ]);

        // ── App-level action handlers ────────────────────────────────────────
        cx.on_action::<Quit>(|_, cx| {
            // Synchronous session save before quit — GPUI tears down entities after quit.
            if let Some(wb_ref) = cx.try_global::<WorkbenchRef>() {
                if let Some(entity) = wb_ref.0.upgrade() {
                    entity.read(cx).save_session(cx);
                }
            }
            cx.quit();
        });
        cx.on_action::<NewWindow>(|_, cx| {
            cx.spawn(async move |cx| {
                cx.open_window(new_window_opts(), |window, cx| {
                    let workbench = cx.new(|cx| {
                        let mut wb = Workbench::new(window, cx);
                        wb.new_tab(window, cx);
                        wb
                    });
                    let view: AnyView = workbench.into();
                    cx.new(|cx| Root::new(view, window, cx))
                })?;
                Ok::<_, anyhow::Error>(())
            })
            .detach();
        });

        cx.on_action::<ToggleReaderMode>(|_, cx| {
            if let Some(wb_ref) = cx.try_global::<WorkbenchRef>() {
                if let Some(entity) = wb_ref.0.upgrade() {
                    entity.update(cx, |wb, cx| wb.toggle_reader_mode(cx));
                }
            }
        });

        cx.on_action::<OpenTestSpa>(|_, cx| {
            if let Some(wb_ref) = cx.try_global::<WorkbenchRef>() {
                if let Some(entity) = wb_ref.0.upgrade() {
                    entity.update(cx, |wb, cx| {
                        // We don't have a Window ref here; use open_spa directly.
                        let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .parent().unwrap().parent().unwrap().to_path_buf();
                        let path = workspace_root.join("examples/test-spa.prod");
                        match epoca_sandbox::ProdBundle::from_file(&path) {
                            Ok(bundle) => {
                                log::info!("[dot] menu: loaded bundle, app_type={}", bundle.manifest.app.app_type);
                            }
                            Err(e) => log::warn!("[dot] menu: failed to load: {e}"),
                        }
                    });
                }
            }
        });

        // Initialize settings and chain globals
        let settings_global = SettingsGlobal::load();
        let chain_client = ChainClient::new();

        // Restore chain connections from saved settings
        if settings_global.settings.experimental_chain {
            for &id in epoca_chain::ChainId::all() {
                if settings_global.settings.enabled_chains.contains(&format!("{id:?}")) {
                    chain_client.connect(id);
                }
            }
        }

        cx.set_global(settings_global);
        cx.set_global(ChainGlobal { client: chain_client });
        cx.set_global(WalletGlobal {
            manager: epoca_wallet::WalletManager::new(),
        });
        epoca_wallet::register_sleep_observer();
        epoca_core::host::init_hostapi(cx);

        // Statement store — ephemeral keypair, always-on gossip.
        // Register callback before starting poll thread to avoid race.
        epoca_core::statements_api::init_network_bridge();
        epoca_chain::statement_store::init();

        cx.spawn(async move |cx| {
            cx.open_window(new_window_opts(), |window, cx| {
                let workbench = cx.new(|cx| {
                    let mut wb = Workbench::new(window, cx);

                    match &open_target {
                        Some(OpenTarget::PolkaVM(path)) => {
                            let name = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("app")
                                .to_string();
                            wb.open_sandbox_app(name, path, window, cx);
                        }
                        Some(OpenTarget::ProdBundle(path)) => {
                            // Install to ~/.epoca/apps/ then open.
                            let _ = epoca_core::app_library::install_prod(path);
                            match epoca_sandbox::ProdBundle::from_file(path) {
                                Ok(bundle) => {
                                    let app_id = bundle.manifest.app.id.clone();
                                    epoca_core::app_library::touch_last_launched(&app_id);
                                    if bundle.manifest.app.app_type == "spa" {
                                        wb.open_spa(bundle, window, cx);
                                    } else {
                                        wb.open_framebuffer_app(bundle, window, cx);
                                    }
                                }
                                Err(e) => {
                                    log::error!("Failed to load .prod bundle: {e}");
                                    wb.new_tab(window, cx);
                                }
                            }
                        }
                        Some(OpenTarget::Declarative(path)) => {
                            let path_str = path.to_string_lossy().to_string();
                            wb.open_declarative_app(path_str, window, cx);
                        }
                        Some(OpenTarget::DeclarativeDev(path)) => {
                            let path_str = path.to_string_lossy().to_string();
                            wb.open_declarative_dev(path_str, window, cx);
                        }
                        Some(OpenTarget::WebView(url)) => {
                            wb.open_webview(url.clone(), window, cx);
                        }
                        None => {
                            // Try session restore; fall back to onboarding page.
                            if !wb.restore_session(window, cx) {
                                wb.open_onboard(window, cx);
                            }
                        }
                    }

                    wb
                });

                // Set WorkbenchRef global so Quit handler can save session.
                cx.set_global(WorkbenchRef(workbench.downgrade()));

                // Auto-open test SPA if DOT_TEST=1
                if std::env::var("DOT_TEST").as_deref() == Ok("1") {
                    workbench.update(cx, |wb, cx| {
                        wb.resolve_dot_url("dot://test-spa.dot", window, cx);
                    });
                }

                let view: AnyView = workbench.into();
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
