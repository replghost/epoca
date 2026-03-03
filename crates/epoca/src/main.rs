use gpui::*;
use gpui_component::Root;
use std::path::PathBuf;
use epoca_core::workbench::{Workbench, NewTab, CloseActiveTab, FocusUrlBar, Reload, HardReload};

// App-level actions (not workbench-scoped)
actions!(epoca, [Quit, NewWindow]);

/// Determines what to open based on the CLI argument.
enum OpenTarget {
    PolkaVM(PathBuf),
    Declarative(PathBuf),
    DeclarativeDev(PathBuf),
    WebView(String),
}

fn main() {
    // Crash reporting. Set SENTRY_DSN to your project DSN to enable.
    // Silently no-ops if the variable is absent.
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

        // ── Keyboard shortcuts ───────────────────────────────────────────────
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-t", NewTab, None),
            KeyBinding::new("cmd-w", CloseActiveTab, None),
            KeyBinding::new("cmd-l", FocusUrlBar, None),
            KeyBinding::new("cmd-r", Reload, None),
            KeyBinding::new("cmd-shift-r", HardReload, None),
        ]);

        // ── macOS menu bar ───────────────────────────────────────────────────
        cx.set_menus(vec![
            Menu {
                name: "Epoca".into(),
                items: vec![
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
                    MenuItem::action("Close Tab", CloseActiveTab),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Focus URL Bar", FocusUrlBar),
                    MenuItem::separator(),
                    MenuItem::action("Reload Page", Reload),
                    MenuItem::action("Hard Reload", HardReload),
                ],
            },
        ]);

        // ── App-level action handlers ────────────────────────────────────────
        cx.on_action::<Quit>(|_, cx| cx.quit());

        cx.spawn(async move |cx| {
            let opts = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    appears_transparent: true,
                    // Position traffic lights inside the sidebar
                    // y=12 → button center at y=19, matching the 38px top-row center.
                    traffic_light_position: Some(point(px(18.0), px(12.0))),
                    ..Default::default()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(100.0), px(100.0)),
                    size(px(1280.0), px(800.0)),
                ))),
                ..Default::default()
            };

            cx.open_window(opts, |window, cx| {
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
                            wb.new_tab(window, cx);
                        }
                    }

                    wb
                });
                let view: AnyView = workbench.into();
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
}
