use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dock::{Panel, PanelEvent, PanelState};
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::theme::ActiveTheme;
use gpui_component::IconName;
use gpui_component::scroll::ScrollableElement;
use gpui_component::Sizable;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use epoca_broker::{CapabilityBroker, PermissionResult};
use epoca_protocol::GuestEvent;
use epoca_sandbox::{SandboxConfig, SandboxInstance};

// ---------------------------------------------------------------------------
// Tab Entry — unified tab tracking for the sidebar
// ---------------------------------------------------------------------------

/// Type-erased navigation capability for a tab.
///
/// Populated by navigable tab types (`WebViewTab`) at construction; `None` for
/// tabs that do not support browser navigation.  Removes the need for
/// `entity.downcast::<WebViewTab>()` scattered through `workbench.rs` — adding
/// a new navigable tab type only requires implementing this trait, not touching
/// any match arms in the workbench.
pub trait NavHandler: Send {
    fn navigate_back(&self, cx: &mut App);
    fn navigate_forward(&self, cx: &mut App);
    fn reload(&self, cx: &mut App);
    fn load_url(&self, url: &str, cx: &mut App);
}

/// A single entry in the workbench tab list.
pub struct TabEntry {
    pub id: u64,
    pub kind: TabKind,
    pub title: String,
    pub icon: IconName,
    pub entity: AnyView,
    /// Pinned tabs persist in the sidebar and are shown in their own section.
    pub pinned: bool,
    /// Navigation delegate — `Some` for navigable tabs, `None` for others.
    pub nav: Option<Box<dyn NavHandler>>,
    /// Favicon URL for WebView tabs — loaded via FAVICON_SCRIPT + epocaFavicon handler.
    /// None until the page reports its icon; falls back to `icon` field for display.
    pub favicon_url: Option<String>,
    /// Session context ID — `None` = isolated (private), `Some(id)` = shared named context.
    pub context_id: Option<String>,
}

/// The kind of tab that can be opened in the workbench.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TabKind {
    Welcome,
    Settings,
    CodeEditor { path: Option<String> },
    SandboxApp { app_id: String },
    FramebufferApp { app_id: String },
    DeclarativeApp { path: String },
    WebView { url: String },
}


// ---------------------------------------------------------------------------
// Welcome Panel
// ---------------------------------------------------------------------------

pub struct WelcomeTab {
    focus_handle: FocusHandle,
}

impl WelcomeTab {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for WelcomeTab {}

impl Focusable for WelcomeTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for WelcomeTab {
    fn panel_name(&self) -> &'static str {
        "WelcomeTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        "Welcome"
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for WelcomeTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(
                div()
                    .text_xl()
                    .text_color(cx.theme().foreground)
                    .child("Welcome to Epoca"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("A cross-platform programmable workbench"),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(Button::new("new-file").primary().label("New File"))
                    .child(Button::new("open-file").label("Open File"))
                    .child(Button::new("open-app").label("Open App")),
            )
    }
}

// ---------------------------------------------------------------------------
// Code Editor Panel (using InputState as a simple text editor for now)
// ---------------------------------------------------------------------------

pub struct CodeEditorTab {
    focus_handle: FocusHandle,
    file_path: Option<String>,
    input_state: Entity<InputState>,
}

impl CodeEditorTab {
    pub fn new(file_path: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let content = file_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();

        let input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx)
                .code_editor("toml")
                .line_number(true);
            if !content.is_empty() {
                state.set_value(content, window, cx);
            }
            state
        });

        Self {
            focus_handle: cx.focus_handle(),
            file_path,
            input_state,
        }
    }

    fn file_name(&self) -> &str {
        self.file_path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("Untitled")
    }
}

impl EventEmitter<PanelEvent> for CodeEditorTab {}

impl Focusable for CodeEditorTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for CodeEditorTab {
    fn panel_name(&self) -> &'static str {
        "CodeEditorTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.file_name().to_string())
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for CodeEditorTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(Label::new(self.file_name().to_string()))
                    .child(
                        Button::new("save")
                            .ghost()
                            .icon(IconName::Check)
                            .label("Save"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .size_full()
                    .child(Input::new(&self.input_state).h_full()),
            )
    }
}

// ---------------------------------------------------------------------------
// Declarative App Panel (TOML markup or ZML → ViewTree)
// ---------------------------------------------------------------------------

use epoca_dsl::{CallbackEntry, ZmlApp};

pub struct DeclarativeAppTab {
    focus_handle: FocusHandle,
    file_path: String,
    view: Entity<crate::view_bridge::SandboxAppView>,
    parse_error: Option<String>,
    _watcher_task: Option<gpui::Task<()>>,
    // ZML-specific state
    zml_app: Option<ZmlApp>,
    zml_state: epoca_dsl::StateStore,
    zml_callbacks: Vec<CallbackEntry>,
    event_queue: Arc<Mutex<Vec<epoca_protocol::GuestEvent>>>,
    broker: Arc<Mutex<CapabilityBroker>>,
}

impl DeclarativeAppTab {
    pub fn new(
        file_path: String,
        broker: Arc<Mutex<CapabilityBroker>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let event_queue: Arc<Mutex<Vec<epoca_protocol::GuestEvent>>> =
            Arc::new(Mutex::new(Vec::new()));
        let eq = event_queue.clone();

        let view = cx.new(|_cx| {
            crate::view_bridge::SandboxAppView::new(move |event| {
                if let Ok(mut q) = eq.lock() {
                    q.push(event);
                }
            })
        });

        let is_zml = file_path.ends_with(".zml");

        let mut tab = Self {
            focus_handle: cx.focus_handle(),
            file_path,
            view,
            parse_error: None,
            _watcher_task: None,
            zml_app: None,
            zml_state: epoca_dsl::StateStore::new(),
            zml_callbacks: Vec::new(),
            event_queue,
            broker,
        };

        tab.reload(cx);
        tab.start_file_watcher(cx);

        // ZML apps need a pump loop for event processing
        if is_zml {
            cx.spawn(async move |this: WeakEntity<Self>, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let Ok(()) = cx.update(|cx| {
                        if let Some(entity) = this.upgrade() {
                            entity.update(cx, |tab, cx| {
                                tab.pump_zml(cx);
                            });
                        }
                    }) else {
                        break;
                    };
                }
            })
            .detach();
        }

        tab
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let content = match std::fs::read_to_string(&self.file_path) {
            Ok(c) => c,
            Err(e) => {
                self.parse_error = Some(format!("Read error: {e}"));
                cx.notify();
                return;
            }
        };

        if self.file_path.ends_with(".zml") {
            self.reload_zml(&content, cx);
        } else {
            self.reload_toml(&content, cx);
        }
    }

    fn reload_toml(&mut self, content: &str, cx: &mut Context<Self>) {
        match crate::declarative::parse_declarative(content) {
            Ok(tree) => {
                self.view.update(cx, |v, _cx| v.set_tree(tree));
                self.parse_error = None;
                log::info!("Declarative app reloaded: {}", self.file_path);
            }
            Err(e) => {
                self.parse_error = Some(format!("Parse error: {e}"));
                log::error!("Failed to parse {}: {e}", self.file_path);
            }
        }
        cx.notify();
    }

    fn reload_zml(&mut self, content: &str, cx: &mut Context<Self>) {
        match epoca_dsl::parse(content) {
            Ok(app) => {
                // Load permissions into broker
                self.load_zml_permissions(&app);

                // Initialize state (only reset if state block changed or first load)
                let prev_state_len = self.zml_app.as_ref().map(|a| a.state_block.len());
                let new_state_len = app.state_block.len();
                let state_changed = prev_state_len != Some(new_state_len)
                    || self.zml_app.is_none();

                if state_changed {
                    self.zml_state = epoca_dsl::StateStore::new();
                    epoca_dsl::init_state(&app.state_block, &mut self.zml_state);
                }

                // Evaluate and render
                let result = epoca_dsl::eval_app(&app, &self.zml_state);
                self.view.update(cx, |v, _cx| v.set_tree(result.tree));
                self.zml_callbacks = result.callbacks;
                self.zml_app = Some(app);
                self.parse_error = None;
                log::info!("ZML app reloaded: {}", self.file_path);
            }
            Err(e) => {
                self.parse_error = Some(format!("{e}"));
                log::error!("Failed to parse ZML {}: {e}", self.file_path);
            }
        }
        cx.notify();
    }

    fn load_zml_permissions(&self, app: &ZmlApp) {
        let app_id = self.app_id();

        // Check for manifest override file
        let manifest_path = std::path::Path::new(&self.file_path)
            .with_extension("manifest.toml");

        if manifest_path.exists() {
            if let Ok(mut b) = self.broker.lock() {
                if let Err(e) = b.load_manifest_file(&app_id, &manifest_path) {
                    log::warn!("Failed to load manifest override for {app_id}: {e}");
                } else {
                    log::info!("Loaded manifest override for {app_id}");
                    return;
                }
            }
        }

        // Use inline permissions
        if let Some(perms) = &app.permissions {
            let manifest_toml = format!(
                "[permissions]\nnetwork = [{}]\ncamera = {}\ngeolocation = \"{}\"\ngpu = \"{}\"\nstorage = \"{}\"",
                perms.network.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", "),
                perms.camera,
                perms.geolocation,
                perms.gpu,
                perms.storage.as_deref().unwrap_or("0"),
            );
            if let Ok(mut b) = self.broker.lock() {
                if let Err(e) = b.load_manifest(&app_id, &manifest_toml) {
                    log::warn!("Failed to load inline permissions for {app_id}: {e}");
                }
            }
        }
    }

    fn pump_zml(&mut self, cx: &mut Context<Self>) {
        let Some(app) = &self.zml_app else {
            return;
        };

        // Drain event queue
        let events: Vec<epoca_protocol::GuestEvent> = if let Ok(mut q) = self.event_queue.lock() {
            q.drain(..).collect()
        } else {
            return;
        };

        if events.is_empty() {
            return;
        }

        let mut state_changed = false;

        for event in events {
            // Find the matching callback
            if let Some(cb_entry) = self.zml_callbacks.iter().find(|c| c.callback_id == event.callback_id) {
                if cb_entry.actions.is_empty() {
                    // This is a bind callback — find the node's bind prop
                    // We need to find the corresponding ViewNode to get bind prop
                    if let Some(tree) = self.view.read(cx).bridge.current_tree() {
                        if let Some(node) = find_node_by_callback(&tree.root, event.callback_id) {
                            epoca_dsl::handle_bind(&node.props, &mut self.zml_state, &event.data);
                            state_changed = true;
                        }
                    }
                } else {
                    // Regular handler actions
                    if let Err(e) = epoca_dsl::exec_actions(
                        &cb_entry.actions,
                        &mut self.zml_state,
                        &event.data,
                    ) {
                        log::error!("ZML action error: {e}");
                    }
                    state_changed = true;
                }
            }
        }

        if state_changed {
            // Re-evaluate
            let result = epoca_dsl::eval_app(app, &self.zml_state);
            self.view.update(cx, |v, _cx| v.set_tree(result.tree));
            self.zml_callbacks = result.callbacks;
            cx.notify();
        }
    }

    fn app_id(&self) -> String {
        // Use the canonical (absolute, symlink-resolved) path so that two files
        // with the same name in different directories get distinct broker entries.
        // Falls back to the raw path if canonicalization fails (e.g. file not yet on disk).
        std::path::Path::new(&self.file_path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&self.file_path))
            .to_string_lossy()
            .into_owned()
    }

    fn start_file_watcher(&mut self, cx: &mut Context<Self>) {
        let path = self.file_path.clone();
        let initial_modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok();

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut last_modified = initial_modified;
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(1))
                    .await;

                let current = std::fs::metadata(&path)
                    .and_then(|m| m.modified())
                    .ok();

                if current != last_modified {
                    last_modified = current;
                    let Ok(()) = cx.update(|cx| {
                        if let Some(entity) = this.upgrade() {
                            entity.update(cx, |tab, cx| {
                                tab.reload(cx);
                            });
                        }
                    }) else {
                        break;
                    };
                }
            }
        });

        self._watcher_task = Some(task);
    }

    fn file_name(&self) -> &str {
        self.file_path
            .rsplit('/')
            .next()
            .unwrap_or(&self.file_path)
    }
}

/// Find a ViewNode by callback ID in the tree.
fn find_node_by_callback(node: &epoca_protocol::ViewNode, cb_id: u64) -> Option<&epoca_protocol::ViewNode> {
    if node.callbacks.iter().any(|c| c.id == cb_id) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_callback(child, cb_id) {
            return Some(found);
        }
    }
    None
}

impl EventEmitter<PanelEvent> for DeclarativeAppTab {}

impl Focusable for DeclarativeAppTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for DeclarativeAppTab {
    fn panel_name(&self) -> &'static str {
        "DeclarativeAppTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.file_name().to_string())
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for DeclarativeAppTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(err) = &self.parse_error {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(gpui::red())
                        .text_color(gpui::white())
                        .child(Label::new(err.clone())),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(self.view.clone())
                .into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(content)
    }
}

// ---------------------------------------------------------------------------
// Sandbox App Panel (renders ViewTree from a PolkaVM guest)
// ---------------------------------------------------------------------------

/// A pending permission request displayed to the user.
struct PendingPermission {
    domain: String,
    _fetch_callback: u64,
}

pub struct SandboxAppTab {
    focus_handle: FocusHandle,
    app_id: String,
    sandbox: Option<SandboxInstance>,
    view: Entity<crate::view_bridge::SandboxAppView>,
    event_queue: Arc<Mutex<Vec<GuestEvent>>>,
    broker: Arc<Mutex<CapabilityBroker>>,
    pending_permission: Option<PendingPermission>,
    error: Option<String>,
}

impl SandboxAppTab {
    /// Create a new sandbox tab by loading a .polkavm file.
    pub fn from_file(
        app_id: String,
        polkavm_path: &std::path::Path,
        broker: Arc<Mutex<CapabilityBroker>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let event_queue: Arc<Mutex<Vec<GuestEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let eq = event_queue.clone();

        let view = cx.new(|_cx| {
            crate::view_bridge::SandboxAppView::new(move |event| {
                if let Ok(mut q) = eq.lock() {
                    q.push(event);
                }
            })
        });

        // Try to load manifest from alongside the .polkavm file
        {
            let manifest_path = polkavm_path.with_extension("manifest.toml");
            if manifest_path.exists() {
                if let Ok(mut b) = broker.lock() {
                    if let Err(e) = b.load_manifest_file(&app_id, &manifest_path) {
                        log::warn!("Failed to load manifest for {}: {e}", app_id);
                    } else {
                        log::info!("Loaded manifest for {} from {}", app_id, manifest_path.display());
                    }
                }
            }
        }

        let config = SandboxConfig::default();
        let mut sandbox = None;
        let mut error = None;

        match SandboxInstance::from_file(polkavm_path, &config) {
            Ok(mut inst) => {
                if let Err(e) = inst.call_init() {
                    error = Some(format!("init failed: {e}"));
                } else {
                    if let Some(tree) = inst.take_view_tree() {
                        view.update(cx, |v, _cx| v.set_tree(tree));
                    }
                }
                sandbox = Some(inst);
            }
            Err(e) => {
                error = Some(format!("load failed: {e}"));
            }
        }

        // Set up a timer to pump update() at ~30fps
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let Ok(()) = cx.update(|cx| {
                    if let Some(entity) = this.upgrade() {
                        entity.update(cx, |tab, cx| {
                            tab.pump_update(cx);
                        });
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            app_id,
            sandbox,
            view,
            event_queue,
            broker,
            pending_permission: None,
            error,
        }
    }

    /// Pump one update cycle: send queued events, call update(), take new view tree.
    fn pump_update(&mut self, cx: &mut Context<Self>) {
        let Some(sandbox) = &mut self.sandbox else {
            return;
        };

        // Drain event queue and send to sandbox
        if let Ok(mut q) = self.event_queue.lock() {
            for event in q.drain(..) {
                sandbox.send_event(event);
            }
        }

        // Call guest update
        if let Err(e) = sandbox.call_update() {
            log::error!("Guest update error: {e}");
            self.error = Some(format!("update error: {e}"));
            return;
        }

        // Check pending network fetches against the broker
        let fetches = sandbox.take_pending_fetches();
        if !fetches.is_empty() {
            if let Ok(broker) = self.broker.lock() {
                for (url, callback_id) in fetches {
                    match broker.check_network(&self.app_id, &url) {
                        PermissionResult::Allowed => {
                            log::info!("Network allowed for {}: {url}", self.app_id);
                            // TODO: actually perform the fetch and send response back
                        }
                        PermissionResult::Denied(reason) => {
                            log::warn!("Network denied for {}: {url} — {reason}", self.app_id);
                        }
                        PermissionResult::NeedsPrompt(_msg) => {
                            // Extract domain for the prompt
                            let domain = url
                                .trim_start_matches("https://")
                                .trim_start_matches("http://")
                                .split('/')
                                .next()
                                .unwrap_or(&url)
                                .to_string();
                            self.pending_permission = Some(PendingPermission {
                                domain,
                                _fetch_callback: callback_id,
                            });
                            cx.notify();
                        }
                    }
                }
            }
        }

        // Take new view tree if any
        if let Some(tree) = sandbox.take_view_tree() {
            self.view.update(cx, |v, _cx| v.set_tree(tree));
            cx.notify();
        }
    }

    fn grant_pending_permission(&mut self, cx: &mut Context<Self>) {
        if let Some(perm) = self.pending_permission.take() {
            if let Ok(mut broker) = self.broker.lock() {
                broker.grant_network(&self.app_id, &perm.domain);
                log::info!("User granted network access to {} for {}", perm.domain, self.app_id);
            }
            cx.notify();
        }
    }

    fn deny_pending_permission(&mut self, cx: &mut Context<Self>) {
        if let Some(perm) = self.pending_permission.take() {
            log::info!("User denied network access to {} for {}", perm.domain, self.app_id);
            cx.notify();
        }
    }
}

impl EventEmitter<PanelEvent> for SandboxAppTab {}

impl Focusable for SandboxAppTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SandboxAppTab {
    fn panel_name(&self) -> &'static str {
        "SandboxAppTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.app_id.clone())
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for SandboxAppTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if let Some(err) = &self.error {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(gpui::red())
                        .text_color(gpui::white())
                        .child(Label::new(err.clone())),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .child(self.view.clone())
                .into_any_element()
        };

        let mut root = div()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(content);

        // Permission prompt overlay
        if let Some(perm) = &self.pending_permission {
            let domain = perm.domain.clone();
            root = root.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .p_3()
                    .flex()
                    .items_center()
                    .gap_3()
                    .bg(cx.theme().warning)
                    .child(
                        Label::new(format!(
                            "\"{}\" wants to access: {}",
                            self.app_id, domain
                        ))
                        .into_element(),
                    )
                    .child(
                        Button::new("perm-allow")
                            .primary()
                            .label("Allow")
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.grant_pending_permission(cx);
                            })),
                    )
                    .child(
                        Button::new("perm-deny")
                            .label("Deny")
                            .on_click(cx.listener(|this, _ev, _window, cx| {
                                this.deny_pending_permission(cx);
                            })),
                    ),
            );
        }

        root
    }
}

// ---------------------------------------------------------------------------
// Framebuffer App Panel — pixel-buffer sandbox tab (e.g. DOOM)
// ---------------------------------------------------------------------------

use epoca_sandbox::{InputEvent, ProdBundle};

/// Shared slot between the background sandbox thread and the GPUI main thread.
struct FramebufferShared {
    /// Latest rendered frame (background thread writes, main thread reads).
    frame: Option<std::sync::Arc<gpui::RenderImage>>,
    /// Input events queued by the main thread for the sandbox.
    input_queue: VecDeque<InputEvent>,
    /// Error from the background thread.
    error: Option<String>,
    /// Set to true when the tab is dropped to stop the background thread.
    stopped: bool,
}

pub struct FramebufferAppTab {
    focus_handle: FocusHandle,
    app_id: String,
    shared: Arc<Mutex<FramebufferShared>>,
    current_frame: Option<std::sync::Arc<gpui::RenderImage>>,
    #[allow(dead_code)]
    broker: Arc<Mutex<CapabilityBroker>>,
    error: Option<String>,
}

impl FramebufferAppTab {
    /// Create from a loaded `.prod` bundle.
    pub fn from_bundle(
        bundle: ProdBundle,
        broker: Arc<Mutex<CapabilityBroker>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let app_id = bundle.manifest.app.id.clone();
        let max_gas = bundle
            .manifest
            .sandbox
            .as_ref()
            .and_then(|s| s.max_gas_per_update)
            .unwrap_or(u64::MAX);

        let config = SandboxConfig {
            max_gas_per_update: max_gas,
            ..Default::default()
        };

        let shared = Arc::new(Mutex::new(FramebufferShared {
            frame: None,
            input_queue: VecDeque::new(),
            error: None,
            stopped: false,
        }));

        let mut init_error = None;

        // Spawn the sandbox on a background thread — all PolkaVM work happens there.
        let shared_bg = shared.clone();
        match SandboxInstance::from_bytes(&bundle.program_bytes, &config) {
            Ok(mut sandbox) => {
                log::info!("[fb] sandbox loaded, {} assets", bundle.assets.len());
                for key in bundle.assets.keys() {
                    log::info!("[fb] asset: {} ({} bytes)", key, bundle.assets[key].len());
                }
                sandbox.load_assets(bundle.assets);
                log::info!("[fb] calling init...");
                if let Err(e) = sandbox.call_init() {
                    log::error!("[fb] init failed: {e}");
                    init_error = Some(format!("init failed: {e}"));
                } else {
                    log::info!("[fb] init succeeded, spawning bg thread");
                    std::thread::Builder::new()
                        .name(format!("fb-{}", app_id))
                        .spawn(move || {
                            Self::bg_loop(sandbox, shared_bg);
                        })
                        .ok();
                }
            }
            Err(e) => {
                log::error!("[fb] load failed: {e}");
                init_error = Some(format!("load failed: {e}"));
            }
        }

        // Poll for new frames from the background thread at ~60fps.
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let Ok(()) = cx.update(|cx| {
                    if let Some(entity) = this.upgrade() {
                        entity.update(cx, |tab, cx| {
                            tab.poll_frame(cx);
                        });
                    }
                }) else {
                    break;
                };
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            app_id,
            shared,
            current_frame: None,
            broker,
            error: init_error,
        }
    }

    /// Background thread loop: runs sandbox update + pixel conversion off the main thread.
    fn bg_loop(mut sandbox: SandboxInstance, shared: Arc<Mutex<FramebufferShared>>) {
        let mut rgba_buf: Vec<u8> = Vec::new();
        let target_dt = std::time::Duration::from_micros(16_667); // ~60fps
        let mut tick_count = 0u64;

        log::info!("[fb-bg] background loop started");

        loop {
            let tick_start = std::time::Instant::now();

            // Check if we should stop, and drain input events into the sandbox.
            {
                let mut s = shared.lock().unwrap();
                if s.stopped {
                    log::info!("[fb-bg] stopped");
                    return;
                }
                while let Some(evt) = s.input_queue.pop_front() {
                    sandbox.send_input(evt);
                }
            }

            // Run one guest update tick.
            if let Err(e) = sandbox.call_update() {
                log::error!("[fb-bg] update error on tick {}: {e}", tick_count);
                let mut s = shared.lock().unwrap();
                s.error = Some(format!("update error: {e}"));
                return;
            }
            tick_count += 1;
            if tick_count <= 5 || tick_count % 60 == 0 {
                log::info!("[fb-bg] tick {} ok", tick_count);
            }

            // If the guest presented a frame, convert to RGBA and build RenderImage.
            // Guest writes 0xAARRGGBB u32s. On little-endian (riscv32) that's [B,G,R,A] in memory.
            if let Some((argb, w, h)) = sandbox.take_framebuffer() {
                if tick_count <= 5 {
                    log::info!("[fb-bg] got frame {}x{} ({} bytes)", w, h, argb.len());
                }
                let pixel_count = (w * h) as usize;
                rgba_buf.resize(pixel_count * 4, 0);
                for i in 0..pixel_count {
                    let base = i * 4;
                    // Memory layout (little-endian): [B, G, R, A]
                    let b = argb[base];
                    let g = argb[base + 1];
                    let r = argb[base + 2];
                    let a = argb[base + 3];
                    // Output: RGBA
                    rgba_buf[base] = r;
                    rgba_buf[base + 1] = g;
                    rgba_buf[base + 2] = b;
                    rgba_buf[base + 3] = a;
                }
                if let Some(img_buf) = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
                    w, h, rgba_buf.clone(),
                ) {
                    let frame = image::Frame::new(img_buf);
                    let render_image = std::sync::Arc::new(
                        gpui::RenderImage::new(smallvec::smallvec![frame]),
                    );
                    let mut s = shared.lock().unwrap();
                    s.frame = Some(render_image);
                }
            }

            // Sleep to maintain target framerate.
            let elapsed = tick_start.elapsed();
            if elapsed < target_dt {
                std::thread::sleep(target_dt - elapsed);
            }
        }
    }

    /// Main thread: pick up the latest frame from the background thread.
    fn poll_frame(&mut self, cx: &mut Context<Self>) {
        let mut s = self.shared.lock().unwrap();

        if let Some(err) = s.error.take() {
            self.error = Some(err);
            cx.notify();
            return;
        }

        if let Some(frame) = s.frame.take() {
            self.current_frame = Some(frame);
            cx.notify();
        }
    }

    /// Translate a GPUI keystroke into a simple key_code for the guest.
    fn keystroke_to_code(keystroke: &gpui::Keystroke) -> Option<u8> {
        let key = keystroke.key_char.as_deref().unwrap_or("");
        // Map common keys to DOOM-compatible scancodes
        match key {
            "w" | "W" => Some(0x11), // up
            "a" | "A" => Some(0x1E), // strafe left
            "s" | "S" => Some(0x1F), // down
            "d" | "D" => Some(0x20), // strafe right
            " " => Some(0x39),       // space (use)
            _ => {
                // Arrow keys and others
                match keystroke.key.as_str() {
                    "up" => Some(0x48),
                    "down" => Some(0x50),
                    "left" => Some(0x4B),
                    "right" => Some(0x4D),
                    "enter" => Some(0x1C),
                    "escape" => Some(0x01),
                    "tab" => Some(0x0F),
                    "shift" => Some(0x2A),
                    "control" => Some(0x1D),
                    _ => {
                        // Single ASCII character
                        if let Some(ch) = key.chars().next() {
                            if ch.is_ascii() {
                                Some(ch as u8)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                }
            }
        }
    }
}

impl EventEmitter<PanelEvent> for FramebufferAppTab {}

impl Focusable for FramebufferAppTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FramebufferAppTab {
    fn panel_name(&self) -> &'static str {
        "FramebufferAppTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(self.app_id.clone())
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Drop for FramebufferAppTab {
    fn drop(&mut self) {
        if let Ok(mut s) = self.shared.lock() {
            s.stopped = true;
        }
    }
}

impl Render for FramebufferAppTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(err) = &self.error {
            return div()
                .track_focus(&self.focus_handle)
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(gpui::red())
                        .text_color(gpui::white())
                        .child(Label::new(err.clone())),
                )
                .into_any_element();
        }

        let content = if let Some(frame) = &self.current_frame {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black())
                .child(
                    gpui::img(gpui::ImageSource::Render(frame.clone()))
                        .size_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Label::new("Waiting for first frame..."))
                .into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .child(content)
            .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _window, _cx| {
                if let Some(code) = Self::keystroke_to_code(&ev.keystroke) {
                    if let Ok(mut s) = this.shared.lock() {
                        s.input_queue.push_back(InputEvent {
                            event_type: 1, // key down
                            key_code: code,
                            _pad: [0, 0],
                        });
                    }
                }
            }))
            .on_key_up(cx.listener(|this, ev: &gpui::KeyUpEvent, _window, _cx| {
                if let Some(code) = Self::keystroke_to_code(&ev.keystroke) {
                    if let Ok(mut s) = this.shared.lock() {
                        s.input_queue.push_back(InputEvent {
                            event_type: 2, // key up
                            key_code: code,
                            _pad: [0, 0],
                        });
                    }
                }
            }))
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------
// WebView Panel
// ---------------------------------------------------------------------------

use gpui_component::webview;

/// Intercepted cmd-click on links to open them in a new tab (background).
/// Tracks document.title changes and reports them via the `epocaMeta`
/// WKScriptMessageHandler. Covers initial load, DOMContentLoaded, SPA
/// pushState/replaceState, and MutationObserver on <title>.
/// Idempotent via window.__epocaTitleTracker.
const TITLE_TRACKER_SCRIPT: &str = r#"(function(){
if(window.__epocaTitleTracker)return;
window.__epocaTitleTracker=true;
function _send(t){
  if(!t||!window.webkit||!window.webkit.messageHandlers||!window.webkit.messageHandlers.epocaMeta)return;
  window.webkit.messageHandlers.epocaMeta.postMessage({type:'titleChanged',title:t});
}
function _check(){var t=document.title;if(t)_send(t);}
// Fire on initial load states
if(document.readyState==='loading'){
  document.addEventListener('DOMContentLoaded',_check);
}else{_check();}
window.addEventListener('load',_check);
// MutationObserver on <title> element
var _titleEl=document.querySelector('title');
if(_titleEl){new MutationObserver(_check).observe(_titleEl,{childList:true,characterData:true,subtree:true});}
// Watch for <title> being added dynamically
new MutationObserver(function(){
  var el=document.querySelector('title');
  if(el&&el!==_titleEl){
    _titleEl=el;
    new MutationObserver(_check).observe(el,{childList:true,characterData:true,subtree:true});
    _check();
  }
}).observe(document.documentElement,{childList:true,subtree:true});
// SPA navigation hooks
(function(){
  function _wrap(orig){return function(){var r=orig.apply(this,arguments);_check();return r;};}
  history.pushState=_wrap(history.pushState);
  history.replaceState=_wrap(history.replaceState);
  window.addEventListener('popstate',_check);
})();
})();"#;

/// Arc-style link status bar: fixed bottom-left pill showing the URL of a
/// hovered link.  While ⌘ is held, shows "Open in new tab: [url]".
/// While ⌘⇧ is held, shows "Open in new tab → switch: [url]".
/// Fades in on hover, fades out on mouse-leave.  Idempotent via window.__epocaStatus.
const LINK_STATUS_SCRIPT: &str = r#"(function(){
if(window.__epocaStatus)return;
window.__epocaStatus=true;
var _bar=document.createElement('div');
_bar.id='__epocaStatusBar';
var _s=_bar.style;
_s.cssText=[
  'position:fixed',
  'bottom:12px',
  'left:12px',
  'max-width:55vw',
  'height:26px',
  'line-height:26px',
  'padding:0 10px',
  'border-radius:13px',
  'background:rgba(34,34,34,0.92)',
  'backdrop-filter:blur(8px)',
  '-webkit-backdrop-filter:blur(8px)',
  'border:1px solid rgba(255,255,255,0.10)',
  'color:rgba(180,180,180,0.85)',
  'font:12px/26px ui-monospace,monospace',
  'white-space:nowrap',
  'overflow:hidden',
  'text-overflow:ellipsis',
  'pointer-events:none',
  'z-index:2147483640',
  'opacity:0',
  'transition:opacity 0.15s ease',
  'box-shadow:0 2px 8px rgba(0,0,0,0.4)',
].join(';');
document.documentElement.appendChild(_bar);
var _cur='';
var _meta=false;
var _shift=false;
function _show(url){
  _cur=url;
  _bar.textContent=(_meta&&_shift)?'Open in new tab \u2192 switch: '+url:(_meta?'Open in new tab: '+url:url);
  _bar.style.color=(_meta)?'rgba(220,220,220,0.95)':'rgba(180,180,180,0.85)';
  _bar.style.opacity='1';
}
function _hide(){_cur='';_bar.style.opacity='0';}
document.addEventListener('mouseover',function(e){
  var el=e.target;while(el&&el.tagName!=='A')el=el.parentElement;
  if(el&&el.href&&!el.href.startsWith('javascript:')){_show(el.href);}
},true);
document.addEventListener('mouseout',function(e){
  var el=e.target;while(el&&el.tagName!=='A')el=el.parentElement;
  if(el&&el.href){_hide();}
},true);
document.addEventListener('keydown',function(e){
  _meta=e.metaKey;_shift=e.shiftKey;
  if(_cur)_show(_cur);
},true);
document.addEventListener('keyup',function(e){
  _meta=e.metaKey;_shift=e.shiftKey;
  if(_cur)_show(_cur);
},true);
})();"#;

/// cmd+shift+click → open with focus (foreground switch).
/// Idempotent via window.__epocaNavInterceptor.
const CMD_CLICK_SCRIPT: &str = r#"(function(){
if(window.__epocaNavInterceptor)return;
window.__epocaNavInterceptor=true;
document.addEventListener('click',function(e){
  if(!e.metaKey)return;
  var el=e.target;
  while(el&&el.tagName!=='A')el=el.parentElement;
  if(!el||!el.href)return;
  var url=el.href;
  if(!url||url.startsWith('javascript:'))return;
  e.preventDefault();
  e.stopPropagation();
  var focus=e.shiftKey;
  if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaNav){
    window.webkit.messageHandlers.epocaNav.postMessage({
      type: focus ? 'openInNewTabFocus' : 'openInNewTab',
      url: url
    });
  }
},true);
})();"#;

/// Injected into every page. Finds the best favicon URL from <link rel="icon">
/// elements or falls back to /favicon.ico, then posts it to epocaFavicon.
/// Fires on DOMContentLoaded and re-fires on SPA navigations.
const FAVICON_SCRIPT: &str = r#"(function(){
if(window.__epocaFavicon)return;
window.__epocaFavicon=true;
function _epocaSendFavicon(){
  var best=null,bestSz=0;
  var links=document.querySelectorAll('link[rel~="icon"],link[rel~="apple-touch-icon"]');
  for(var i=0;i<links.length;i++){
    var l=links[i];
    if(!l.href||!l.href.startsWith('http'))continue;
    var sz=0;
    if(l.sizes&&l.sizes.length>0){
      var s=l.sizes[0]||'';
      sz=parseInt(s.split('x')[0]||'0',10)||0;
    }
    if(!best||sz>bestSz){best=l.href;bestSz=sz;}
  }
  if(!best)best=location.origin+'/favicon.ico';
  if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaFavicon){
    window.webkit.messageHandlers.epocaFavicon.postMessage({type:'faviconFound',url:best});
  }
}
if(document.readyState==='loading'){
  document.addEventListener('DOMContentLoaded',_epocaSendFavicon,{once:true});
}else{_epocaSendFavicon();}
var _origPush=history.pushState,_origReplace=history.replaceState;
history.pushState=function(){_origPush.apply(this,arguments);setTimeout(_epocaSendFavicon,200);};
history.replaceState=function(){_origReplace.apply(this,arguments);setTimeout(_epocaSendFavicon,200);};
window.addEventListener('popstate',function(){setTimeout(_epocaSendFavicon,200);});
})();"#;

/// Injected into every page. When ⌘-click opens a background tab a green
/// expanding ring radiates from the click point — macOS "notification sent"
/// feedback. Fires only on cmd-click (not cmd-shift-click, which is foreground).
const RIPPLE_SCRIPT: &str = r#"(function(){
if(window.__epocaRipple)return;
window.__epocaRipple=true;
document.addEventListener('click',function(e){
  if(!e.metaKey||e.shiftKey)return;
  var el=e.target;while(el&&el.tagName!=='A')el=el.parentElement;
  if(!el||!el.href)return;
  var r=document.createElement('div');
  var x=e.clientX,y=e.clientY,sz=48;
  r.style.cssText='position:fixed;pointer-events:none;border-radius:50%;'
    +'border:2px solid rgba(160,160,160,0.7);background:rgba(160,160,160,0.08);'
    +'left:'+(x-sz/2)+'px;top:'+(y-sz/2)+'px;'
    +'width:'+sz+'px;height:'+sz+'px;'
    +'transform:scale(0.1);opacity:1;z-index:2147483647;'
    +'transition:transform 400ms cubic-bezier(0.25,0.46,0.45,0.94),opacity 380ms ease-out;';
  document.body.appendChild(r);
  r.getBoundingClientRect();
  r.style.transform='scale(4.5)';r.style.opacity='0';
  setTimeout(function(){r.remove();},420);
},true);
})();"#;

/// Tracks mouse hover over links and posts cursor state to epocaCursor.
/// GPUI overrides WKWebView's CSS cursor every frame, so we feed the hover
/// state back through GPUI's own cursor system instead.
/// Idempotent via window.__epocaCursorTracker guard.
const CURSOR_TRACKER_SCRIPT: &str = r#"(function(){
if(window.__epocaCursorTracker)return;
window.__epocaCursorTracker=true;
var _onLink=false;
document.addEventListener('mouseover',function(e){
  var el=e.target;
  while(el&&el.tagName!=='A')el=el.parentElement;
  if(el&&el.href&&!el.href.startsWith('javascript:')){
    if(!_onLink){
      _onLink=true;
      if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaCursor)
        window.webkit.messageHandlers.epocaCursor.postMessage({pointer:true});
    }
  }else if(_onLink){
    _onLink=false;
    if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaCursor)
      window.webkit.messageHandlers.epocaCursor.postMessage({pointer:false});
  }
},true);
document.addEventListener('mouseout',function(e){
  if(!e.relatedTarget&&_onLink){
    _onLink=false;
    if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaCursor)
      window.webkit.messageHandlers.epocaCursor.postMessage({pointer:false});
  }
},true);
})();"#;

/// Intercepts right-click (contextmenu) on links and posts to epocaContextMenu.
/// If the click is not on a link, lets the native context menu through.
/// Idempotent via window.__epocaCtxMenu guard.
const CONTEXT_MENU_SCRIPT: &str = r#"(function(){
if(window.__epocaCtxMenu)return;
window.__epocaCtxMenu=true;
document.addEventListener('contextmenu',function(e){
  var el=e.target;
  while(el&&el.tagName!=='A')el=el.parentElement;
  if(!el||!el.href)return;
  var url=el.href;
  if(!url||url.startsWith('javascript:'))return;
  e.preventDefault();
  if(window.webkit&&window.webkit.messageHandlers&&window.webkit.messageHandlers.epocaContextMenu){
    window.webkit.messageHandlers.epocaContextMenu.postMessage({
      href:url,
      text:(el.textContent||'').trim().substring(0,200),
      x:e.clientX,
      y:e.clientY
    });
  }
},true);
})();"#;

/// Injected into every page at document creation time. Styles the WebKit
/// scrollbar to match the dark chrome look (same technique Arc uses).
const SCROLLBAR_CSS_SCRIPT: &str = r#"(function(){
  var s=document.createElement('style');
  s.textContent='::-webkit-scrollbar{width:8px;height:8px}'
    +'::-webkit-scrollbar-track{border-radius:4px}'
    +'::-webkit-scrollbar-thumb{border-radius:4px}'
    +'@media(prefers-color-scheme:dark){'
      +'::-webkit-scrollbar-track{background:rgba(15,15,15,0.6)}'
      +'::-webkit-scrollbar-thumb{background:rgba(130,130,130,0.75)}'
      +'::-webkit-scrollbar-thumb:hover{background:rgba(180,180,180,0.9)}'
    +'}'
    +'@media(prefers-color-scheme:light){'
      +'::-webkit-scrollbar-track{background:rgba(200,200,200,0.4)}'
      +'::-webkit-scrollbar-thumb{background:rgba(80,80,80,0.45)}'
      +'::-webkit-scrollbar-thumb:hover{background:rgba(50,50,50,0.65)}'
    +'}';
  (document.head||document.documentElement).appendChild(s);
})();"#;

/// Returns the `EpocaSidebarBlocker` NSView subclass, creating and registering it
/// with the ObjC runtime the first time this is called.
///
/// `EpocaSidebarBlocker` wins hit-testing over WKWebView in the sidebar overlay
/// region (because it is inserted above it in the NSView z-order), then
/// forwards every mouse/scroll event to its `nextResponder` (GPUIView) so GPUI
/// can process sidebar button clicks, input fields, etc.
///
/// A plain NSView silently discards events instead of forwarding them — that is
/// why we need a custom subclass here.
#[cfg(target_os = "macos")]
fn passthrough_view_class() -> &'static objc2::runtime::AnyClass {
    use std::sync::OnceLock;
    use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
    use objc2::{msg_send, sel};

    static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
    *CLASS.get_or_init(|| unsafe {
        // Reuse the class if it was already registered (e.g., library hot-reload).
        if let Some(cls) = AnyClass::get("EpocaSidebarBlocker") {
            return cls;
        }
        let superclass = AnyClass::get("NSView").expect("NSView not found");
        let mut builder = ClassBuilder::new("EpocaSidebarBlocker", superclass)
            .expect("failed to create EpocaSidebarBlocker");

        // Single impl reused for all mouse/scroll selectors — they all have the
        // same signature: (self, _cmd, NSEvent*) -> void.
        // Forwards to nextResponder (GPUIView) using performSelector:withObject:
        // so GPUI receives and processes the event.
        // We use *mut AnyObject (raw pointer) as the receiver to avoid the
        // HRTB lifetime issue that arises with `&mut AnyObject` receivers.
        unsafe extern "C" fn forward_event(
            this: *mut AnyObject,
            sel: Sel,
            event: *mut AnyObject,
        ) {
            let next: *mut AnyObject = msg_send![this, nextResponder];
            if !next.is_null() {
                // performSelector:withObject: returns `id` (type code '@'), not void.
                let _: *mut AnyObject = msg_send![next, performSelector: sel withObject: event];
            }
        }

        // Explicitly coerce the fn item to a fn pointer so `MethodImplementation`
        // is satisfied (the trait is only implemented for fn pointer types).
        type ForwardFn = unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject);
        let f = forward_event as ForwardFn;
        builder.add_method(sel!(mouseDown:), f);
        builder.add_method(sel!(mouseUp:), f);
        builder.add_method(sel!(mouseDragged:), f);
        builder.add_method(sel!(rightMouseDown:), f);
        builder.add_method(sel!(rightMouseUp:), f);
        builder.add_method(sel!(otherMouseDown:), f);
        builder.add_method(sel!(otherMouseUp:), f);
        builder.add_method(sel!(scrollWheel:), f);

        builder.register()
    })
}

/// On macOS: apply a CALayer mask to the WKWebView so it only renders in the
/// region x=left_inset..width (in the WKWebView's own coordinate space).
///
/// The sidebar occupies x=0..left_inset of the WKWebView's layer bounds.
/// By masking that region out, GPUI's Metal layer (which renders the sidebar)
/// becomes visible there — the sidebar appears OVER web content with no
/// content shift (the WKWebView frame and thus page viewport are unchanged).
///
/// When left_inset ≤ 0 the mask is removed (WKWebView renders fully).
/// Creates an `EpocaSidebarBlocker` NSView above the WKWebView that intercepts
/// mouse events in the sidebar overlay region and forwards them to GPUIView.
/// CALayer masks clip rendering but NOT NSView hit-testing, so without this
/// the WKWebView steals all clicks before GPUI can process them.
/// Returns the blocker's *mut AnyObject cast to u64, or 0 on failure.
#[cfg(target_os = "macos")]
fn create_sidebar_blocker(wv: &gpui_component::wry::WebView) -> u64 {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use gpui_component::wry::WebViewExtMacOS;
    let wk = wv.webview();
    unsafe {
        let wk_obj = &*wk as *const _ as *mut AnyObject;
        let superview: *mut AnyObject = msg_send![wk_obj, superview];
        if superview.is_null() { return 0; }
        let view_cls = passthrough_view_class();
        let zero = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize { width: 0.0, height: 0.0 },
        };
        let blocker: *mut AnyObject = msg_send![view_cls, alloc];
        let blocker: *mut AnyObject = msg_send![blocker, initWithFrame: zero];
        if blocker.is_null() { return 0; }
        // NSWindowAbove = 1 (NSWindowOrderingMode, NSInteger = i64).
        let _: () = msg_send![superview, addSubview: blocker positioned: 1i64 relativeTo: wk_obj];
        blocker as u64
    }
}

/// Resize the sidebar blocker NSView to cover x = 0..sidebar_width in the
/// superview's coordinate space.  `inset` is in WKWebView-local coords
/// (OverlayLeftInset = (SIDEBAR_W × anim − CHROME).max(0)); adding CHROME
/// converts it back to superview/window coordinates.
#[cfg(target_os = "macos")]
fn update_sidebar_blocker(blocker_ptr: u64, inset: f32) {
    if blocker_ptr == 0 { return; }
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    const CHROME: f32 = 10.0;
    let width = if inset > 0.0 { (inset + CHROME) as f64 } else { 0.0 };
    unsafe {
        let blocker = blocker_ptr as *mut AnyObject;
        let frame = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize { width, height: 100_000.0 },
        };
        let _: () = msg_send![blocker, setFrame: frame];
    }
}

#[cfg(target_os = "macos")]
fn apply_webview_sidebar_mask(wv: &gpui_component::wry::WebView, left_inset: f32) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use gpui_component::wry::WebViewExtMacOS;

    let wk = wv.webview();
    unsafe {
        let obj = &*wk as *const _ as *mut AnyObject;
        let _: () = msg_send![obj, setWantsLayer: objc2::ffi::YES];
        let layer: *mut AnyObject = msg_send![obj, layer];
        if layer.is_null() { return; }

        if left_inset <= 0.5 {
            // Sidebar hidden — remove mask so WKWebView renders fully.
            let null: *const AnyObject = std::ptr::null();
            let _: () = msg_send![layer, setMask: null];
            return;
        }

        // Create an autoreleased CALayer covering x=left_inset..∞ (clamped by
        // the parent layer's bounds automatically).  A non-transparent
        // backgroundColor makes the mask opaque in the visible region.
        let Some(mask_cls) = AnyClass::get("CALayer") else { return };
        let mask_layer: *mut AnyObject = msg_send![mask_cls, layer];
        if mask_layer.is_null() { return; }

        // Frame in the WKWebView layer coordinate space.  Use huge w/h so we
        // don't need to read the layer's actual bounds; CALayer clips the mask
        // to its own bounds automatically.
        let frame = NSRect {
            origin: NSPoint { x: left_inset as f64, y: -10_000.0 },
            size: NSSize { width: 100_000.0, height: 100_000.0 },
        };
        let _: () = msg_send![mask_layer, setFrame: frame];

        // `NSColor.CGColor` returns `CGColorRef` (a CoreFoundation type, type code
        // `^{CGColor=}`), NOT an ObjC object (`@`).  objc2's `msg_send!` validates
        // the return type encoding in debug builds, so we need a stub type whose
        // `Encode` impl produces `^{CGColor=}`.
        #[repr(C)] struct CGColorOpaque;
        // SAFETY: CGColor is an opaque CF struct; we only hold it as a raw pointer.
        unsafe impl objc2::Encode for CGColorOpaque {
            const ENCODING: objc2::Encoding = objc2::Encoding::Struct("CGColor", &[]);
        }
        // `*const CGColorOpaque` requires RefEncode (pointer-to-type encoding).
        unsafe impl objc2::RefEncode for CGColorOpaque {
            const ENCODING_REF: objc2::Encoding =
                objc2::Encoding::Pointer(&<Self as objc2::Encode>::ENCODING);
        }

        let Some(ns_color_cls) = AnyClass::get("NSColor") else { return };
        let white: *mut AnyObject = msg_send![ns_color_cls, whiteColor];
        let cg_color: *const CGColorOpaque = msg_send![white, CGColor];
        if !cg_color.is_null() {
            let _: () = msg_send![mask_layer, setBackgroundColor: cg_color];
        }

        let _: () = msg_send![layer, setMask: mask_layer];
    }
}

/// On macOS: show or hide the WKWebView NSView entirely.
/// Used to keep the native view from rendering above GPUI modal overlays
/// (e.g. the omnibox), since NSView z-order always beats GPUI's Metal layer.
#[cfg(target_os = "macos")]
fn set_webview_hidden(wv: &gpui_component::wry::WebView, hidden: bool) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use gpui_component::wry::WebViewExtMacOS;
    let wk = wv.webview();
    unsafe {
        let obj = &*wk as *const _ as *mut AnyObject;
        let val = if hidden { objc2::ffi::YES } else { objc2::ffi::NO };
        let _: () = msg_send![obj, setHidden: val];
    }
}

/// On macOS: set WKWebView's CALayer corner radius and masksToBounds so the
/// web content (including the scrollbar) is hardware-clipped to a rounded rect
/// at the OS compositor level — exactly what Arc does.
///
/// Requires the webview to be fully built before calling.
#[cfg(target_os = "macos")]
fn apply_webview_corner_radius(wv: &gpui_component::wry::WebView, radius: f64) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use gpui_component::wry::WebViewExtMacOS;
    let wk = wv.webview();
    unsafe {
        let obj = &*wk as *const _ as *mut AnyObject;
        // Ensure the view has a backing layer
        let _: () = msg_send![obj, setWantsLayer: objc2::ffi::YES];
        // Get the CALayer and apply corner radius with clipping
        let layer: *mut AnyObject = msg_send![obj, layer];
        if !layer.is_null() {
            let _: () = msg_send![layer, setCornerRadius: radius];
            let _: () = msg_send![layer, setMasksToBounds: objc2::ffi::YES];
        }
    }
}

/// On macOS: register the `epocaShield` WKScriptMessageHandler name on the
/// WKUserContentController.  For P0 this is a no-op handler — the JS-side
/// `window.webkit.messageHandlers.epocaShield.postMessage(...)` calls are
/// silently dropped but the scripts themselves still execute correctly.
/// P1 will wire a real handler via a channel to ShieldManager.
#[cfg(target_os = "macos")]
/// Returns the WKWebView pointer cast to `usize` (used as a stable tab identity
/// for routing title-change events back to the correct `TabEntry`).
/// Returns 0 on failure (missing configuration / null pointers).
fn install_shield_message_handler(wv: &gpui_component::wry::WebView) -> usize {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use gpui_component::wry::WebViewExtMacOS;

    unsafe {
        let wk = wv.webview();
        let obj = &*wk as *const _ as *mut AnyObject;
        let config: *mut AnyObject = msg_send![obj, configuration];
        if config.is_null() { return 0; }
        let uc: *mut AnyObject = msg_send![config, userContentController];
        if uc.is_null() { return 0; }
        log::debug!("Shield: WKUserContentController at {:p}", uc);
        // Use the WKWebView pointer as a stable tab identity key.
        let webview_ptr = obj as usize;
        crate::shield::register_nav_handler(uc);
        crate::shield::register_meta_handler(uc, webview_ptr);
        crate::shield::register_shield_handler(uc, webview_ptr);
        crate::shield::register_favicon_handler(uc, webview_ptr);
        crate::shield::register_context_menu_handler(uc, webview_ptr);
        crate::shield::register_cursor_handler(uc, webview_ptr);
        #[cfg(feature = "test-server")]
        crate::test_server::register_test_result_handler(uc, webview_ptr);
        webview_ptr
    }
}

pub struct WebViewTab {
    focus_handle: FocusHandle,
    url: String,
    webview: Option<Entity<webview::WebView>>,
    error: Option<String>,
    /// Keeps the OverlayLeftInset observation alive so the native WKWebView
    /// frame is re-laid-out whenever the sidebar animates in or out.
    _inset_subscription: gpui::Subscription,
    /// Keeps the OmniboxOpen observation alive so the WKWebView is hidden
    /// while the omnibox modal is open (NSView z-order puts it above GPUI).
    _omnibox_subscription: gpui::Subscription,
    /// Transparent NSView placed above the WKWebView in the window's NSView
    /// hierarchy to intercept mouse events in the sidebar overlay region.
    /// CALayer masks clip rendering but NOT hit-testing, so without this
    /// the WKWebView consumes all clicks before GPUI sees them.
    /// Stored as u64 (raw *mut AnyObject) so the struct is Send.
    sidebar_blocker_ptr: u64,
    /// Raw WKWebView pointer (cast to usize) used to route title-change events
    /// from `TITLE_CHANNEL` back to the correct `TabEntry` in Workbench.
    pub webview_ptr: usize,
    /// Running count of cosmetic elements hidden by the shield on this tab.
    pub blocked_count: u32,
    /// True when the mouse is hovering an `<a href>` link inside the WebView.
    /// Drives `.cursor_pointer()` on the GPUI wrapper div so GPUI's cursor
    /// system shows the hand cursor instead of overriding WKWebView's CSS cursor.
    pub cursor_pointer: bool,
}

impl WebViewTab {
    pub fn new(url: String, context_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // None = isolated (private), Some = shared named context (persistent store)
        let isolated = context_id.is_none();
        // Observe OverlayLeftInset so this entity is marked dirty — and therefore
        // re-painted by GPUI — whenever the sidebar animation moves. Without this,
        // GPUI may skip re-rendering the entity and the native WKWebView frame
        // stays at its previous position even after the sidebar collapses.
        //
        // We also dispatch a JS resize event so pages with position:fixed overlays
        // (e.g. Google's sign-in card) reflow into the updated viewport bounds.
        // Apply a CALayer mask whenever the sidebar inset changes so the WKWebView
        // doesn't cover the sidebar area.  The mask clips the WKWebView's
        // rendering to x=inset..width (in webview-local coords) while leaving the
        // frame — and thus the page viewport — unchanged (no content reflow).
        let _inset_subscription =
            cx.observe_global::<crate::OverlayLeftInset>(|this: &mut Self, cx| {
                let inset = cx
                    .try_global::<crate::OverlayLeftInset>()
                    .map(|g| g.0)
                    .unwrap_or(0.0);
                if let Some(wv_entity) = &this.webview {
                    let raw = wv_entity.read(cx).raw();
                    #[cfg(target_os = "macos")]
                    apply_webview_sidebar_mask(&raw, inset);
                }
                // Keep the hit-test blocker in sync with the sidebar position.
                #[cfg(target_os = "macos")]
                update_sidebar_blocker(this.sidebar_blocker_ptr, inset);
                cx.notify();
            });

        let _omnibox_subscription =
            cx.observe_global::<crate::OmniboxOpen>(|this: &mut Self, cx| {
                let open = cx
                    .try_global::<crate::OmniboxOpen>()
                    .map(|g| g.0)
                    .unwrap_or(false);
                if let Some(wv_entity) = &this.webview {
                    let raw = wv_entity.read(cx).raw();
                    #[cfg(target_os = "macos")]
                    set_webview_hidden(&raw, open);
                }
                // When the webview un-hides, restore the sidebar blocker.
                if !open {
                    let inset = cx
                        .try_global::<crate::OverlayLeftInset>()
                        .map(|g| g.0)
                        .unwrap_or(0.0);
                    #[cfg(target_os = "macos")]
                    update_sidebar_blocker(this.sidebar_blocker_ptr, inset);
                }
                cx.notify();
            });

        let mut error = None;
        let mut wv_entity = None;
        let mut sidebar_blocker_ptr: u64 = 0;
        let mut webview_ptr: usize = 0;

        // Pull the current shield config (may be default/empty if bootstrap is
        // still running in the background — that's acceptable for early opens).
        let shield = crate::shield::current_config();

        match gpui_component::wry::WebViewBuilder::new()
            .with_url(&url)
            .with_incognito(isolated)
            .with_initialization_script(SCROLLBAR_CSS_SCRIPT)
            .with_initialization_script(TITLE_TRACKER_SCRIPT)
            .with_initialization_script(LINK_STATUS_SCRIPT)
            .with_initialization_script(CMD_CLICK_SCRIPT)
            .with_initialization_script(RIPPLE_SCRIPT)
            .with_initialization_script(FAVICON_SCRIPT)
            .with_initialization_script(CONTEXT_MENU_SCRIPT)
            .with_initialization_script(CURSOR_TRACKER_SCRIPT)
            .with_initialization_script(&shield.document_start_script)
            .with_initialization_script(&shield.document_end_script)
            .build_as_child(window)
        {
            Ok(wry_wv) => {
                // Option B: macOS native CALayer corner radius (true clipping like Arc).
                // Falls back to the GPUI corner-cap overlay (option A) on other platforms.
                #[cfg(target_os = "macos")]
                apply_webview_corner_radius(&wry_wv, 10.0);

                #[cfg(target_os = "macos")]
                { webview_ptr = install_shield_message_handler(&wry_wv); }

                #[cfg(target_os = "macos")]
                { sidebar_blocker_ptr = create_sidebar_blocker(&wry_wv); }

                wv_entity = Some(cx.new(|cx| {
                    webview::WebView::new(wry_wv, window, cx)
                }));
            }
            Err(e) => {
                error = Some(format!("WebView creation failed: {e}"));
                log::error!("Failed to create WebView: {e}");
            }
        }

        Self {
            focus_handle: cx.focus_handle(),
            url,
            webview: wv_entity,
            error,
            _inset_subscription,
            _omnibox_subscription,
            sidebar_blocker_ptr,
            webview_ptr,
            blocked_count: 0,
            cursor_pointer: false,
        }
    }

    pub fn navigate_back(&self, cx: &mut App) {
        if let Some(wv) = &self.webview {
            wv.update(cx, |wv, _cx| {
                let _ = wv.back();
            });
        }
    }

    pub fn navigate_forward(&self, cx: &mut App) {
        if let Some(wv) = &self.webview {
            let _ = wv.read(cx).raw().evaluate_script("history.forward();");
        }
    }

    pub fn reload(&self, cx: &mut App) {
        if let Some(wv) = &self.webview {
            let _ = wv.read(cx).raw().reload();
        }
    }

    pub fn load_url(&self, url: &str, cx: &mut App) {
        if let Some(wv) = &self.webview {
            wv.update(cx, |wv, _cx| {
                wv.load_url(url);
            });
        }
    }

    pub fn evaluate_script(&self, js: &str, cx: &App) {
        if let Some(wv) = &self.webview {
            let _ = wv.read(cx).raw().evaluate_script(js);
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
}

/// `NavHandler` implementation for `WebViewTab`.
/// Captures `Entity<WebViewTab>` so calls can be dispatched without knowledge
/// of the concrete type at the `workbench.rs` call site.
struct WebViewNavHandler(Entity<WebViewTab>);

impl NavHandler for WebViewNavHandler {
    fn navigate_back(&self, cx: &mut App) {
        self.0.update(cx, |tab, cx| tab.navigate_back(cx));
    }
    fn navigate_forward(&self, cx: &mut App) {
        self.0.update(cx, |tab, cx| tab.navigate_forward(cx));
    }
    fn reload(&self, cx: &mut App) {
        self.0.update(cx, |tab, cx| tab.reload(cx));
    }
    fn load_url(&self, url: &str, cx: &mut App) {
        self.0.update(cx, |tab, cx| tab.load_url(url, cx));
    }
}

impl WebViewTab {
    /// Build a `NavHandler` that dispatches to this entity.
    /// Call immediately after `cx.new(|cx| WebViewTab::new(...))`.
    pub fn nav_handler(entity: Entity<Self>) -> Box<dyn NavHandler> {
        Box::new(WebViewNavHandler(entity))
    }

    /// Hard-reload: bypasses cache via `reloadFromOrigin` on macOS.
    pub fn hard_reload(&self, cx: &mut App) {
        if let Some(wv_entity) = &self.webview {
            #[cfg(target_os = "macos")]
            unsafe {
                use objc2::msg_send;
                use objc2::runtime::AnyObject;
                use gpui_component::wry::WebViewExtMacOS;
                let raw = wv_entity.read(cx).raw();
                let wkwebview = raw.webview();
                let ptr = &*wkwebview as *const _ as *mut AnyObject;
                let _: () = msg_send![ptr, reloadFromOrigin];
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = wv_entity.read(cx).raw().evaluate_script("location.reload()");
            }
        }
    }
}

impl Drop for WebViewTab {
    fn drop(&mut self) {
        // Remove the sidebar blocker NSView from the window hierarchy when this
        // tab is closed, so it doesn't linger and intercept clicks for other tabs.
        #[cfg(target_os = "macos")]
        if self.sidebar_blocker_ptr != 0 {
            use objc2::msg_send;
            use objc2::runtime::AnyObject;
            unsafe {
                let blocker = self.sidebar_blocker_ptr as *mut AnyObject;
                let _: () = msg_send![blocker, removeFromSuperview];
            }
        }
    }
}

impl EventEmitter<PanelEvent> for WebViewTab {}

impl Focusable for WebViewTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for WebViewTab {
    fn panel_name(&self) -> &'static str {
        "WebViewTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        SharedString::from(format!("Web: {}", self.url))
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for WebViewTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(err) = &self.error {
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .p_4()
                        .rounded_md()
                        .bg(gpui::red())
                        .text_color(gpui::white())
                        .child(Label::new(err.clone())),
                )
        } else if let Some(wv) = &self.webview {
            let is_pointer = self.cursor_pointer;
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .child(
                    // Window-level cursor override during paint phase: bypasses GPUI's
                    // hitbox hit-testing (which never sees mouse events over the native
                    // WKWebView NSView sitting above GPUI's Metal layer).
                    canvas(
                        |_bounds, _window, _cx| {},
                        move |_bounds, _, window, _cx| {
                            if is_pointer {
                                window.set_window_cursor_style(CursorStyle::PointingHand);
                            }
                        },
                    )
                    .absolute()
                    .size_0(),
                )
                .child(wv.clone())
        } else {
            div()
                .track_focus(&self.focus_handle)
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Label::new("Loading..."))
        }
    }
}

// ---------------------------------------------------------------------------
// Settings Panel
// ---------------------------------------------------------------------------

use crate::settings::{SearchEngine, SettingsGlobal};
use crate::chain::ChainGlobal;
use epoca_chain::{ChainId, ChainState, ConnectionBackend};
use gpui::prelude::FluentBuilder;

pub struct SettingsTab {
    focus_handle: FocusHandle,
    _refresh_task: gpui::Task<()>,
    /// Input entities for editing context names, keyed by context id.
    context_name_inputs: Vec<(String, Entity<InputState>)>,
    /// Subscriptions for context name input blur events.
    _context_subs: Vec<Subscription>,
}

impl SettingsTab {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Refresh the UI every 2 seconds so chain status updates from background threads are visible.
        let _refresh_task = cx.spawn(async move |this: WeakEntity<Self>, cx| loop {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let done = cx
                .update(|cx| {
                    if let Some(entity) = this.upgrade() {
                        entity.update(cx, |_, cx| cx.notify());
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true);
            if done {
                break;
            }
        });
        Self {
            focus_handle: cx.focus_handle(),
            _refresh_task,
            context_name_inputs: Vec::new(),
            _context_subs: Vec::new(),
        }
    }
}

impl SettingsTab {
    /// Ensure context name inputs match the current settings contexts list.
    fn sync_context_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::input::InputEvent;

        let contexts = cx
            .try_global::<SettingsGlobal>()
            .map(|g| g.settings.contexts.clone())
            .unwrap_or_default();

        // Rebuild if IDs differ
        let current_ids: Vec<&str> = self.context_name_inputs.iter().map(|(id, _)| id.as_str()).collect();
        let settings_ids: Vec<&str> = contexts.iter().map(|c| c.id.as_str()).collect();

        if current_ids != settings_ids {
            let mut subs = Vec::new();
            self.context_name_inputs = contexts
                .iter()
                .map(|ctx| {
                    let name = ctx.name.clone();
                    let input = cx.new(|cx| {
                        let mut s = InputState::new(window, cx);
                        s.set_value(name, window, cx);
                        s
                    });
                    // Save name to settings on blur or Enter
                    let ctx_id = ctx.id.clone();
                    let sub = cx.subscribe(&input, move |_this, entity, ev: &InputEvent, cx| {
                        let should_save = matches!(ev, InputEvent::Blur | InputEvent::PressEnter { .. });
                        if should_save {
                            let new_name = entity.read(cx).value().to_string();
                            let cid = ctx_id.clone();
                            if !new_name.is_empty() {
                                cx.update_global::<SettingsGlobal, _>(|g, _| {
                                    if let Some(c) = g.settings.contexts.iter_mut().find(|c| c.id == cid) {
                                        c.name = new_name;
                                    }
                                    g.save();
                                });
                            }
                        }
                    });
                    subs.push(sub);
                    (ctx.id.clone(), input)
                })
                .collect();
            self._context_subs = subs;
        }
    }
}

impl EventEmitter<PanelEvent> for SettingsTab {}

impl Focusable for SettingsTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SettingsTab {
    fn panel_name(&self) -> &'static str {
        "SettingsTab"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        "Settings"
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Render for SettingsTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_context_inputs(window, cx);
        let settings = cx
            .try_global::<SettingsGlobal>()
            .map(|g| g.settings.clone())
            .unwrap_or_default();

        let isolated_tabs = settings.isolated_tabs;
        let shield_enabled = settings.shield_enabled;
        let experimental_chain = settings.experimental_chain;
        let enabled_chains = settings.enabled_chains.clone();
        let search_engine = settings.search_engine;
        let open_links_in_background = settings.open_links_in_background;
        let experimental_contexts = settings.experimental_contexts;
        let session_contexts = settings.contexts.clone();

        // Chain statuses snapshot (read once for this render)
        let chain_statuses: Option<Vec<epoca_chain::ChainStatus>> =
            cx.try_global::<ChainGlobal>().map(|g| g.client.all_statuses());

        let text_primary = rgba(0xffffffff);
        let text_secondary = rgba(0xffffffaa);
        let text_muted = rgba(0xffffff66);
        let border_color = rgba(0xffffff14);
        let section_bg = rgba(0xffffff08);

        // ── Section header ────────────────────────────────────────────────────
        let section_header = |label: &'static str| {
            div()
                .text_xs()
                .text_color(text_muted)
                .mb(px(4.0))
                .child(label)
        };

        // ── Toggle row ────────────────────────────────────────────────────────
        // We can't use a helper fn with cx.listener here (borrow issues), so
        // each toggle is built inline below.

        let toggle_pill = |on: bool| {
            div()
                .w(px(44.0))
                .h(px(24.0))
                .rounded_full()
                .flex()
                .items_center()
                .px(px(2.0))
                .bg(if on { rgba(0x22c55eff) } else { rgba(0x4b5563ff) })
                .when(on, |d| d.justify_end())
                .child(div().w(px(20.0)).h(px(20.0)).rounded_full().bg(gpui::white()))
        };

        // ── Chain status badge ────────────────────────────────────────────────
        let status_badge = |state: &ChainState| -> AnyElement {
            let (dot_color, label) = match state {
                ChainState::Disconnected => {
                    return div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgba(0x6b7280ff)))
                        .child(div().text_xs().text_color(rgba(0x6b7280ff)).child("Disconnected"))
                        .into_any_element();
                }
                ChainState::Connecting => {
                    return div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgba(0xfbbf24ff)))
                        .child(div().text_xs().text_color(rgba(0xfbbf24ff)).child("Connecting…"))
                        .into_any_element();
                }
                ChainState::Syncing { best_block, peers } => {
                    let label = if *peers > 0 {
                        format!("Syncing #{best_block} | {peers} peers")
                    } else {
                        format!("Syncing #{best_block}")
                    };
                    (rgba(0xfbbf24ff), label)
                }
                ChainState::Live { best_block, peers } => {
                    let label = if *peers > 0 {
                        format!("Live #{best_block} | {peers} peers")
                    } else {
                        format!("Live #{best_block}")
                    };
                    (rgba(0x22c55eff), label)
                }
                ChainState::Error(_) => {
                    return div()
                        .flex()
                        .items_center()
                        .gap(px(4.0))
                        .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(rgba(0xef4444ff)))
                        .child(div().text_xs().text_color(rgba(0xef4444ff)).child("Error"))
                        .into_any_element();
                }
            };
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(dot_color))
                .child(div().text_xs().text_color(dot_color).child(label))
                .into_any_element()
        };

        // ── Assemble ──────────────────────────────────────────────────────────
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scrollbar()
            .px(px(28.0))
            .py(px(24.0))
            .flex()
            .flex_col()
            .gap(px(24.0))
            .child(
                div()
                    .text_lg()
                    .text_color(text_primary)
                    .child("Settings"),
            )
            // ── General section ───────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(section_header("GENERAL"))
                    .child(
                        div()
                            .rounded(px(8.0))
                            .bg(section_bg)
                            .border_1()
                            .border_color(border_color)
                            .overflow_hidden()
                            // Isolated Tabs toggle
                            .child(
                                div()
                                    .id("toggle-isolated")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.update_global::<SettingsGlobal, _>(|g, _| {
                                            g.settings.isolated_tabs = !g.settings.isolated_tabs;
                                            g.save();
                                        });
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Isolated Tabs"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Each tab uses a private data store — no cookies, cache, or storage shared between tabs"),
                                            ),
                                    )
                                    .child(toggle_pill(isolated_tabs)),
                            )
                            // Divider
                            .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                            // Open Links in Background toggle
                            .child(
                                div()
                                    .id("toggle-bg-links")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.update_global::<SettingsGlobal, _>(|g, _| {
                                            g.settings.open_links_in_background =
                                                !g.settings.open_links_in_background;
                                            g.save();
                                        });
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Open Links in Background"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Cmd-click opens links without switching tabs"),
                                            ),
                                    )
                                    .child(toggle_pill(open_links_in_background)),
                            )
                            // Divider
                            .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                            // Search engine selector
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Search Engine"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Default search when typing in the address bar"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .children(SearchEngine::all().iter().map(|&engine| {
                                                let is_active = engine == search_engine;
                                                div()
                                                    .id(SharedString::from(format!(
                                                        "engine-{}",
                                                        engine.display_name()
                                                    )))
                                                    .min_w(px(72.0))
                                                    .text_xs()
                                                    .px(px(8.0))
                                                    .py(px(4.0))
                                                    .rounded(px(4.0))
                                                    .cursor_pointer()
                                                    .text_color(if is_active {
                                                        rgba(0xffffffff)
                                                    } else {
                                                        text_secondary
                                                    })
                                                    .bg(if is_active {
                                                        rgba(0x22c55eff)
                                                    } else {
                                                        rgba(0xffffff14)
                                                    })
                                                    .on_click(cx.listener(move |_, _, _, cx| {
                                                        cx.update_global::<SettingsGlobal, _>(
                                                            |g, _| {
                                                                g.settings.search_engine = engine;
                                                                g.save();
                                                            },
                                                        );
                                                        cx.notify();
                                                    }))
                                                    .child(engine.display_name())
                                            })),
                                    ),
                            ),
                    ),
            )
            // ── Privacy section ───────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(section_header("PRIVACY & BLOCKING"))
                    .child(
                        div()
                            .rounded(px(8.0))
                            .bg(section_bg)
                            .border_1()
                            .border_color(border_color)
                            .overflow_hidden()
                            .child(
                                div()
                                    .id("toggle-shield")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.update_global::<SettingsGlobal, _>(|g, _| {
                                            g.settings.shield_enabled = !g.settings.shield_enabled;
                                            g.save();
                                        });
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Content Shield"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Block ads and trackers using EasyList + EasyPrivacy"),
                                            ),
                                    )
                                    .child(toggle_pill(shield_enabled)),
                            ),
                    ),
            )
            // ── Session Contexts ─────────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(section_header("SESSION CONTEXTS"))
                    .child(
                        div()
                            .rounded(px(8.0))
                            .bg(section_bg)
                            .border_1()
                            .border_color(border_color)
                            .overflow_hidden()
                            // Master toggle
                            .child(
                                div()
                                    .id("toggle-contexts")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.update_global::<SettingsGlobal, _>(|g, _| {
                                            g.settings.experimental_contexts =
                                                !g.settings.experimental_contexts;
                                            g.save();
                                        });
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Session Contexts"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Create named contexts to share logins and cookies across tabs. Tabs without a context are fully private."),
                                            ),
                                    )
                                    .child(toggle_pill(experimental_contexts)),
                            )
                            // Context list + add button (only when enabled)
                            .when(experimental_contexts, |d| {
                                let mut container = d;
                                // Render each existing context with editable name
                                for (idx, ctx) in session_contexts.iter().enumerate() {
                                    let ctx_id = ctx.id.clone();
                                    let dot_color = parse_hex_rgba(&ctx.color);
                                    // Find the matching input entity
                                    let name_input = self.context_name_inputs
                                        .iter()
                                        .find(|(id, _)| *id == ctx.id)
                                        .map(|(_, e)| e.clone());
                                    container = container
                                        .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .justify_between()
                                                .px(px(16.0))
                                                .py(px(6.0))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .items_center()
                                                        .gap(px(8.0))
                                                        .flex_1()
                                                        .child(
                                                            div()
                                                                .w(px(8.0))
                                                                .h(px(8.0))
                                                                .rounded_full()
                                                                .bg(dot_color),
                                                        )
                                                        .when_some(name_input, |d, input| {
                                                            d.child(
                                                                Input::new(&input)
                                                                    .appearance(false)
                                                                    .small(),
                                                            )
                                                        }),
                                                )
                                                .child(
                                                    div()
                                                        .id(SharedString::from(format!("del-ctx-{idx}")))
                                                        .cursor_pointer()
                                                        .text_xs()
                                                        .text_color(rgba(0xef444499))
                                                        .hover(|d| d.text_color(rgba(0xef4444ff)))
                                                        .child("Delete")
                                                        .on_click(cx.listener(move |_, _, _, cx| {
                                                            let cid = ctx_id.clone();
                                                            cx.update_global::<SettingsGlobal, _>(|g, _| {
                                                                g.settings.contexts.retain(|c| c.id != cid);
                                                                g.save();
                                                            });
                                                            cx.notify();
                                                        })),
                                                ),
                                        );
                                }
                                // Add Context button
                                container
                                    .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                                    .child(
                                        div()
                                            .id("add-context")
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .px(px(16.0))
                                            .py(px(10.0))
                                            .cursor_pointer()
                                            .text_color(rgba(0x3b82f6cc))
                                            .hover(|d| d.text_color(rgba(0x3b82f6ff)))
                                            .child(gpui_component::Icon::new(IconName::Plus).size(px(12.0)))
                                            .child(div().text_sm().child("Add Context"))
                                            .on_click(cx.listener(|_, _, _, cx| {
                                                cx.update_global::<SettingsGlobal, _>(|g, _| {
                                                    // Pick first unused color from preset palette.
                                                    let used: std::collections::HashSet<&str> =
                                                        g.settings.contexts.iter().map(|c| c.color.as_str()).collect();
                                                    let color = crate::settings::DEFAULT_CONTEXT_COLORS
                                                        .iter()
                                                        .find(|c| !used.contains(**c))
                                                        .unwrap_or(&crate::settings::DEFAULT_CONTEXT_COLORS[0])
                                                        .to_string();
                                                    let idx = g.settings.contexts.len();
                                                    let name = format!("Context {}", idx + 1);
                                                    let id = format!("ctx-{}", uuid_v4_simple());
                                                    g.settings.contexts.push(crate::settings::SessionContext {
                                                        id,
                                                        name,
                                                        color,
                                                    });
                                                    g.save();
                                                });
                                                cx.notify();
                                            })),
                                    )
                            }),
                    ),
            )
            // ── Experimental section ──────────────────────────────────────────
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(section_header("EXPERIMENTAL"))
                    .child(
                        div()
                            .rounded(px(8.0))
                            .bg(section_bg)
                            .border_1()
                            .border_color(border_color)
                            .overflow_hidden()
                            // Blockchain Light Client master toggle
                            .child(
                                div()
                                    .id("toggle-chain")
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .px(px(16.0))
                                    .py(px(12.0))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.update_global::<SettingsGlobal, _>(|g, _| {
                                            g.settings.experimental_chain =
                                                !g.settings.experimental_chain;
                                            // Disconnect all chains when disabling
                                            if !g.settings.experimental_chain {
                                                g.settings.enabled_chains.clear();
                                            }
                                            g.save();
                                        });
                                        // Stop all chain connections if disabled
                                        if cx
                                            .try_global::<SettingsGlobal>()
                                            .map(|g| !g.settings.experimental_chain)
                                            .unwrap_or(true)
                                        {
                                            if cx.has_global::<ChainGlobal>() {
                                                cx.update_global::<ChainGlobal, _>(|g, _| {
                                                    for &id in ChainId::all() {
                                                        g.client.disconnect(id);
                                                    }
                                                });
                                            }
                                        }
                                        cx.notify();
                                    }))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.0))
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(text_primary)
                                                    .child("Blockchain Light Client"),
                                            )
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(text_secondary)
                                                    .child("Directly sync chain state from the peer network — no central server required"),
                                            ),
                                    )
                                    .child(toggle_pill(experimental_chain)),
                            )
                            // Individual chain rows (only when master toggle is on)
                            .when(experimental_chain, |d| {
                                let pol_enabled = enabled_chains.contains("PolkadotAssetHub");
                                let pas_enabled = enabled_chains.contains("PaseoAssetHub");
                                let pre_enabled = enabled_chains.contains("Previewnet");

                                let pol_state = chain_statuses
                                    .as_ref()
                                    .and_then(|v| v.iter().find(|s| s.id == ChainId::PolkadotAssetHub))
                                    .map(|s| s.state.clone())
                                    .unwrap_or(ChainState::Disconnected);
                                let pas_state = chain_statuses
                                    .as_ref()
                                    .and_then(|v| v.iter().find(|s| s.id == ChainId::PaseoAssetHub))
                                    .map(|s| s.state.clone())
                                    .unwrap_or(ChainState::Disconnected);
                                let pre_state = chain_statuses
                                    .as_ref()
                                    .and_then(|v| v.iter().find(|s| s.id == ChainId::Previewnet))
                                    .map(|s| s.state.clone())
                                    .unwrap_or(ChainState::Disconnected);

                                d.child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                                    .child(chain_row(
                                        "pol-chain",
                                        "Polkadot Asset Hub",
                                        pol_enabled,
                                        status_badge(&pol_state),
                                        ChainId::PolkadotAssetHub,
                                        &pol_state,
                                        cx,
                                    ))
                                    .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                                    .child(chain_row(
                                        "pas-chain",
                                        "Paseo Asset Hub",
                                        pas_enabled,
                                        status_badge(&pas_state),
                                        ChainId::PaseoAssetHub,
                                        &pas_state,
                                        cx,
                                    ))
                                    .child(div().h(px(1.0)).mx(px(16.0)).bg(border_color))
                                    .child(chain_row(
                                        "pre-chain",
                                        "Previewnet",
                                        pre_enabled,
                                        status_badge(&pre_state),
                                        ChainId::Previewnet,
                                        &pre_state,
                                        cx,
                                    ))
                            }),
                    ),
            )
    }
}

fn chain_row(
    id: impl Into<ElementId>,
    label: &'static str,
    enabled: bool,
    badge: AnyElement,
    chain_id: ChainId,
    state: &ChainState,
    cx: &mut Context<SettingsTab>,
) -> impl IntoElement {
    let text_primary = rgba(0xffffffff);
    let check_color = if enabled { rgba(0x22c55eff) } else { rgba(0x4b5563ff) };

    // Show first-sync hint for smoldot chains that are still connecting or syncing
    let show_first_sync_hint = chain_id.backend() == ConnectionBackend::Smoldot
        && matches!(state, ChainState::Connecting | ChainState::Syncing { .. });

    // Extract error message for inline display
    let error_msg = match state {
        ChainState::Error(msg) => Some(msg.clone()),
        _ => None,
    };

    div()
        .flex()
        .flex_col()
        .px(px(16.0))
        .py(px(10.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                // Checkbox indicator — only this is clickable
                .child(
                    div()
                        .id(id.into())
                        .w(px(16.0))
                        .h(px(16.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(check_color)
                        .bg(if enabled { check_color } else { rgba(0x00000000) })
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let mut now_enabled = false;
                            cx.update_global::<SettingsGlobal, _>(|g, _| {
                                let key = format!("{chain_id:?}");
                                if g.settings.enabled_chains.contains(&key) {
                                    g.settings.enabled_chains.remove(&key);
                                    now_enabled = false;
                                } else {
                                    g.settings.enabled_chains.insert(key);
                                    now_enabled = true;
                                }
                                g.save();
                            });
                            if cx.has_global::<ChainGlobal>() {
                                cx.update_global::<ChainGlobal, _>(|g, _| {
                                    if now_enabled {
                                        g.client.connect(chain_id);
                                    } else {
                                        g.client.disconnect(chain_id);
                                    }
                                });
                            }
                            cx.notify();
                        }))
                        .when(enabled, |d| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(gpui::white())
                                    .child("✓"),
                            )
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(text_primary)
                        .child(label),
                )
                .child(badge),
        )
        .when(show_first_sync_hint, |d| {
            d.child(
                div()
                    .pl(px(28.0))
                    .pt(px(2.0))
                    .text_xs()
                    .text_color(rgba(0xffffff55))
                    .child("Initial sync may take a few minutes"),
            )
        })
        .when_some(error_msg, |d, msg| {
            d.child(
                div()
                    .pl(px(28.0))
                    .pt(px(2.0))
                    .text_xs()
                    .text_color(rgba(0xef444499))
                    .child(msg),
            )
        })
}

/// Parse a "#rrggbb" hex color string to an Rgba value.
fn parse_hex_rgba(hex: &str) -> Rgba {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        ) {
            return rgba(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff);
        }
    }
    rgba(0xffffff66)
}

/// Simple pseudo-UUID (no external crate needed).
fn uuid_v4_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:016x}", t)
}
