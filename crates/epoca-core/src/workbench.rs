use gpui::prelude::FluentBuilder;
use gpui::*;
use crate::shield::init_shield;

// ── Workbench-scoped actions ────────────────────────────────────────────────
actions!(workbench, [NewTab, CloseActiveTab, FocusUrlBar, Reload, HardReload, ToggleSiteShield, OpenSettings, FindInPage, FindPrev, CloseFindBar]);
use gpui_component::PixelsExt as _;
use crate::{OmniboxOpen, OverlayLeftInset};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Icon, IconName};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use epoca_broker::CapabilityBroker;

/// Set the macOS traffic-light buttons' opacity so they fade in/out with the sidebar.
/// alpha=0.0 → fully invisible + non-interactive (setHidden:YES).
/// alpha=1.0 → fully opaque.
/// Must be called on the main thread.
#[cfg(target_os = "macos")]
fn set_traffic_lights_alpha(alpha: f32) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    let alpha = alpha.clamp(0.0, 1.0);
    // Use setHidden:YES for low alpha so macOS hover/focus events can't reset us.
    let hidden = alpha < 0.05;
    unsafe {
        let Some(app_cls) = AnyClass::get("NSApplication") else { return };
        let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
        if app.is_null() { return; }
        let window: *mut AnyObject = msg_send![app, keyWindow];
        if window.is_null() { return; }
        let hidden_val = if hidden { objc2::ffi::YES } else { objc2::ffi::NO };
        for kind in [0usize, 1, 2] {
            let btn: *mut AnyObject = msg_send![window, standardWindowButton: kind];
            if !btn.is_null() {
                let _: () = msg_send![btn, setHidden: hidden_val];
                if !hidden {
                    let _: () = msg_send![btn, setAlphaValue: alpha as f64];
                }
            }
        }
    }
}

/// Returns true when the key window is in macOS native fullscreen mode.
/// In fullscreen macOS manages traffic-light visibility itself; we must not
/// call set_traffic_lights_hidden() or the user loses the ability to exit.
#[cfg(target_os = "macos")]
fn is_window_fullscreen() -> bool {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    unsafe {
        let Some(app_cls) = AnyClass::get("NSApplication") else { return false };
        let app: *mut AnyObject = msg_send![app_cls, sharedApplication];
        if app.is_null() { return false; }
        let window: *mut AnyObject = msg_send![app, keyWindow];
        if window.is_null() { return false; }
        // NSWindowStyleMaskFullScreen = 1 << 14
        let mask: usize = msg_send![window, styleMask];
        mask & (1 << 14) != 0
    }
}

#[cfg(not(target_os = "macos"))]
fn is_window_fullscreen() -> bool {
    false
}

use crate::tabs::{
    CodeEditorTab, DeclarativeAppTab, FramebufferAppTab, SandboxAppTab, SettingsTab, TabEntry,
    TabKind, WebViewTab,
};
use epoca_sandbox::ProdBundle;

const SIDEBAR_W: f32 = 260.0;
/// Hover-zone width at the left edge that triggers sidebar reveal.
const EDGE_ZONE: f32 = 8.0;
/// Easing factor per frame (exponential ease-out).
const ANIM_EASE: f32 = 0.22;

/// Sidebar display mode.
#[derive(Clone, Copy, PartialEq)]
pub enum SidebarMode {
    /// Always visible, pushes content to its right.
    Pinned,
    /// Overlays content, shown/hidden on hover near the left edge.
    Overlay,
}

/// GPUI global that stores a weak reference to the primary Workbench.
/// Used by the Quit handler to trigger a synchronous session save.
pub struct WorkbenchRef(pub WeakEntity<Workbench>);
impl Global for WorkbenchRef {}

/// Arc browser-style workbench.
pub struct Workbench {
    // Sidebar
    pub(crate) sidebar_mode: SidebarMode,
    /// 0.0 = fully hidden, 1.0 = fully shown (drives layout + overlay position).
    pub(crate) sidebar_anim: f32,
    /// The value `sidebar_anim` is animating toward (0.0 or 1.0).
    sidebar_target: f32,
    /// Running animation frame task — dropped to cancel.
    _sidebar_anim_task: Option<Task<()>>,
    /// Monotonically-increasing counter. Each new task captures the value at
    /// spawn time; if the counter has advanced when the task wakes, the task
    /// is stale and exits. Prevents rapid-toggle from leaving multiple
    /// concurrent animation loops in flight.
    sidebar_anim_gen: u64,

    // Tabs
    pub(crate) tabs: Vec<TabEntry>,
    pub(crate) active_tab_id: Option<u64>,
    next_tab_id: u64,

    // URL bar
    pub(crate) url_input: Entity<InputState>,
    _url_subscription: Subscription,
    pending_nav: Option<String>,

    // Omnibox
    pub(crate) omnibox_open: bool,
    omnibox_input: Entity<InputState>,
    _omnibox_subscription: Subscription,
    omnibox_pending_nav: Option<String>,

    // Broker
    broker: Arc<Mutex<CapabilityBroker>>,

    /// When true, every new WebView tab gets a non-persistent data store
    /// (WKWebsiteDataStore.nonPersistentDataStore on macOS). No cookies,
    /// localStorage, or cache is shared between tabs or sessions.
    /// Defeats per-tab metered paywalls and cross-tab tracking.
    pub isolated_tabs: bool,

    /// Currently selected session context (experimental). `None` = private/isolated.
    /// When `experimental_contexts` is on, new WebView tabs inherit this value.
    pub active_context: Option<String>,

    /// Whether the context picker dropdown is open.
    context_picker_open: bool,

    // Session restore
    _session_save_task: Option<Task<()>>,

    // Find-in-page
    pub(crate) find_open: bool,
    pub(crate) find_input: Entity<InputState>,
    _find_subscription: Subscription,
}

/// Extract the hostname from a URL string without pulling in the `url` crate.
/// e.g. "https://example.com/path" → "example.com"
fn hostname_from_url(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = rest.split('/').next().unwrap_or(rest);
    host.split(':').next().unwrap_or(host)
}

impl Workbench {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Start shield bootstrap in background (list fetch + compile).
        init_shield(cx);

        // Start embedded test server if EPOCA_TEST=1 is set.
        #[cfg(feature = "test-server")]
        crate::test_server::maybe_start();

        let url_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search or enter URL")
        });

        let _url_subscription = cx.subscribe(&url_input, Self::on_url_input_event);

        let omnibox_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search or open URL...")
        });

        let _omnibox_subscription =
            cx.subscribe(&omnibox_input, Self::on_omnibox_input_event);

        let find_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Find in page…")
        });
        let _find_subscription = cx.subscribe(&find_input, Self::on_find_input_event);

        let broker =
            CapabilityBroker::new().with_storage("epoca_permissions.json".to_string());

        // Periodic session save every 30 seconds.
        let session_save_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(30))
                    .await;
                let should_continue = cx
                    .update(|cx| {
                        if let Some(entity) = this.upgrade() {
                            entity.read(cx).save_session(cx);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        });

        Self {
            sidebar_mode: SidebarMode::Pinned,
            sidebar_anim: 1.0,
            sidebar_target: 1.0,
            _sidebar_anim_task: None,
            sidebar_anim_gen: 0,
            tabs: Vec::new(),
            active_tab_id: None,
            next_tab_id: 1,
            url_input,
            _url_subscription,
            pending_nav: None,
            omnibox_open: false,
            omnibox_input,
            _omnibox_subscription,
            omnibox_pending_nav: None,
            broker: Arc::new(Mutex::new(broker)),
            isolated_tabs: false,
            active_context: None,
            context_picker_open: false,
            _session_save_task: Some(session_save_task),
            find_open: false,
            find_input,
            _find_subscription,
        }
    }

    // ------------------------------------------------------------------
    // Sidebar animation
    // ------------------------------------------------------------------

    /// Show the overlay sidebar, optionally after a short delay (to avoid
    /// triggering while the mouse is just sweeping past the left edge).
    fn trigger_sidebar_show(&mut self, with_delay: bool, cx: &mut Context<Self>) {
        if self.sidebar_target >= 1.0 {
            return; // already showing or animating in
        }
        self.sidebar_target = 1.0;
        self.start_anim_task(if with_delay { 250 } else { 0 }, cx);
    }

    /// Hide the overlay sidebar immediately (no delay).
    fn trigger_sidebar_hide(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_target <= 0.0 {
            return;
        }
        self.sidebar_target = 0.0;
        // Dismiss context picker when sidebar hides
        self.context_picker_open = false;
        self.start_anim_task(0, cx);
    }

    /// Spawn (or restart) the frame-rate animation task that moves
    /// `sidebar_anim` toward `sidebar_target`.
    fn start_anim_task(&mut self, initial_delay_ms: u64, cx: &mut Context<Self>) {
        // Advance the generation counter. The new task captures this value; any
        // previously-spawned task whose captured generation no longer matches
        // will exit on its next wake — preventing two loops running in parallel
        // after a rapid toggle even if the old Task handle wasn't fully cancelled.
        self.sidebar_anim_gen = self.sidebar_anim_gen.wrapping_add(1);
        let my_gen = self.sidebar_anim_gen;

        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            if initial_delay_ms > 0 {
                cx.background_executor()
                    .timer(Duration::from_millis(initial_delay_ms))
                    .await;
                // Cancel if target flipped back during the delay, or if a newer
                // task was spawned (generation advanced).
                let still_valid = cx
                    .update(|cx| {
                        this.upgrade()
                            .map(|e| {
                                let wb = e.read(cx);
                                wb.sidebar_anim_gen == my_gen && wb.sidebar_target >= 1.0
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                if !still_valid {
                    return;
                }
            }

            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16)) // ~60 fps
                    .await;

                let done = cx
                    .update(|cx| -> bool {
                        let Some(entity) = this.upgrade() else {
                            return true;
                        };
                        let mut finished = false;
                        entity.update(cx, |wb, cx| {
                            // Exit immediately if a newer task has taken over.
                            if wb.sidebar_anim_gen != my_gen {
                                finished = true;
                                return;
                            }
                            let target = wb.sidebar_target;
                            let diff = target - wb.sidebar_anim;
                            if diff.abs() < 0.005 {
                                wb.sidebar_anim = target;
                                finished = true;
                            } else {
                                wb.sidebar_anim += diff * ANIM_EASE;
                            }
                            // Keep traffic lights in sync with sidebar visibility.
                            // Never hide traffic lights in fullscreen — macOS needs
                            // them visible so the user can exit fullscreen mode.
                            if wb.sidebar_mode == SidebarMode::Overlay
                                && !is_window_fullscreen()
                            {
                                #[cfg(target_os = "macos")]
                                set_traffic_lights_alpha(wb.sidebar_anim);
                            }
                            cx.notify();
                        });
                        finished
                    })
                    .unwrap_or(true);

                if done {
                    break;
                }
            }
        });
        self._sidebar_anim_task = Some(task);
    }

    // ------------------------------------------------------------------
    // URL input
    // ------------------------------------------------------------------

    fn on_url_input_event(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { .. } = event {
            let text = self.url_input.read(cx).value().to_string();
            if !text.is_empty() {
                self.pending_nav = Some(text);
                cx.notify();
            }
        }
    }

    fn process_pending_nav(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // URL bar enter — navigate the current tab if it's a WebView, else open new.
        if let Some(raw_text) = self.pending_nav.take() {
            let text = raw_text.trim().to_string();
            log::info!("[nav] URL bar submit: {:?}", text);
            // Check file paths first — they contain dots/slashes which looks_like_url matches.
            if !text.starts_with("http://") && !text.starts_with("https://") && std::path::Path::new(&text).exists() {
                self.navigate_or_open(&text, window, cx);
            } else if text.starts_with("http://") || text.starts_with("https://") || looks_like_url(&text) {
                let url = if text.starts_with("http://") || text.starts_with("https://") {
                    text.clone()
                } else {
                    format!("https://{text}")
                };
                // Try navigating the active tab in-place.
                let navigated = self.active_tab_id.map(|id| {
                    if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                        if let Some(nav) = &tab.nav {
                            nav.load_url(&url, cx);
                            true
                        } else { false }
                    } else { false }
                }).unwrap_or(false);
                if !navigated {
                    self.open_webview(url, window, cx);
                } else {
                    // Update the TabKind url so switch_context reads the right value.
                    if let Some(id) = self.active_tab_id {
                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                            tab.kind = TabKind::WebView { url: url.clone() };
                        }
                    }
                    cx.notify();
                }
            } else {
                // Search query — navigate in the active WebView tab if possible.
                let encoded = url_encode_query(&text);
                let search_engine = cx
                    .try_global::<crate::settings::SettingsGlobal>()
                    .map(|g| g.settings.search_engine)
                    .unwrap_or_default();
                let url = search_engine.search_url(&encoded);
                let navigated = self.active_tab_id.map(|id| {
                    if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                        if let Some(nav) = &tab.nav {
                            nav.load_url(&url, cx);
                            true
                        } else { false }
                    } else { false }
                }).unwrap_or(false);
                if !navigated {
                    self.open_webview(url, window, cx);
                } else {
                    if let Some(id) = self.active_tab_id {
                        if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                            tab.kind = TabKind::WebView { url: url.clone() };
                        }
                    }
                    cx.notify();
                }
            }
        }
        // Omnibox (Cmd+T) — always opens new tab.
        if let Some(text) = self.omnibox_pending_nav.take() {
            self.omnibox_open = false;
            cx.set_global(OmniboxOpen(false));
            self.navigate_or_open(&text, window, cx);
        }
        // Drain cmd-click / new-tab events from JS
        let new_tabs = crate::shield::drain_nav_events();
        let bg_links = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.open_links_in_background)
            .unwrap_or(true);
        for (url, focus) in new_tabs {
            if focus || !bg_links {
                self.open_webview(url, window, cx);
            } else {
                self.open_webview_background(url, window, cx);
            }
        }
        // Drain page title events from JS (epocaMeta WKScriptMessageHandler)
        let title_events = crate::shield::drain_title_events();
        if !title_events.is_empty() {
            let mut changed = false;
            for tab in &mut self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                    let wv_ptr = entity.read(cx).webview_ptr;
                    if wv_ptr == 0 { continue; }
                    for (ev_ptr, title) in &title_events {
                        if *ev_ptr == wv_ptr {
                            tab.title = title.clone();
                            changed = true;
                        }
                    }
                }
            }
            if changed { cx.notify(); }
        }
        // Drain favicon URL events (epocaFavicon WKScriptMessageHandler)
        let favicon_events = crate::shield::drain_favicon_events();
        if !favicon_events.is_empty() {
            let mut changed = false;
            for tab in &mut self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                    let wv_ptr = entity.read(cx).webview_ptr;
                    if wv_ptr == 0 { continue; }
                    for (ev_ptr, ref url) in &favicon_events {
                        if *ev_ptr == wv_ptr {
                            tab.favicon_url = Some(url.clone());
                            changed = true;
                        }
                    }
                }
            }
            if changed { cx.notify(); }
        }
        // Drain shield cosmetic-count events (epocaShield WKScriptMessageHandler)
        let shield_events = crate::shield::drain_shield_events();
        if !shield_events.is_empty() {
            let mut changed = false;
            for (ev_ptr, count) in shield_events {
                for tab in &mut self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        let wv_ptr = entity.read(cx).webview_ptr;
                        if wv_ptr == ev_ptr {
                            entity.update(cx, |wv, _| wv.blocked_count += count);
                            changed = true;
                        }
                    }
                }
            }
            if changed { cx.notify(); }
        }
        // Drain cursor hover events (link hover → pointer cursor)
        for (ev_ptr, is_pointer) in crate::shield::drain_cursor_events() {
            for tab in &mut self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                    if entity.read(cx).webview_ptr == ev_ptr {
                        entity.update(cx, |wv, ecx| {
                            if wv.cursor_pointer != is_pointer {
                                wv.cursor_pointer = is_pointer;
                                ecx.notify();
                            }
                        });
                    }
                }
            }
        }
        // Drain right-click context menu events from JS.
        // Verify the webview_ptr still belongs to a live tab before showing
        // the menu — a closed tab would leave a dangling pointer.
        let ctx_events = crate::shield::drain_context_menu_events();
        for ev in ctx_events {
            let alive = self.tabs.iter().any(|t| {
                t.entity.clone().downcast::<WebViewTab>().ok()
                    .map(|e| e.read(cx).webview_ptr == ev.webview_ptr)
                    .unwrap_or(false)
            });
            #[cfg(target_os = "macos")]
            if alive {
                self.show_link_context_menu(&ev, cx);
            }
        }
        // Drain NSMenu action callbacks
        let menu_actions = crate::shield::drain_menu_actions();
        for action in menu_actions {
            match action {
                crate::shield::MenuAction::OpenInNewTab(url) => {
                    // Trigger ripple on the source tab (same feedback as cmd-click).
                    let origin = crate::shield::take_menu_origin();
                    if origin.webview_ptr != 0 {
                        self.trigger_ripple(origin.webview_ptr, origin.x, origin.y, cx);
                    }
                    self.open_webview_background(url, window, cx);
                }
                crate::shield::MenuAction::OpenInNewWindow(url) => {
                    self.open_in_new_window(url, cx);
                }
                crate::shield::MenuAction::OpenInContext(url, context_id) => {
                    // Open directly with the specified context_id — don't use
                    // open_webview_background() which inherits from the active tab.
                    let id = self.alloc_id();
                    let title = url_to_title(&url);
                    let url_clone = url.clone();
                    let ctx = Some(context_id);
                    let entity = cx.new(|cx| WebViewTab::new(url, ctx.clone(), window, cx));
                    let nav = WebViewTab::nav_handler(entity.clone());
                    self.tabs.push(TabEntry {
                        id,
                        kind: TabKind::WebView { url: url_clone },
                        title,
                        icon: IconName::Globe,
                        entity: entity.into(),
                        pinned: false,
                        nav: Some(nav),
                        favicon_url: None,
                        context_id: ctx,
                    });
                    cx.notify();
                }
                crate::shield::MenuAction::OpenPrivate(url) => {
                    let id = self.alloc_id();
                    let title = url_to_title(&url);
                    let url_clone = url.clone();
                    let entity = cx.new(|cx| WebViewTab::new(url, None, window, cx));
                    let nav = WebViewTab::nav_handler(entity.clone());
                    self.tabs.push(TabEntry {
                        id,
                        kind: TabKind::WebView { url: url_clone },
                        title,
                        icon: IconName::Globe,
                        entity: entity.into(),
                        pinned: false,
                        nav: Some(nav),
                        favicon_url: None,
                        context_id: None,
                    });
                    cx.notify();
                }
                crate::shield::MenuAction::CopyLink(url) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(url));
                }
            }
        }

        // Drain keyboard shortcuts from NSApp local event monitor
        for action in crate::shield::drain_shortcuts() {
            use crate::shield::ShortcutAction;
            match action {
                ShortcutAction::NewTab => self.new_tab(window, cx),
                ShortcutAction::CloseActiveTab => {
                    if let Some(id) = self.active_tab_id {
                        self.close_tab(id, window, cx);
                    }
                }
                ShortcutAction::FocusUrlBar => {
                    let fh = self.url_input.focus_handle(cx);
                    window.focus(&fh);
                }
                ShortcutAction::Reload => self.reload_active_tab(false, window, cx),
                ShortcutAction::HardReload => self.reload_active_tab(true, window, cx),
                ShortcutAction::OpenSettings => self.open_settings(window, cx),
                ShortcutAction::FindInPage => {
                    self.find_open = !self.find_open;
                    if self.find_open {
                        let fh = self.find_input.focus_handle(cx);
                        window.focus(&fh);
                    } else {
                        self.close_find(window, cx);
                    }
                    cx.notify();
                }
            }
        }

        // Clean up orphaned tabs whose context was deleted.
        {
            let valid_ids: std::collections::HashSet<String> = cx
                .try_global::<crate::settings::SettingsGlobal>()
                .map(|g| g.settings.contexts.iter().map(|c| c.id.clone()).collect())
                .unwrap_or_default();
            let mut orphaned = false;
            for tab in &mut self.tabs {
                if let Some(ref cid) = tab.context_id {
                    if cid != "default" && !valid_ids.contains(cid) {
                        tab.context_id = None;
                        orphaned = true;
                    }
                }
            }
            // Also reset active_context if it references a deleted context.
            if let Some(ref ac) = self.active_context {
                if ac != "default" && !valid_ids.contains(ac) {
                    self.active_context = None;
                    orphaned = true;
                }
            }
            if orphaned { cx.notify(); }
        }

        // Drain test server commands (no-op unless feature = "test-server")
        #[cfg(feature = "test-server")]
        crate::test_server::drain_test_commands(self, window, cx);
    }

    /// Show a native NSMenu at the right-click position for a link context menu event.
    #[cfg(target_os = "macos")]
    fn show_link_context_menu(&self, ev: &crate::shield::ContextMenuEvent, cx: &App) {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
        use objc2_foundation::NSPoint;
        use crate::shield::{send_menu_action, MenuAction};

        // ── Build the EpocaMenuTarget ObjC class (one-time) ────────────────
        // Each menu item action routes through a static channel.
        static TARGET_CLASS: std::sync::OnceLock<&'static AnyClass> = std::sync::OnceLock::new();
        let cls = TARGET_CLASS.get_or_init(|| {
            if let Some(c) = AnyClass::get("EpocaMenuTarget") {
                return c;
            }
            unsafe {
                let superclass = AnyClass::get("NSObject").unwrap();
                let mut builder = ClassBuilder::new("EpocaMenuTarget", superclass).unwrap();

                unsafe extern "C" fn open_new_tab(
                    _this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject,
                ) {
                    let rep: *mut AnyObject = msg_send![sender, representedObject];
                    if rep.is_null() { return; }
                    let cstr: *const i8 = msg_send![rep, UTF8String];
                    if cstr.is_null() { return; }
                    let url = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
                    send_menu_action(MenuAction::OpenInNewTab(url));
                }
                unsafe extern "C" fn open_new_window(
                    _this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject,
                ) {
                    let rep: *mut AnyObject = msg_send![sender, representedObject];
                    if rep.is_null() { return; }
                    let cstr: *const i8 = msg_send![rep, UTF8String];
                    if cstr.is_null() { return; }
                    let url = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
                    send_menu_action(MenuAction::OpenInNewWindow(url));
                }
                unsafe extern "C" fn copy_link(
                    _this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject,
                ) {
                    let rep: *mut AnyObject = msg_send![sender, representedObject];
                    if rep.is_null() { return; }
                    let cstr: *const i8 = msg_send![rep, UTF8String];
                    if cstr.is_null() { return; }
                    let url = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
                    send_menu_action(MenuAction::CopyLink(url));
                }
                unsafe extern "C" fn open_in_context(
                    _this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject,
                ) {
                    // representedObject is "url\ncontext_id"
                    let rep: *mut AnyObject = msg_send![sender, representedObject];
                    if rep.is_null() { return; }
                    let cstr: *const i8 = msg_send![rep, UTF8String];
                    if cstr.is_null() { return; }
                    let s = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
                    if let Some((url, ctx)) = s.split_once('\n') {
                        send_menu_action(MenuAction::OpenInContext(url.to_string(), ctx.to_string()));
                    }
                }
                unsafe extern "C" fn open_private(
                    _this: *mut AnyObject, _sel: Sel, sender: *mut AnyObject,
                ) {
                    let rep: *mut AnyObject = msg_send![sender, representedObject];
                    if rep.is_null() { return; }
                    let cstr: *const i8 = msg_send![rep, UTF8String];
                    if cstr.is_null() { return; }
                    let url = std::ffi::CStr::from_ptr(cstr).to_string_lossy().to_string();
                    send_menu_action(MenuAction::OpenPrivate(url));
                }

                builder.add_method(
                    objc2::sel!(openNewTab:),
                    open_new_tab as unsafe extern "C" fn(_, _, _),
                );
                builder.add_method(
                    objc2::sel!(openNewWindow:),
                    open_new_window as unsafe extern "C" fn(_, _, _),
                );
                builder.add_method(
                    objc2::sel!(copyLink:),
                    copy_link as unsafe extern "C" fn(_, _, _),
                );
                builder.add_method(
                    objc2::sel!(openInContext:),
                    open_in_context as unsafe extern "C" fn(_, _, _),
                );
                builder.add_method(
                    objc2::sel!(openPrivate:),
                    open_private as unsafe extern "C" fn(_, _, _),
                );

                builder.register()
            }
        });

        unsafe {
            let target: *mut AnyObject = msg_send![*cls, new];
            if target.is_null() { return; }

            let ns_string = AnyClass::get("NSString").unwrap();
            let ns_menu = AnyClass::get("NSMenu").unwrap();
            let ns_menu_item = AnyClass::get("NSMenuItem").unwrap();

            // Helper: create an NSString from a Rust &str
            // Helper: create an NSString from a Rust &str.
            // Appends a NUL byte so stringWithUTF8String: sees a valid C string.
            macro_rules! nsstr {
                ($s:expr) => {{
                    let mut buf = $s.as_bytes().to_vec();
                    buf.push(0);
                    let ns: *mut AnyObject = msg_send![
                        ns_string,
                        stringWithUTF8String: buf.as_ptr() as *const i8
                    ];
                    ns
                }};
            }

            let href_ns = nsstr!(&ev.href);

            // Helper: create an NSMenuItem with title, action selector, target, and representedObject
            macro_rules! make_item {
                ($title:expr, $sel:expr, $rep:expr) => {{
                    let item: *mut AnyObject = msg_send![ns_menu_item, new];
                    let title_ns = nsstr!($title);
                    let _: () = msg_send![item, setTitle: title_ns];
                    let _: () = msg_send![item, setAction: $sel];
                    let _: () = msg_send![item, setTarget: target];
                    let _: () = msg_send![item, setRepresentedObject: $rep];
                    let _: () = msg_send![item, setEnabled: objc2::ffi::YES];
                    item
                }};
            }

            // Build NSMenu
            let menu: *mut AnyObject = msg_send![ns_menu, new];
            let _: () = msg_send![menu, setAutoenablesItems: objc2::ffi::NO];

            // "Open in New Tab"
            let item1 = make_item!("Open in New Tab", objc2::sel!(openNewTab:), href_ns);
            let _: () = msg_send![menu, addItem: item1];

            // "Open in New Window"
            let item2 = make_item!("Open in New Window", objc2::sel!(openNewWindow:), href_ns);
            let _: () = msg_send![menu, addItem: item2];

            // "Open in Context ▸" submenu (only when experimental_contexts is on)
            let experimental_contexts_on = cx
                .try_global::<crate::settings::SettingsGlobal>()
                .map(|g| g.settings.experimental_contexts)
                .unwrap_or(false);
            if experimental_contexts_on {
                let all_contexts = cx
                    .try_global::<crate::settings::SettingsGlobal>()
                    .map(|g| g.settings.contexts.clone())
                    .unwrap_or_default();
                if !all_contexts.is_empty() {
                    let submenu: *mut AnyObject = msg_send![ns_menu, new];
                    // "Private Tab" — always available so user can open link without context
                    {
                        let private_item = make_item!(
                            "Private Tab",
                            objc2::sel!(openPrivate:),
                            href_ns
                        );
                        let _: () = msg_send![submenu, addItem: private_item];
                        let sub_sep: *mut AnyObject = msg_send![ns_menu_item, separatorItem];
                        let _: () = msg_send![submenu, addItem: sub_sep];
                    }
                    for ctx in &all_contexts {
                        let rep = format!("{}\n{}", ev.href, ctx.id);
                        let rep_ns = nsstr!(&rep);
                        let ctx_item = make_item!(&ctx.name, objc2::sel!(openInContext:), rep_ns);
                        let _: () = msg_send![submenu, addItem: ctx_item];
                    }
                    let ctx_parent: *mut AnyObject = msg_send![ns_menu_item, new];
                    let ctx_parent_title = nsstr!("Open in Context");
                    let _: () = msg_send![ctx_parent, setTitle: ctx_parent_title];
                    let _: () = msg_send![ctx_parent, setSubmenu: submenu];
                    let _: () = msg_send![menu, addItem: ctx_parent];
                }
            }

            // Separator
            let sep: *mut AnyObject = msg_send![ns_menu_item, separatorItem];
            let _: () = msg_send![menu, addItem: sep];

            // "Copy Link Address"
            let item3 = make_item!("Copy Link Address", objc2::sel!(copyLink:), href_ns);
            let _: () = msg_send![menu, addItem: item3];

            // ── Find the WKWebView NSView to anchor the menu ───────────
            // Use the webview_ptr to find the correct WKWebView NSView.
            let wv_ptr = ev.webview_ptr as *mut AnyObject;

            // WKWebView is a flipped NSView (isFlipped = YES), so CSS
            // (clientX, clientY) maps directly to the view's local
            // coordinate system. JS clientX/clientY are already in CSS
            // (logical) pixels, which matches NSView points on macOS.
            let location = NSPoint { x: ev.x, y: ev.y };

            // Store click origin so the OpenInNewTab handler can trigger ripple.
            crate::shield::set_menu_origin(crate::shield::MenuClickOrigin {
                webview_ptr: ev.webview_ptr,
                x: ev.x,
                y: ev.y,
            });

            // Pop up the menu at the computed position in the WKWebView.
            // Returns BOOL — must capture as bool, not ().
            let _: bool = msg_send![
                menu,
                popUpMenuPositioningItem: std::ptr::null::<AnyObject>()
                atLocation: location
                inView: wv_ptr
            ];
        }
    }

    /// Evaluate the ripple animation JS on the WebView identified by `webview_ptr`.
    /// Same visual as the cmd-click ripple in RIPPLE_SCRIPT.
    /// Evaluate the ripple animation JS on the WebView identified by `webview_ptr`.
    /// Same visual as the cmd-click ripple in RIPPLE_SCRIPT.
    fn trigger_ripple(&self, webview_ptr: usize, x: f64, y: f64, cx: &App) {
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                if entity.read(cx).webview_ptr == webview_ptr {
                    let js = format!(
                        r#"(function(){{var r=document.createElement('div');
var x={x},y={y},sz=48;
r.style.cssText='position:fixed;pointer-events:none;border-radius:50%;'
+'border:2px solid rgba(160,160,160,0.7);background:rgba(160,160,160,0.08);'
+'left:'+(x-sz/2)+'px;top:'+(y-sz/2)+'px;'
+'width:'+sz+'px;height:'+sz+'px;'
+'transform:scale(0.1);opacity:1;z-index:2147483647;'
+'transition:transform 400ms cubic-bezier(0.25,0.46,0.45,0.94),opacity 380ms ease-out;';
document.body.appendChild(r);r.getBoundingClientRect();
r.style.transform='scale(4.5)';r.style.opacity='0';
setTimeout(function(){{r.remove();}},420);}})()"#,
                        x = x, y = y
                    );
                    entity.read(cx).evaluate_script(&js, cx);
                    break;
                }
            }
        }
    }

    fn close_omnibox(&mut self, cx: &mut Context<Self>) {
        self.omnibox_open = false;
        cx.set_global(OmniboxOpen(false));
        cx.notify();
    }

    fn on_omnibox_input_event(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { .. } = event {
            let text = self.omnibox_input.read(cx).value().to_string();
            if !text.is_empty() {
                self.omnibox_pending_nav = Some(text);
                cx.notify();
            }
        }
    }

    fn navigate_or_open(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text.starts_with("http://") || text.starts_with("https://") {
            self.open_webview(text.to_string(), window, cx);
            return;
        }
        let text_trimmed = text.trim();
        let path = std::path::Path::new(text_trimmed);
        log::info!("[nav] navigate_or_open: {:?} exists={}", text_trimmed, path.exists());
        if path.exists() {
            log::info!("[nav] extension: {:?}", path.extension());
            match path.extension().and_then(|e| e.to_str()) {
                Some("toml") | Some("zml") => {
                    self.open_declarative_app(text.to_string(), window, cx);
                }
                Some("polkavm") => {
                    let name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("app")
                        .to_string();
                    self.open_sandbox_app(name, path, window, cx);
                }
                Some("prod") => {
                    match ProdBundle::from_file(path) {
                        Ok(bundle) => {
                            if bundle.manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer) {
                                self.open_framebuffer_app(bundle, window, cx);
                            } else {
                                // Non-framebuffer .prod — open as regular sandbox app
                                let name = bundle.manifest.app.name.clone();
                                let config = epoca_sandbox::SandboxConfig::default();
                                match epoca_sandbox::SandboxInstance::from_bytes(&bundle.program_bytes, &config) {
                                    Ok(_) => {
                                        // Re-open via sandbox path (create temp or use bundle bytes directly)
                                        log::info!("Non-framebuffer .prod bundle: {name}");
                                    }
                                    Err(e) => log::error!("Failed to load .prod sandbox: {e}"),
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to load .prod: {e}");
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if looks_like_url(text) {
            let url = format!("https://{text}");
            self.open_webview(url, window, cx);
        } else {
            let encoded = url_encode_query(text);
            let search_engine = cx
                .try_global::<crate::settings::SettingsGlobal>()
                .map(|g| g.settings.search_engine)
                .unwrap_or_default();
            let url = search_engine.search_url(&encoded);
            self.open_webview(url, window, cx);
        }
    }

    // ------------------------------------------------------------------
    // Tab management
    // ------------------------------------------------------------------

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    fn active_tab(&self) -> Option<&TabEntry> {
        self.tabs.iter().find(|t| Some(t.id) == self.active_tab_id)
    }

    fn switch_tab(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab_id = Some(tab_id);
        let tab = self.tabs.iter().find(|t| t.id == tab_id);
        let value = tab
            .map(|tab| match &tab.kind {
                TabKind::WebView { url } => url.clone(),
                TabKind::DeclarativeApp { path } => path.clone(),
                TabKind::SandboxApp { app_id } => app_id.clone(),
                TabKind::FramebufferApp { app_id } => app_id.clone(),
                TabKind::CodeEditor { path } => path.clone().unwrap_or_default(),
                TabKind::Welcome | TabKind::Settings => String::new(),
            })
            .unwrap_or_default();
        // Sync the context indicator to reflect this tab's context.
        // Only update from WebView tabs — non-WebView tabs (Settings, Welcome, etc.)
        // have no meaningful context and shouldn't reset the indicator.
        if let Some(t) = tab {
            if matches!(t.kind, TabKind::WebView { .. }) {
                self.active_context = t.context_id.clone();
            }
        }
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(value, window, inner_cx));
        cx.notify();
    }

    #[cfg(feature = "test-server")]
    pub(crate) fn close_tab_by_id(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab(tab_id, window, cx);
    }
    #[cfg(feature = "test-server")]
    pub(crate) fn switch_tab_by_id(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_tab(tab_id, window, cx);
    }

    fn close_tab(&mut self, tab_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let idx = self.tabs.iter().position(|t| t.id == tab_id);
        self.tabs.retain(|t| t.id != tab_id);
        if self.active_tab_id == Some(tab_id) {
            let new_idx = idx
                .map(|i| i.min(self.tabs.len().saturating_sub(1)))
                .unwrap_or(0);
            self.active_tab_id = self.tabs.get(new_idx).map(|t| t.id);
            if let Some(id) = self.active_tab_id {
                self.switch_tab(id, window, cx);
                return;
            }
        }
        cx.notify();
    }

    pub fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.omnibox_input
            .update(cx, |s, cx| s.set_value("".to_string(), window, cx));
        self.omnibox_open = true;
        cx.set_global(OmniboxOpen(true));
        // Focus the input field so the user can type immediately.
        let focus_handle = self.omnibox_input.focus_handle(cx);
        window.focus(&focus_handle);
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Open methods
    // ------------------------------------------------------------------

    pub fn open_webview(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let title = url_to_title(&url);
        let url_clone = url.clone();
        let context_id = self.resolve_context_id(cx);
        let entity = cx.new(|cx| WebViewTab::new(url, context_id.clone(), window, cx));
        let nav = WebViewTab::nav_handler(entity.clone());
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::WebView { url: url_clone.clone() },
            title,
            icon: IconName::Globe,
            entity: entity.into(),
            pinned: false,
            nav: Some(nav),
            favicon_url: None,
            context_id,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(url_clone, window, inner_cx));
        cx.notify();
    }

    /// Open a URL in a brand-new OS window.
    pub fn open_in_new_window(&self, url: String, cx: &mut Context<Self>) {
        use gpui_component::Root;
        cx.spawn(async move |_, cx| {
            cx.open_window(
                WindowOptions {
                    titlebar: Some(TitlebarOptions {
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(18.0), px(12.0))),
                        ..Default::default()
                    }),
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(120.0), px(120.0)),
                        size(px(1280.0), px(800.0)),
                    ))),
                    ..Default::default()
                },
                |window, cx| {
                    let workbench = cx.new(|cx| {
                        let mut wb = Workbench::new(window, cx);
                        wb.open_webview(url, window, cx);
                        wb
                    });
                    let view: AnyView = workbench.into();
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    }

    /// Open a new WebView tab in the background without switching to it.
    /// The active tab stays focused; the new tab is appended to the tab list.
    pub fn open_webview_background(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let title = url_to_title(&url);
        let url_clone = url.clone();
        // Background tabs (cmd-click) inherit context from the source (active) tab.
        let context_id = self.active_tab_context_id();
        let entity = cx.new(|cx| WebViewTab::new(url, context_id.clone(), window, cx));
        let nav = WebViewTab::nav_handler(entity.clone());
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::WebView { url: url_clone },
            title,
            icon: IconName::Globe,
            entity: entity.into(),
            pinned: false,
            nav: Some(nav),
            favicon_url: None,
            context_id,
        });
        // Do NOT change active_tab_id — stay on current tab.
        cx.notify();
    }

    /// Toggle isolated-tabs mode. Takes effect for all subsequently opened tabs.
    pub fn set_isolated_tabs(&mut self, isolated: bool, cx: &mut Context<Self>) {
        self.isolated_tabs = isolated;
        cx.notify();
    }

    /// Resolve the context_id for a new tab based on current settings.
    /// When `experimental_contexts` is on: use `active_context`.
    /// When off: `None` if `isolated_tabs` is true, otherwise `Some("default")` for shared.
    fn resolve_context_id(&self, cx: &App) -> Option<String> {
        let experimental = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.experimental_contexts)
            .unwrap_or(false);
        if experimental {
            self.active_context.clone()
        } else if self.isolated_tabs {
            None // isolated → incognito
        } else {
            Some("default".to_string()) // shared persistent store
        }
    }

    /// Get context_id of the currently active tab (for background opens inheriting context).
    fn active_tab_context_id(&self) -> Option<String> {
        self.active_tab_id.and_then(|id| {
            self.tabs.iter().find(|t| t.id == id).and_then(|t| t.context_id.clone())
        })
    }

    /// Switch the active context. If the active tab already has a URL loaded,
    /// close it and reopen the same URL in a new tab with the chosen context
    /// (WKWebView data stores can't change after creation).
    fn switch_context(
        &mut self,
        new_ctx: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_context == new_ctx {
            cx.notify();
            return;
        }
        self.active_context = new_ctx;
        // If the active tab is a WebView, reopen it in the new context.
        // Use the URL bar value (live URL) instead of TabKind url (may be stale).
        let reopen = self.active_tab_id.and_then(|id| {
            let tab = self.tabs.iter().find(|t| t.id == id)?;
            if matches!(tab.kind, TabKind::WebView { .. }) {
                let live_url = self.url_input.read(cx).value().to_string();
                if !live_url.is_empty()
                    && (live_url.starts_with("http://") || live_url.starts_with("https://"))
                {
                    Some((id, live_url))
                } else {
                    // Fall back to TabKind url
                    if let TabKind::WebView { url } = &tab.kind {
                        if !url.is_empty() { Some((id, url.clone())) } else { None }
                    } else {
                        None
                    }
                }
            } else {
                None
            }
        });
        if let Some((old_id, url)) = reopen {
            self.close_tab(old_id, window, cx);
            self.open_webview(url, window, cx);
        } else {
            cx.notify();
        }
    }

    /// Create a new context with an auto-generated name and the first unused color,
    /// then switch to it.
    fn create_new_context(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing: Vec<crate::settings::SessionContext> = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.contexts.clone())
            .unwrap_or_default();

        // Pick the first unused color from the preset palette.
        let used_colors: std::collections::HashSet<&str> =
            existing.iter().map(|c| c.color.as_str()).collect();
        let color = crate::settings::DEFAULT_CONTEXT_COLORS
            .iter()
            .find(|c| !used_colors.contains(**c))
            .unwrap_or(&crate::settings::DEFAULT_CONTEXT_COLORS[0]);

        // Generate a unique id and name like "Context 1", "Context 2", etc.
        let num = existing.len() + 1;
        let id = format!("ctx-{}", num);
        let name = format!("Context {}", num);

        let new_ctx = crate::settings::SessionContext {
            id: id.clone(),
            name,
            color: color.to_string(),
        };

        cx.update_global::<crate::settings::SettingsGlobal, _>(|g, _| {
            g.settings.contexts.push(new_ctx);
            g.save();
        });

        self.switch_context(Some(id), window, cx);
    }

    /// Toggle the shield exception for the active tab's hostname.
    /// Eye icon in URL bar calls this; globe turns red when excepted.
    fn toggle_site_shield(&mut self, cx: &mut Context<Self>) {
        let hostname = self
            .active_tab()
            .and_then(|t| match &t.kind {
                TabKind::WebView { url } => Some(hostname_from_url(url).to_string()),
                _ => None,
            });
        if let Some(host) = hostname {
            if host.is_empty() { return; }
            cx.update_global::<crate::shield::ShieldGlobal, _>(|g, _| {
                g.0.toggle_site_exception(&host);
            });
            cx.notify();
        }
    }

    /// Open the Settings tab, or switch to it if already open.
    pub fn open_settings(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::Settings).map(|t| t.id) {
            self.active_tab_id = Some(id);
            cx.notify();
            return;
        }
        let id = self.alloc_id();
        let entity = cx.new(|cx| SettingsTab::new(cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::Settings,
            title: "Settings".to_string(),
            icon: IconName::Settings,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    pub fn reload_active_tab(&mut self, hard: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_tab_id {
            if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                if let Ok(entity) = tab.entity.clone().downcast::<crate::tabs::WebViewTab>() {
                    entity.update(cx, |tab, cx| {
                        if hard { tab.hard_reload(cx); } else { tab.reload(cx); }
                    });
                }
            }
        }
    }

    pub fn open_declarative_app(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let title = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path)
            .to_string();
        let broker = self.broker.clone();
        let path_clone = path.clone();
        let entity = cx.new(|cx| DeclarativeAppTab::new(path, broker, window, cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::DeclarativeApp { path: path_clone.clone() },
            title,
            icon: IconName::File,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(path_clone, window, inner_cx));
        cx.notify();
    }

    pub fn open_sandbox_app(
        &mut self,
        app_id: String,
        polkavm_path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let title = app_id.clone();
        let broker = self.broker.clone();
        let path = polkavm_path.to_owned();
        let app_id_clone = app_id.clone();
        let entity =
            cx.new(|cx| SandboxAppTab::from_file(app_id, &path, broker, window, cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::SandboxApp { app_id: app_id_clone.clone() },
            title,
            icon: IconName::SquareTerminal,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(app_id_clone, window, inner_cx));
        cx.notify();
    }

    pub fn open_framebuffer_app(
        &mut self,
        bundle: ProdBundle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let app_id = bundle.manifest.app.id.clone();
        let title = bundle.manifest.app.name.clone();
        let broker = self.broker.clone();
        let app_id_clone = app_id.clone();
        let entity =
            cx.new(|cx| FramebufferAppTab::from_bundle(bundle, broker, window, cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::FramebufferApp { app_id: app_id_clone.clone() },
            title,
            icon: IconName::Frame,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(app_id_clone, window, inner_cx));
        cx.notify();
    }

    pub fn open_editor(
        &mut self,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let title = path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("Untitled")
            .to_string();
        let path_clone = path.clone();
        let entity = cx.new(|cx| CodeEditorTab::new(path, window, cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::CodeEditor { path: path_clone },
            title,
            icon: IconName::File,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    pub fn open_declarative_dev(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_editor(Some(path.clone()), window, cx);
        self.open_declarative_app(path, window, cx);
    }

    // ------------------------------------------------------------------
    // Navigation stubs
    // ------------------------------------------------------------------

    fn navigate_back(&mut self, _: &ClickEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(nav) = self.active_tab().and_then(|t| t.nav.as_ref()) {
            nav.navigate_back(cx);
        }
    }

    fn navigate_forward(&mut self, _: &ClickEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(nav) = self.active_tab().and_then(|t| t.nav.as_ref()) {
            nav.navigate_forward(cx);
        }
    }

    fn reload_page(&mut self, _: &ClickEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(nav) = self.active_tab().and_then(|t| t.nav.as_ref()) {
            nav.reload(cx);
        }
    }

    fn toggle_sidebar_mode(
        &mut self,
        _: &ClickEvent,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.sidebar_mode {
            SidebarMode::Pinned => {
                self.sidebar_mode = SidebarMode::Overlay;
                self.sidebar_anim = 0.0;
                self.sidebar_target = 0.0;
                self._sidebar_anim_task = None;
                // Sidebar is now hidden → hide traffic lights, UNLESS we are in
                // native fullscreen where macOS must control traffic-light visibility.
                #[cfg(target_os = "macos")]
                if !is_window_fullscreen() {
                    set_traffic_lights_alpha(0.0);
                }
            }
            SidebarMode::Overlay => {
                self.sidebar_mode = SidebarMode::Pinned;
                self.sidebar_anim = 1.0;
                self.sidebar_target = 1.0;
                self._sidebar_anim_task = None;
                // Back to pinned → lights always visible.
                #[cfg(target_os = "macos")]
                set_traffic_lights_alpha(1.0);
            }
        }
        cx.notify();
    }

    // ------------------------------------------------------------------
    // Render: sidebar
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Session save / restore
    // ------------------------------------------------------------------

    /// Build a SessionState snapshot from the current tabs.
    pub fn save_session(&self, _cx: &App) {
        use crate::session::{is_restorable, save_session, SessionState, SessionTab};

        let tabs: Vec<SessionTab> = self
            .tabs
            .iter()
            .filter(|t| is_restorable(&t.kind))
            .map(|t| SessionTab {
                kind: t.kind.clone(),
                title: t.title.clone(),
                pinned: t.pinned,
                favicon_url: t.favicon_url.clone(),
                context_id: t.context_id.clone(),
            })
            .collect();

        if tabs.is_empty() {
            return;
        }

        // Find the index of the active tab within the restorable subset.
        let active_tab_index = self
            .active_tab_id
            .and_then(|id| {
                let restorable_ids: Vec<u64> = self
                    .tabs
                    .iter()
                    .filter(|t| is_restorable(&t.kind))
                    .map(|t| t.id)
                    .collect();
                restorable_ids.iter().position(|&tid| tid == id)
            })
            .unwrap_or(0);

        let state = SessionState {
            version: 1,
            tabs,
            active_tab_index,
            next_tab_id: self.next_tab_id,
            active_context: self.active_context.clone(),
            isolated_tabs: self.isolated_tabs,
        };

        save_session(&state);
    }

    /// Restore tabs from saved session. Returns true if at least one tab was restored.
    pub fn restore_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        use crate::session::load_session;
        use crate::tabs::TabKind;

        let state = match load_session() {
            Some(s) => s,
            None => return false,
        };

        self.next_tab_id = state.next_tab_id;
        self.active_context = state.active_context;
        self.isolated_tabs = state.isolated_tabs;

        let mut restored_count = 0usize;

        for stab in &state.tabs {
            match &stab.kind {
                TabKind::WebView { url } => {
                    let id = self.alloc_id();
                    let ctx = stab.context_id.clone();
                    let entity = cx.new(|cx| {
                        WebViewTab::new(url.clone(), ctx.clone(), window, cx)
                    });
                    let nav = WebViewTab::nav_handler(entity.clone());
                    self.tabs.push(TabEntry {
                        id,
                        kind: stab.kind.clone(),
                        title: stab.title.clone(),
                        icon: IconName::Globe,
                        entity: entity.into(),
                        pinned: stab.pinned,
                        nav: Some(nav),
                        favicon_url: stab.favicon_url.clone(),
                        context_id: ctx,
                    });
                    restored_count += 1;
                }
                TabKind::Settings => {
                    self.open_settings(window, cx);
                    restored_count += 1;
                }
                TabKind::CodeEditor { path } => {
                    if let Some(p) = path {
                        if !std::path::Path::new(p).exists() {
                            log::warn!("Skipping restore of missing file: {p}");
                            continue;
                        }
                    }
                    let id = self.alloc_id();
                    let entity = cx.new(|cx| {
                        CodeEditorTab::new(path.clone(), window, cx)
                    });
                    self.tabs.push(TabEntry {
                        id,
                        kind: stab.kind.clone(),
                        title: stab.title.clone(),
                        icon: IconName::File,
                        entity: entity.into(),
                        pinned: stab.pinned,
                        nav: None,
                        favicon_url: None,
                        context_id: None,
                    });
                    restored_count += 1;
                }
                TabKind::DeclarativeApp { path } => {
                    if !std::path::Path::new(path).exists() {
                        log::warn!("Skipping restore of missing app: {path}");
                        continue;
                    }
                    let id = self.alloc_id();
                    let broker = self.broker.clone();
                    let entity = cx.new(|cx| {
                        DeclarativeAppTab::new(path.clone(), broker, window, cx)
                    });
                    self.tabs.push(TabEntry {
                        id,
                        kind: stab.kind.clone(),
                        title: stab.title.clone(),
                        icon: IconName::File,
                        entity: entity.into(),
                        pinned: stab.pinned,
                        nav: None,
                        favicon_url: None,
                        context_id: None,
                    });
                    restored_count += 1;
                }
                _ => {
                    // Welcome, SandboxApp, FramebufferApp — skip
                }
            }
        }

        if restored_count == 0 {
            return false;
        }

        // Set active tab from saved index, clamped to restored tab count.
        let idx = state.active_tab_index.min(self.tabs.len().saturating_sub(1));
        if let Some(tab) = self.tabs.get(idx) {
            let id = tab.id;
            self.switch_tab(id, window, cx);
        }

        cx.notify();
        true
    }

    // ------------------------------------------------------------------
    // Find-in-page
    // ------------------------------------------------------------------

    fn on_find_input_event(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = self.find_input.read(cx).value().to_string();
                if query.is_empty() {
                    self.clear_find_highlights(cx);
                } else {
                    self.find_in_active_tab(false, cx);
                }
            }
            InputEvent::PressEnter { .. } => {
                self.find_in_active_tab(false, cx);
            }
            _ => {}
        }
    }

    fn find_in_active_tab(&self, backwards: bool, cx: &App) {
        let query = self.find_input.read(cx).value().to_string();
        if query.is_empty() {
            return;
        }
        let Some(id) = self.active_tab_id else { return };
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
            // Escape the query for embedding in JS string literal.
            let escaped = query
                .replace('\\', "\\\\")
                .replace('\'', "\\'")
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            let js = format!(
                "window.find('{}', false, {}, true)",
                escaped,
                if backwards { "true" } else { "false" }
            );
            entity.read(cx).evaluate_script(&js, cx);
        }
    }

    fn clear_find_highlights(&self, cx: &App) {
        let Some(id) = self.active_tab_id else { return };
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
            entity
                .read(cx)
                .evaluate_script("window.getSelection().removeAllRanges()", cx);
        }
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        self.clear_find_highlights(cx);
        self.find_input
            .update(cx, |s, inner_cx| s.set_value("".to_string(), window, inner_cx));
        cx.notify();
    }

    fn render_find_bar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let find_bar_bg = rgba(0x3a3a3aff);
        let border_color = rgba(0xffffff1e);

        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .py(px(4.0))
            .bg(find_bar_bg)
            .rounded_t(px(8.0))
            .border_b_1()
            .border_color(border_color)
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.0))
                    .text_color(rgba(0xffffff66)),
            )
            .child(
                div()
                    .flex_1()
                    .child(
                        Input::new(&self.find_input)
                            .appearance(false)
                            .small()
                            .cleanable(true),
                    ),
            )
            .child(
                Button::new("find-prev")
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowUp)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.find_in_active_tab(true, cx);
                    })),
            )
            .child(
                Button::new("find-next")
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowDown)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.find_in_active_tab(false, cx);
                    })),
            )
            .child(
                Button::new("find-close")
                    .ghost()
                    .compact()
                    .icon(IconName::Close)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_find(window, cx);
                    })),
            )
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    this.close_find(window, cx);
                }
                // Shift+Enter → find previous
                if ev.keystroke.key == "enter" && ev.keystroke.modifiers.shift {
                    this.find_in_active_tab(true, cx);
                }
            }))
    }

    fn render_sidebar(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let url_bar_bg = rgba(0xffffff14);
        let item_active_bg = rgba(0xffffff1c);
        let item_hover_bg = rgba(0xffffff0f);
        let text_active: Rgba = rgba(0xffffffff);
        let text_normal: Rgba = rgba(0xffffffcc);
        let text_muted: Rgba = rgba(0xffffff66);
        let icon_active: Rgba = rgba(0xffffffcc);
        let icon_muted: Rgba = rgba(0xffffff66);
        let divider_color = rgba(0xffffff14);

        // ── Top row: traffic-light spacer + nav buttons ───────────────────
        // Pinned (panel flush at y=0): h=38, icons center y=19 = traffic light center.
        // Overlay (panel starts at SIDEBAR_TOP=4): h=28, icons center y=4+14=18 ≈
        // traffic light center (y=12 + radius 6 = 18). Nav buttons pushed right
        // in overlay mode so pin icon sits beside traffic lights, nav at far right.
        let is_overlay = self.sidebar_mode == SidebarMode::Overlay;
        let top_row = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .h(px(if is_overlay { 28.0 } else { 38.0 }))
            .flex_shrink_0()
            .child(div().w(px(68.0)).flex_shrink_0()) // traffic-light reserve
            .child(
                Button::new("sidebar-mode")
                    .ghost()
                    .compact()
                    .icon(IconName::PanelLeft)
                    .on_click(cx.listener(Self::toggle_sidebar_mode)),
            )
            .when(is_overlay, |d| d.child(div().flex_1())) // push nav right in overlay
            .child(
                Button::new("nav-back")
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowLeft)
                    .on_click(cx.listener(Self::navigate_back)),
            )
            .child(
                Button::new("nav-forward")
                    .ghost()
                    .compact()
                    .icon(IconName::ArrowRight)
                    .on_click(cx.listener(Self::navigate_forward)),
            )
            .child(
                Button::new("nav-reload")
                    .ghost()
                    .compact()
                    .icon(IconName::Redo)
                    .on_click(cx.listener(Self::reload_page)),
            );

        // ── URL bar ───────────────────────────────────────────────────────
        // The outer div owns the visual border/bg; Input is appearance=false
        // so it doesn't add a second bg/border layer. Size::Small reduces
        // horizontal padding from 12px → 8px, tightening the globe and X gaps.
        let experimental_contexts_on = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.experimental_contexts)
            .unwrap_or(false);
        let all_contexts = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.contexts.clone())
            .unwrap_or_default();
        let active_ctx = self.active_context.clone();

        // Context indicator dot — sits inside the URL bar prefix, left of the globe.
        // Colored dot = named context, EyeOff icon = private. Click opens dropdown.
        let url_prefix: AnyElement = if experimental_contexts_on {
            let dot_color = match &active_ctx {
                None => None,
                Some(id) => all_contexts
                    .iter()
                    .find(|c| c.id == *id)
                    .and_then(|c| parse_hex_color(&c.color)),
            };
            div()
                .id("ctx-dot")
                .flex()
                .items_center()
                .gap(px(4.0))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.context_picker_open = !this.context_picker_open;
                    cx.notify();
                }))
                .when_some(dot_color, |d, color| {
                    d.child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(color)
                            .flex_shrink_0(),
                    )
                })
                .when(dot_color.is_none(), |d| {
                    d.child(Icon::new(IconName::EyeOff).size(px(12.0)).text_color(rgba(0xffffff55)))
                })
                .child(Icon::new(IconName::Globe).size(px(13.0)))
                .into_any_element()
        } else {
            Icon::new(IconName::Globe).size(px(13.0)).into_any_element()
        };

        // Context dropdown — rendered below the URL bar when open
        let context_picker_open = self.context_picker_open;
        let context_dropdown = if experimental_contexts_on && context_picker_open {
            let active_id = active_ctx.clone();
            let mut rows: Vec<AnyElement> = Vec::new();

            // "Private" option
            let is_private = active_id.is_none();
            rows.push(
                div()
                    .id("ctx-private")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff14)))
                    .when(is_private, |d| d.bg(rgba(0xffffff0c)))
                    .child(Icon::new(IconName::EyeOff).size(px(12.0)).text_color(rgba(0xffffff55)))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(if is_private { rgba(0xffffffff) } else { rgba(0xffffffaa) })
                            .child("Private"),
                    )
                    .when(is_private, |d| {
                        d.child(Icon::new(IconName::Check).size(px(11.0)).text_color(rgba(0x22c55eff)))
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.context_picker_open = false;
                        this.switch_context(None, window, cx);
                    }))
                    .into_any_element(),
            );

            // Named contexts
            for ctx in &all_contexts {
                let ctx_id = ctx.id.clone();
                let ctx_name = ctx.name.clone();
                let dot_color = parse_hex_color(&ctx.color).unwrap_or(rgba(0xffffff44));
                let is_active = active_id.as_deref() == Some(&ctx.id);
                let click_id = ctx_id.clone();
                rows.push(
                    div()
                        .id(SharedString::from(format!("ctx-{}", ctx.id)))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgba(0xffffff14)))
                        .when(is_active, |d| d.bg(rgba(0xffffff0c)))
                        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(dot_color).flex_shrink_0())
                        .child(
                            div()
                                .flex_1()
                                .text_xs()
                                .text_color(if is_active { rgba(0xffffffff) } else { rgba(0xffffffaa) })
                                .child(ctx_name),
                        )
                        .when(is_active, |d| {
                            d.child(Icon::new(IconName::Check).size(px(11.0)).text_color(rgba(0x22c55eff)))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.context_picker_open = false;
                            this.switch_context(Some(click_id.clone()), window, cx);
                        }))
                        .into_any_element(),
                );
            }

            // Separator + "+ New Context" row
            rows.push(
                div()
                    .h(px(1.0))
                    .mx(px(6.0))
                    .my(px(3.0))
                    .bg(rgba(0xffffff14))
                    .into_any_element(),
            );
            rows.push(
                div()
                    .id("ctx-new")
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff14)))
                    .child(Icon::new(IconName::Plus).size(px(12.0)).text_color(rgba(0xffffff55)))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgba(0xffffffaa))
                            .child("New Context"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.context_picker_open = false;
                        this.create_new_context(window, cx);
                    }))
                    .into_any_element(),
            );

            // Position below url bar: top_row(38) + url margin(4) + url height(~32) + gap(2)
            Some(
                div()
                    .absolute()
                    .top(px(76.0))
                    .left(px(8.0))
                    .right(px(8.0))
                    .rounded(px(8.0))
                    .bg(rgba(0x1e1e1eff))
                    .border_1()
                    .border_color(rgba(0xffffff22))
                    .shadow_lg()
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .children(rows),
            )
        } else {
            None
        };

        // Backdrop is now rendered at the root level (Render::render)
        // so it covers the full window, not just the sidebar.

        let url_row = div()
            .id("url-bar")
            .mx(px(8.0))
            .mt(px(4.0))
            .mb(px(10.0))
            .rounded(px(8.0))
            .bg(url_bar_bg)
            .border_1()
            .border_color(rgba(0xffffff22))
            .on_mouse_down(MouseButton::Left, cx.listener(|_this, event: &MouseDownEvent, window, _cx| {
                if event.click_count >= 3 {
                    window.dispatch_action(Box::new(gpui_component::input::SelectAll), _cx);
                }
            }))
            .child(
                Input::new(&self.url_input)
                    .appearance(false)
                    .small()
                    .prefix(url_prefix)
                    .cleanable(true),
            );

        // ── Context color lookup ─────────────────────────────────────────
        let contexts = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.contexts.clone())
            .unwrap_or_default();
        let context_color_for = move |ctx_id: &Option<String>| -> Option<Rgba> {
            let id = ctx_id.as_deref()?;
            if id == "default" { return None; } // non-experimental shared store has no dot
            let ctx = contexts.iter().find(|c| c.id == id)?;
            parse_hex_color(&ctx.color)
        };

        // ── Helper: build one tab row ─────────────────────────────────────
        // Returns AnyElement so we can collect pinned and regular tabs
        // into Vecs without naming the concrete type.
        let make_tab_row = |tab_id: u64,
                            icon: IconName,
                            favicon_url: Option<String>,
                            title: SharedString,
                            is_active: bool,
                            _pinned: bool,
                            context_color: Option<Rgba>,
                            cx: &mut Context<Self>| {
            let close_icon = IconName::Close;
            let close_id = SharedString::from(format!("close-{tab_id}"));
            let close_btn = Button::new(close_id)
                .ghost()
                .compact()
                .icon(close_icon)
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    this.close_tab(tab_id, window, cx);
                }));

            let icon_color = if is_active { icon_active } else { icon_muted };
            // favicon_url is tracked but display requires fetching bytes via
            // reqwest + gpui::Image::from_bytes (backlogged). Show the fallback
            // icon for now.
            let _ = favicon_url;
            div()
                .id(ElementId::Integer(tab_id))
                .flex()
                .items_center()
                .gap(px(6.0))
                .pl(px(10.0))
                .pr(px(2.0))
                .h(px(28.0))
                .w_full()
                .rounded(px(5.0))
                .cursor_pointer()
                .when(is_active, |d| d.bg(item_active_bg))
                .when(!is_active, |d| d.hover(|d| d.bg(item_hover_bg)))
                // Context dot — colored circle left of icon when tab has a context
                .when_some(context_color, |d, color| {
                    d.child(div().w(px(5.0)).h(px(5.0)).rounded_full().bg(color).flex_shrink_0())
                })
                .child(
                    Icon::new(icon).size(px(13.0)).text_color(icon_color),
                )
                .child(
                    div()
                        .flex_1()
                        .overflow_x_hidden()
                        .text_sm()
                        .text_color(if is_active { text_active } else { text_normal })
                        .truncate()
                        .child(title),
                )
                .child(close_btn)
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    this.switch_tab(tab_id, window, cx);
                }))
                .into_any_element()
        };

        // ── Pinned tabs ───────────────────────────────────────────────────
        let pinned_items: Vec<AnyElement> = self
            .tabs
            .iter()
            .filter(|t| t.pinned)
            .map(|t| {
                let cc = context_color_for(&t.context_id);
                make_tab_row(
                    t.id,
                    t.icon.clone(),
                    t.favicon_url.clone(),
                    SharedString::from(t.title.clone()),
                    Some(t.id) == self.active_tab_id,
                    true,
                    cc,
                    cx,
                )
            })
            .collect();

        // ── Regular (non-pinned) tabs ─────────────────────────────────────
        let regular_items: Vec<AnyElement> = self
            .tabs
            .iter()
            .filter(|t| !t.pinned)
            .map(|t| {
                let cc = context_color_for(&t.context_id);
                make_tab_row(
                    t.id,
                    t.icon.clone(),
                    t.favicon_url.clone(),
                    SharedString::from(t.title.clone()),
                    Some(t.id) == self.active_tab_id,
                    false,
                    cc,
                    cx,
                )
            })
            .collect();

        // ── New-tab button ────────────────────────────────────────────────
        let new_tab_btn = div()
            .id("new-tab-btn")
            .flex()
            .items_center()
            .gap(px(6.0))
            .pl(px(10.0))
            .h(px(28.0))
            .w_full()
            .rounded(px(5.0))
            .cursor_pointer()
            .text_color(text_muted)
            .hover(|d| d.text_color(gpui::white()))
            .child(Icon::new(IconName::Plus).size(px(13.0)))
            .child(div().text_sm().child("New Tab"))
            .on_click(cx.listener(|this, _ev, window, cx| {
                this.new_tab(window, cx);
            }));

        // ── Bottom toolbar ────────────────────────────────────────────────
        let bottom_bar = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .py(px(6.0))
            .flex_shrink_0()
            .border_t_1()
            .border_color(divider_color)
            .child(
                Button::new("settings")
                    .ghost()
                    .compact()
                    .icon(IconName::Settings)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_settings(window, cx);
                    })),
            );

        // ── Assemble ──────────────────────────────────────────────────────
        let tabs_area = div()
            .flex_1()
            .overflow_y_hidden()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(4.0))
            .pt(px(4.0))
            // Pinned section
            .when(!pinned_items.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(6.0))
                        .py(px(3.0))
                        .text_xs()
                        .text_color(text_muted)
                        .child("PINNED"),
                )
                .children(pinned_items)
                .child(div().h(px(8.0))) // spacer
            })
            // New-tab button (sits above regular tabs)
            .child(new_tab_btn)
            // Divider before regular tabs (only if there are any)
            .when(!regular_items.is_empty(), |d| {
                d.child(div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(divider_color))
                    .children(regular_items)
            });

        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_W))
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .text_color(gpui::white())
            .child(top_row)
            .child(url_row)
            .child(tabs_area)
            .child(bottom_bar)
            // Context picker dropdown — painted last so it sits on top of tabs
            .children(context_dropdown)
    }

    // ------------------------------------------------------------------
    // Render: omnibox overlay
    // ------------------------------------------------------------------

    fn render_omnibox(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.omnibox_input.read(cx).value().to_string().to_lowercase();
        let active_id = self.active_tab_id;

        let tab_rows: Vec<AnyElement> = self
            .tabs
            .iter()
            .filter(|t| {
                query.is_empty()
                    || t.title.to_lowercase().contains(&query)
                    || matches!(&t.kind, TabKind::WebView { url } if url.to_lowercase().contains(&query))
            })
            .map(|t| {
                let tab_id = t.id;
                let is_active = Some(t.id) == active_id;
                let icon = t.icon.clone();
                let title = SharedString::from(t.title.clone());
                div()
                    .id(ElementId::Integer(tab_id + 1_000_000))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(16.0))
                    .h(px(40.0))
                    .cursor_pointer()
                    .when(is_active, |d| d.bg(rgba(0xffffff14)))
                    .hover(|d| d.bg(rgba(0xffffff0d)))
                    .child(Icon::new(icon).size(px(14.0)).text_color(rgba(0xffffffaa)))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(rgba(0xffffffdd))
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgba(0xffffff44))
                            .child("Switch to Tab"),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_omnibox(cx);
                        this.switch_tab(tab_id, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let has_tabs = !tab_rows.is_empty();

        // Backdrop — click outside the panel to dismiss the omnibox.
        div()
            .id("omnibox-backdrop")
            .absolute()
            .top_0()
            .left_0()
            .bottom_0()
            .w_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000055))
            .on_click(cx.listener(|this, _, _, cx| {
                this.close_omnibox(cx);
            }))
            .child(
                // Panel absorbs clicks so the backdrop handler does not fire.
                div()
                    .id("omnibox-panel")
                    .w(px(520.0))
                    .rounded(px(12.0))
                    .bg(rgb(0x2e2e2e))
                    .border_1()
                    .border_color(rgba(0xffffff22))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
                    .child(
                        // Search input row
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(16.0))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(rgba(0xffffff14))
                            .child(
                                Icon::new(IconName::Search)
                                    .size(px(16.0))
                                    .text_color(rgba(0xffffff55)),
                            )
                            .child(div().flex_1().child(Input::new(&self.omnibox_input))),
                    )
                    .when(has_tabs, |d| d.children(tab_rows))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(16.0))
                            .h(px(38.0))
                            .text_xs()
                            .text_color(rgba(0xffffff55))
                            .when(has_tabs, |d| {
                                d.border_t_1().border_color(rgba(0xffffff14))
                            })
                            .child({
                                let engine_name = cx
                                    .try_global::<crate::settings::SettingsGlobal>()
                                    .map(|g| g.settings.search_engine.display_name())
                                    .unwrap_or("DuckDuckGo");
                                format!("↵  open URL or search {engine_name}")
                            }),
                    ),
            )
    }

    // ------------------------------------------------------------------
    // Render: content area
    // ------------------------------------------------------------------

    fn render_content(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render ALL tab entities, not just the active one.
        //
        // If we only render the active tab, GPUI removes inactive tabs' WKWebView NSViews
        // from the window hierarchy when they go off-screen. Re-adding them can trigger
        // a reload (wry re-initialises the view). By keeping every tab in the render tree
        // at all times — active ones at full size, inactive ones at 0×0 — the native
        // WKWebView frames are never removed, so page state (scroll, JS, form data) is
        // fully preserved across tab switches.
        let active_id = self.active_tab_id;

        if self.tabs.is_empty() {
            return div()
                .flex_1()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Open a tab to get started");
        }

        div()
            .relative()
            .flex_1()
            .size_full()
            .overflow_hidden()
            .children(self.tabs.iter().map(|t| {
                let is_active = Some(t.id) == active_id;
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    // Active tab fills the container; inactive tab collapses to 0×0.
                    // A zero-size frame keeps the NSView in the hierarchy (state preserved)
                    // without the view being visible or interactive.
                    .when(is_active, |d| d.w_full().h_full())
                    .when(!is_active, |d| d.w(px(0.0)).h(px(0.0)).overflow_hidden())
                    .child(t.entity.clone())
            }))
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_pending_nav(window, cx);

        let chrome_bg = rgb(0x2b2b2b);
        // Chrome border thickness around the rounded content viewport.
        // Arc uses ~10 px on top/right/bottom; left is provided by the sidebar.
        const CHROME: f32 = 10.0;
        const RADIUS: f32 = 10.0;

        match self.sidebar_mode {
            // ---- Pinned: sidebar in flex flow, content to the right ----
            SidebarMode::Pinned => {
                // In pinned mode the overlay inset is zero.
                cx.set_global(OverlayLeftInset(0.0));

                let find_bar_pinned = if self.find_open {
                    Some(self.render_find_bar(window, cx).into_any_element())
                } else {
                    None
                };

                let content = div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .size_full()
                    .pt(px(CHROME))
                    .pr(px(CHROME))
                    .pb(px(CHROME))
                    // Find bar sits in the chrome zone, ABOVE the rounded content
                    // container. This keeps it on the GPUI Metal layer (visible)
                    // rather than behind the WKWebView native NSView.
                    .children(find_bar_pinned)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .rounded(px(RADIUS))
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .child(self.render_content(window, cx)),
                    );

                let omnibox = if self.omnibox_open {
                    Some(self.render_omnibox(window, cx).into_any_element())
                } else {
                    None
                };

                // Backdrop to dismiss context dropdown when clicking outside
                let ctx_backdrop = if self.context_picker_open {
                    Some(
                        div()
                            .id("ctx-backdrop")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_picker_open = false;
                                cx.notify();
                            })),
                    )
                } else {
                    None
                };

                div()
                    .relative()
                    .size_full()
                    .bg(chrome_bg)
                    .flex()
                    .flex_row()
                    .child(self.render_sidebar(window, cx))
                    .child(content)
                    .children(ctx_backdrop)
                    .children(omnibox)
                    .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
                    .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
                        if let Some(id) = this.active_tab_id {
                            this.close_tab(id, window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &FocusUrlBar, window, _cx| {
                        let focus_handle = this.url_input.focus_handle(_cx);
                        window.focus(&focus_handle);
                    }))
                    .on_action(cx.listener(|this, _: &Reload, window, cx| this.reload_active_tab(false, window, cx)))
                    .on_action(cx.listener(|this, _: &HardReload, window, cx| this.reload_active_tab(true, window, cx)))
                    .on_action(cx.listener(|this, _: &ToggleSiteShield, _, cx| this.toggle_site_shield(cx)))
                    .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
                    .on_action(cx.listener(|this, _: &FindInPage, window, cx| {
                        this.find_open = !this.find_open;
                        if this.find_open {
                            let fh = this.find_input.focus_handle(cx);
                            window.focus(&fh);
                        } else {
                            this.close_find(window, cx);
                        }
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &FindPrev, _, cx| {
                        this.find_in_active_tab(true, cx);
                    }))
                    .on_action(cx.listener(|this, _: &CloseFindBar, window, cx| {
                        this.close_find(window, cx);
                    }))
            }

            // ---- Overlay: sidebar slides in as a modal over full-width content ----
            SidebarMode::Overlay => {
                let anim = self.sidebar_anim;

                // Publish the sidebar inset so WebViewTab can apply a CALayer mask
                // that clips the WKWebView away from the sidebar area. This keeps
                // the WKWebView's frame (and thus page viewport) unchanged — no
                // content reflow — while making the GPUI sidebar visible through
                // the unmasked region. See design.md §Overlay.
                let webview_inset = (SIDEBAR_W * anim - CHROME).max(0.0);
                cx.set_global(OverlayLeftInset(webview_inset));

                let find_bar_overlay = if self.find_open {
                    Some(self.render_find_bar(window, cx).into_any_element())
                } else {
                    None
                };

                // ── Content viewport ─────────────────────────────────────────────────
                // Full width with uniform chrome margins on all sides.
                let content = div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .pt(px(CHROME))
                    .pr(px(CHROME))
                    .pb(px(CHROME))
                    .pl(px(CHROME))
                    .children(find_bar_overlay)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .w_full()
                            .rounded(px(RADIUS))
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .child(self.render_content(window, cx)),
                    );

                // ── Sidebar overlay ──────────────────────────────────────────────────
                // Floats over content as a chrome modal panel. Traffic lights live
                // inside the sidebar top-row — visible only when the sidebar is visible.
                // Small top margin so the panel starts just above where the traffic
                // lights sit (y=12 in window coords), making them appear naturally inset
                // inside the panel. Bottom margin gives a visible floating effect.
                const SIDEBAR_TOP: f32 = 4.0;
                const SIDEBAR_BOTTOM: f32 = 8.0;
                // Noticeably lighter than window chrome (0x2b2b2b) so the panel reads
                // as a distinct modal surface — matching Arc's sidebar treatment.
                let sidebar_chrome = rgb(0x424242);
                let sidebar_left = -SIDEBAR_W * (1.0 - anim);
                let sidebar = if anim > 0.005 {
                    Some(
                        div()
                            .absolute()
                            .top(px(SIDEBAR_TOP))
                            .bottom(px(SIDEBAR_BOTTOM))
                            .w(px(SIDEBAR_W))
                            .left(px(sidebar_left))
                            .rounded(px(RADIUS))
                            .overflow_hidden()
                            .bg(sidebar_chrome)
                            .border_1()
                            .border_color(rgba(0xffffff1e))
                            .child(self.render_sidebar(window, cx)),
                    )
                } else {
                    None
                };

                let omnibox = if self.omnibox_open {
                    Some(self.render_omnibox(window, cx).into_any_element())
                } else {
                    None
                };

                // Backdrop to dismiss context dropdown when clicking outside
                let ctx_backdrop_overlay = if self.context_picker_open {
                    Some(
                        div()
                            .id("ctx-backdrop-overlay")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.context_picker_open = false;
                                cx.notify();
                            })),
                    )
                } else {
                    None
                };

                // In fullscreen overlay mode with sidebar hidden, show a small toolbar
                // at the top-left so the user can access the sidebar pin button next to
                // the traffic lights (which macOS manages in the fullscreen hover zone).
                let fullscreen_bar = if is_window_fullscreen() && anim < 0.005 {
                    Some(
                        div()
                            .absolute()
                            .top(px(SIDEBAR_TOP))
                            .left(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .px(px(8.0))
                            .h(px(28.0))
                            .child(div().w(px(68.0)).flex_shrink_0()) // traffic-light zone
                            .child(
                                Button::new("sidebar-mode-fs")
                                    .ghost()
                                    .compact()
                                    .icon(IconName::PanelLeft)
                                    .on_click(cx.listener(Self::toggle_sidebar_mode)),
                            )
                            .into_any_element(),
                    )
                } else {
                    None
                };

                div()
                    .relative()
                    .size_full()
                    .bg(chrome_bg)
                    .child(content)
                    .children(ctx_backdrop_overlay)
                    .children(sidebar)
                    .children(fullscreen_bar)
                    .children(omnibox)
                    .on_action(cx.listener(|this, _: &NewTab, window, cx| this.new_tab(window, cx)))
                    .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
                        if let Some(id) = this.active_tab_id {
                            this.close_tab(id, window, cx);
                        }
                    }))
                    .on_action(cx.listener(|this, _: &FocusUrlBar, window, _cx| {
                        let focus_handle = this.url_input.focus_handle(_cx);
                        window.focus(&focus_handle);
                    }))
                    .on_action(cx.listener(|this, _: &Reload, window, cx| this.reload_active_tab(false, window, cx)))
                    .on_action(cx.listener(|this, _: &HardReload, window, cx| this.reload_active_tab(true, window, cx)))
                    .on_action(cx.listener(|this, _: &ToggleSiteShield, _, cx| this.toggle_site_shield(cx)))
                    .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
                    .on_action(cx.listener(|this, _: &FindInPage, window, cx| {
                        this.find_open = !this.find_open;
                        if this.find_open {
                            let fh = this.find_input.focus_handle(cx);
                            window.focus(&fh);
                        } else {
                            this.close_find(window, cx);
                        }
                        cx.notify();
                    }))
                    .on_action(cx.listener(|this, _: &FindPrev, _, cx| {
                        this.find_in_active_tab(true, cx);
                    }))
                    .on_action(cx.listener(|this, _: &CloseFindBar, window, cx| {
                        this.close_find(window, cx);
                    }))
                    .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                        if this.sidebar_mode != SidebarMode::Overlay {
                            return;
                        }
                        let x = ev.position.x.as_f32();
                        // Sidebar right edge in window coordinates.
                        let _sidebar_edge = SIDEBAR_W * this.sidebar_anim;

                        if x < EDGE_ZONE {
                            this.trigger_sidebar_show(true, cx);
                        } else if this.sidebar_target >= 1.0 && x >= SIDEBAR_W {
                            // Use SIDEBAR_W (target), not sidebar_anim, so the hide
                            // condition doesn't race against the slide-in animation.
                            // Without this, a mouse at x=150 would trigger hide the
                            // moment the animating sidebar_edge crosses 150.
                            this.trigger_sidebar_hide(cx);
                        }
                    }))
            }
        }
    }
}


/// Returns true if the text looks like a URL rather than a search query.
/// Heuristic: no spaces and contains a dot (e.g. "github.com", "localhost:3000").
fn looks_like_url(s: &str) -> bool {
    !s.contains(' ') && (s.contains('.') || s.contains(':'))
}

/// Percent-encodes a string for use as a query-string value (RFC 3986).
/// Spaces become '+'; other reserved bytes become %XX.
fn url_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn url_to_title(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Parse a "#rrggbb" hex color string to an Rgba value.
fn parse_hex_color(hex: &str) -> Option<Rgba> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 { return None; }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(rgba(
        ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xff,
    ))
}
