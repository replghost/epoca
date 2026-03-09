use gpui::prelude::FluentBuilder;
use gpui::*;
use crate::shield::init_shield;

// ── Workbench-scoped actions ────────────────────────────────────────────────
actions!(workbench, [NewTab, CloseActiveTab, FocusUrlBar, Reload, HardReload, ToggleSiteShield, OpenSettings, OpenAppLibrary, OpenApp, FindInPage, FindPrev, CloseFindBar, ToggleReaderMode, OpenBookmarks, AddBookmark]);
use gpui_component::PixelsExt as _;
use crate::{OmniboxOpen, OverlayLeftInset};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::Sizable;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::theme::ActiveTheme;
use gpui_component::{Icon, IconName};
use gpui_component::tooltip::Tooltip;
use std::collections::HashMap;
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
    AppLibraryTab, BookmarksTab, CodeEditorTab, DeclarativeAppTab, DotLoadingEvent, DotLoadingTab,
    DotStatus, DotVerification, FramebufferAppTab, SandboxAppTab, SettingsTab, SpaTab, TabEntry,
    TabKind, WebViewTab, WelcomeEvent, WelcomeTab,
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

/// State for an in-progress .dot app loading tab, held until the user
/// approves or denies permissions.
struct PendingDotLoad {
    tab_id: u64,
    bundle: Option<ProdBundle>,
    verification: Option<DotVerification>,
    _subscription: Subscription,
}

/// Which wallet bridge initiated a connect request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalletChannel {
    Polkadot,
    Btc,
}

/// A pending wallet connection request awaiting user consent.
pub(crate) struct PendingWalletConnect {
    pub webview_ptr: usize,
    pub id: u64,
    pub origin: String,
    pub channel: WalletChannel,
}

/// A pending wallet sign request awaiting user confirmation.
pub(crate) struct PendingWalletSign {
    /// Which WebView originated this request.
    pub webview_ptr: usize,
    /// Callback ID to resolve in JS.
    pub id: u64,
    /// "signPayload" or "signRaw".
    pub method: String,
    /// The raw params JSON from the dapp.
    pub params_json: String,
    /// Human-readable summary for the dialog.
    pub display_message: String,
    /// The URL of the dapp requesting the signature.
    pub origin: String,
}

/// A pending SPA host API sign request awaiting user confirmation.
pub(crate) struct PendingSpaSign {
    /// Which SPA WebView originated this request.
    pub webview_ptr: usize,
    /// Callback ID to resolve via __epocaResolve.
    pub id: u64,
    /// The app_id for per-app key derivation.
    pub app_id: String,
    /// Raw payload string from the SPA.
    pub payload: String,
}

/// A pending chain.submit request awaiting user confirmation.
pub(crate) struct PendingChainSubmit {
    pub webview_ptr: usize,
    pub id: u64,
    pub app_id: String,
    pub chain: epoca_chain::ChainId,
    /// Raw call data hex string from the app.
    pub call_data: String,
}

/// Phase of the post-approval extrinsic construction state machine.
pub(crate) enum ChainSubmitPhase {
    /// Waiting for nonce + finalized head hash responses.
    AwaitingChainMeta {
        nonce_js_id: u64,
        head_js_id: u64,
        nonce: Option<u64>,
        head_hash: Option<[u8; 32]>,
    },
    /// Have nonce + head hash; waiting for block header to extract block number.
    AwaitingBlockHeader {
        nonce: u64,
        head_hash: [u8; 32],
        header_js_id: u64,
    },
}

/// In-flight extrinsic build (post-approval, pre-submission).
/// Kept separate from `PendingChainSubmit` so the approval dialog clears
/// immediately when the user clicks Approve.
pub(crate) struct ChainSubmitBuild {
    pub webview_ptr: usize,
    pub js_id: u64,
    pub chain: epoca_chain::ChainId,
    /// Raw call data hex string from the app.
    pub call_data: String,
    pub phase: ChainSubmitPhase,
}

// Temporary storage for in-flight runtime version (while we wait for genesis hash).
static PENDING_SPEC_VERSION: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
static PENDING_TX_VERSION: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

/// A pending data.connect request awaiting user confirmation.
pub(crate) struct PendingDataConnect {
    pub webview_ptr: usize,
    pub id: u64,
    pub app_id: String,
    pub peer_address: String,
    pub conn_id: u64,
}

/// A pending Polkadot app host-api sign request awaiting user confirmation.
pub(crate) struct PendingHostApiSign {
    /// Which SPA WebView originated this request.
    pub webview_ptr: usize,
    /// SCALE request_id to include in the response.
    pub request_id: String,
    /// Original request tag (TAG_SIGN_PAYLOAD_REQ or TAG_SIGN_RAW_REQ).
    pub request_tag: u8,
    /// The app_id of the SPA.
    pub app_id: String,
    /// The raw payload bytes to sign.
    pub payload: Vec<u8>,
}

/// A pending WSS permission request awaiting user confirmation.
struct PendingWssPermission {
    app_id: String,
    url: String,
    webview_ptr: usize,
    call_id: u64,
}

struct PendingHttpPermission {
    app_id: String,
    origin: String,
    webview_ptr: usize,
}

/// A pending BTC wallet sign request awaiting user confirmation.
pub(crate) struct PendingBtcWalletSign {
    pub webview_ptr: usize,
    pub id: u64,
    pub message: String,
    pub origin: String,
}

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

    // Loading glow animation
    /// Phase of the pulsing glow (0.0..2π), driven by a timer task.
    loading_glow_phase: f32,
    /// Overall intensity multiplier (1.0 while loading, fades to 0.0 after).
    loading_glow_intensity: f32,
    /// Running animation task — dropped to cancel when loading finishes.
    _loading_glow_task: Option<Task<()>>,

    // Tabs
    pub(crate) tabs: Vec<TabEntry>,
    pub(crate) active_tab_id: Option<u64>,
    next_tab_id: u64,

    // URL bar
    pub(crate) url_input: Entity<InputState>,
    _url_subscription: Subscription,
    pending_nav: Option<String>,
    url_bar_clicked: bool,

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

    /// Periodic poll — ensures host-api channel events are drained even when
    /// no user interaction triggers a re-render.
    _event_poll_task: Option<Task<()>>,

    // Find-in-page
    pub(crate) find_open: bool,
    pub(crate) find_input: Entity<InputState>,
    _find_subscription: Subscription,

    // Onboarding
    _welcome_subscription: Option<Subscription>,

    // History
    _history_cleanup: Option<Task<()>>,
    omnibox_history_results: Vec<crate::history::HistoryEntry>,

    // Wallet
    pending_wallet_sign: Option<PendingWalletSign>,
    pending_wallet_connect: Option<PendingWalletConnect>,
    pending_btc_wallet_sign: Option<PendingBtcWalletSign>,
    pending_spa_sign: Option<PendingSpaSign>,
    pending_chain_submit: Option<PendingChainSubmit>,
    /// Active extrinsic build in progress (post-approval, pre-submission).
    chain_submit_building: Option<ChainSubmitBuild>,
    pending_data_connect: Option<PendingDataConnect>,
    pending_hostapi_sign: Option<PendingHostApiSign>,
    pending_wss_permission: Option<PendingWssPermission>,
    pending_http_permission: Option<PendingHttpPermission>,
    /// Active dot-loading tab state — holds bundle waiting for user approval.
    pending_dot_load: Option<PendingDotLoad>,
    /// Monotonically increasing counter for dot-load requests; detached DOTNS
    /// tasks check this to avoid writing stale state after a second navigation.
    dot_load_generation: u64,
    /// Apps the user has already approved (app name → CID). Skips re-approval
    /// on subsequent loads of the same version.
    approved_dot_apps: std::collections::HashMap<String, String>,
    /// Bundle + verification approved by user, waiting for render to open SPA.
    pending_dot_approve: Option<(ProdBundle, Option<DotVerification>)>,
    /// Tab ID to close after user denied permissions.
    pending_dot_deny_tab: Option<u64>,
    /// Active DOTNS validation — shown in URL bar while resolving on-chain.
    pending_dotns_validation: Option<DotVerification>,
    /// Pending hostapi JSON-RPC queries: js_id → (webview_ptr, scale_request_id).
    /// When chain_api::drain_responses returns a result with a matching js_id,
    /// we encode it as a SCALE TAG_JSONRPC_SEND_RESP and send it back via hostapi.
    pending_hostapi_rpc: std::collections::HashMap<u64, (usize, String)>,
    connected_sites: std::collections::HashSet<String>,
    wallet_popover_open: bool,
    /// When true, the sidebar shows the page-info panel instead of the tab list.
    page_info_open: bool,

    // Tab drag-to-reorder
    /// Tab being dragged (by tab id), start y, current y in sidebar coordinates.
    dragging_tab: Option<DragState>,
}

/// State for an in-progress tab drag-to-reorder operation.
struct DragState {
    tab_id: u64,
    /// Whether we've moved enough to consider it a real drag (not just a click).
    active: bool,
    start_y: f32,
    current_y: f32,
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

        // Start browsing history subsystem.
        crate::history::init_history(cx);

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

        let permissions_path = {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_default();
            #[cfg(target_os = "macos")]
            let dir = std::path::PathBuf::from(&home).join("Library/Application Support/Epoca");
            #[cfg(not(target_os = "macos"))]
            let dir = std::path::PathBuf::from(&home).join(".epoca");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("permissions.json").to_string_lossy().to_string()
        };
        let broker = CapabilityBroker::new().with_storage(permissions_path);

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

        // Hourly history cleanup timer.
        let history_cleanup_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(3600))
                    .await;
                let should_continue = cx
                    .update(|cx| {
                        if this.upgrade().is_none() {
                            return false;
                        }
                        if let Some(hg) = cx.try_global::<crate::history::HistoryGlobal>() {
                            hg.manager.cleanup_expired();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !should_continue {
                    break;
                }
            }
        });

        // Periodic poll to drain host-api / SPA channel events.
        // Without this, messages from WKScriptMessageHandler sit in the channel
        // until a user interaction triggers a re-render.
        let event_poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                let ok = cx
                    .update(|cx| {
                        if let Some(entity) = this.upgrade() {
                            entity.update(cx, |_, cx| cx.notify());
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false);
                if !ok {
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
            loading_glow_phase: 0.0,
            loading_glow_intensity: 0.0,
            _loading_glow_task: None,
            tabs: Vec::new(),
            active_tab_id: None,
            next_tab_id: 1,
            url_input,
            _url_subscription,
            pending_nav: None,
            url_bar_clicked: false,
            omnibox_open: false,
            omnibox_input,
            _omnibox_subscription,
            omnibox_pending_nav: None,
            broker: Arc::new(Mutex::new(broker)),
            isolated_tabs: false,
            active_context: None,
            context_picker_open: false,
            _session_save_task: Some(session_save_task),
            _event_poll_task: Some(event_poll_task),
            find_open: false,
            find_input,
            _find_subscription,
            _welcome_subscription: None,
            _history_cleanup: Some(history_cleanup_task),
            omnibox_history_results: Vec::new(),
            pending_wallet_sign: None,
            pending_wallet_connect: None,
            pending_btc_wallet_sign: None,
            pending_spa_sign: None,
            pending_chain_submit: None,
            chain_submit_building: None,
            pending_data_connect: None,
            pending_hostapi_sign: None,
            pending_wss_permission: None,
            pending_http_permission: None,
            pending_dot_load: None,
            dot_load_generation: 0,
            approved_dot_apps: crate::session::load_approved_apps(),
            pending_dot_approve: None,
            pending_dot_deny_tab: None,
            pending_dotns_validation: None,
            pending_hostapi_rpc: std::collections::HashMap::new(),
            connected_sites: std::collections::HashSet::new(),
            wallet_popover_open: false,
            page_info_open: false,
            dragging_tab: None,
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
        // Keep the sidebar open while an approval dialog is pending.
        if self.has_pending_approval() {
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
        // Keep the global URL-bar-focused flag in sync so the NSApp event monitor
        // knows to intercept plain Enter keypresses.
        // Check both GPUI focus and our manually-tracked click state.
        let url_focused = self.url_input.focus_handle(cx).is_focused(window)
            || self.url_bar_clicked;
        crate::shield::set_url_bar_focused(url_focused);

        // URL bar enter — navigate the current tab if it's a WebView, else open new.
        if let Some(raw_text) = self.pending_nav.take() {
            self.url_bar_clicked = false;
            let text = raw_text.trim().to_string();
            // Internal epoca:// pages.
            if text == "epoca://onboard" || text == "epoca://welcome" {
                self.open_onboard(window, cx);
                cx.notify();
            // dot:// scheme or bare .dot TLD — resolve to local .prod bundle or DOTNS on-chain.
            } else if text.starts_with("dot://") {
                log::info!("[nav] dot:// URL detected: {text}");
                self.resolve_dot_url(&text, window, cx);
            } else if text.ends_with(".dot") && !text.contains('/') && !text.contains(' ') {
                // Bare ".dot" TLD (e.g. "hackme3.dot") — wrap in dot:// scheme.
                let dot_url = format!("dot://{text}");
                log::info!("[nav] bare .dot name detected, resolving: {dot_url}");
                self.resolve_dot_url(&dot_url, window, cx);
            // Check file paths first — they contain dots/slashes which looks_like_url matches.
            } else if !text.starts_with("http://") && !text.starts_with("https://") && std::path::Path::new(&text).exists() {
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
                    record_history_visit(&url, "", cx);
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
                    record_history_visit(&url, "", cx);
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
        let nav_events = crate::shield::drain_nav_events();
        let bg_links = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.open_links_in_background)
            .unwrap_or(true);
        for ev in nav_events {
            match ev {
                crate::shield::NavEvent::OpenUrl { url, focus } => {
                    if focus || !bg_links {
                        self.open_webview(url, window, cx);
                    } else {
                        self.open_webview_background(url, window, cx);
                    }
                }
                crate::shield::NavEvent::GoBack => {
                    if let Some(tab) = self.active_tab_id.and_then(|id| self.tabs.iter().find(|t| t.id == id)) {
                        if let Some(nav) = &tab.nav {
                            nav.navigate_back(cx);
                        }
                    }
                }
                crate::shield::NavEvent::GoForward => {
                    if let Some(tab) = self.active_tab_id.and_then(|id| self.tabs.iter().find(|t| t.id == id)) {
                        if let Some(nav) = &tab.nav {
                            nav.navigate_forward(cx);
                        }
                    }
                }
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
                            if let TabKind::WebView { ref url } = tab.kind {
                                if let Some(hg) = cx.try_global::<crate::history::HistoryGlobal>() {
                                    hg.manager.update_title(url, title);
                                }
                            }
                            changed = true;
                        }
                    }
                }
            }
            // When a page navigates, its wallet_connected flag may be stale
            // if the new hostname is not in connected_sites.
            if changed {
                for tab in &self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        let wv = entity.read(cx);
                        if wv.wallet_connected {
                            let host = hostname_from_url(wv.url()).to_string();
                            if !self.connected_sites.contains(&host) {
                                entity.update(cx, |wv, _| wv.wallet_connected = false);
                            }
                        }
                    }
                }
                cx.notify();
            }
        }
        // Drain URL change events — update tab kind + URL bar for active tab
        let url_events = crate::shield::drain_url_events();
        if !url_events.is_empty() {
            let active_id = self.active_tab_id;
            let mut active_url_changed: Option<String> = None;
            for (ev_ptr, url) in url_events {
                for tab in &mut self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        if entity.read(cx).webview_ptr == ev_ptr {
                            tab.kind = TabKind::WebView { url: url.clone() };
                            if active_id == Some(tab.id) {
                                active_url_changed = Some(url.clone());
                            }
                            record_history_visit(&url, &tab.title, cx);
                        }
                    }
                }
            }
            if let Some(url) = active_url_changed {
                let url_focused = self.url_input.focus_handle(cx).is_focused(window)
                    || self.url_bar_clicked;
                if !url_focused {
                    self.url_input
                        .update(cx, |s, inner_cx| s.set_value(url, window, inner_cx));
                }
            }
            cx.notify();
        }
        // Drain loading progress events from LOADING_PROGRESS_SCRIPT
        let loading_events = crate::shield::drain_loading_events();
        if !loading_events.is_empty() {
            for (ev_ptr, progress) in loading_events {
                for tab in &mut self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        if entity.read(cx).webview_ptr == ev_ptr {
                            tab.loading_progress = progress;
                        }
                    }
                }
            }
            // Start glow animation when active tab begins loading.
            // The animation loop handles fade-out on its own.
            let active_loading = self
                .active_tab_id
                .and_then(|id| self.tabs.iter().find(|t| t.id == id))
                .map(|t| t.loading_progress > 0.0 && t.loading_progress < 1.0)
                .unwrap_or(false);
            if active_loading && self._loading_glow_task.is_none() {
                self.start_loading_glow(cx);
            }
            cx.notify();
        }
        // Drain readerable events — whether page has article content
        let readerable_events = crate::shield::drain_readerable_events();
        if !readerable_events.is_empty() {
            for (ev_ptr, is_readerable) in readerable_events {
                for tab in &mut self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        if entity.read(cx).webview_ptr == ev_ptr {
                            tab.readerable = is_readerable;
                            tab.reader_active = false; // reset on new page
                        }
                    }
                }
            }
            cx.notify();
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
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == ev_ptr {
                        entity.update(cx, |spa, ecx| {
                            if spa.cursor_pointer != is_pointer {
                                spa.cursor_pointer = is_pointer;
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
                    });
                    cx.notify();
                }
                crate::shield::MenuAction::CopyLink(url) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(url));
                }
            }
        }

        // Drain completed WebAuthn ceremony responses → evaluate_script back to page
        for resp in crate::webauthn::drain_webauthn_responses() {
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                    if entity.read(cx).webview_ptr == resp.webview_ptr {
                        // Validate callback_id is numeric to prevent injection
                        let cb_id = resp.callback_id.trim();
                        if !cb_id.chars().all(|c| c.is_ascii_digit()) {
                            log::warn!("WebAuthn: invalid callback_id: {cb_id}");
                            break;
                        }
                        let ok = if resp.ok { "true" } else { "false" };
                        let json = escape_js_string(
                            resp.response_json.as_deref().unwrap_or(""),
                        );
                        let err = escape_js_string(
                            resp.error.as_deref().unwrap_or(""),
                        );
                        let js = format!(
                            "window.__epocaWebAuthnResolve('{cb_id}',{ok},'{json}','{err}')",
                        );
                        entity.read(cx).evaluate_script(&js, cx);
                        break;
                    }
                }
            }
        }

        // Drain SPA host API calls (window.epoca.* → epocaHost WKScriptMessageHandler)
        let spa_events = crate::spa::drain_spa_host_events();
        if !spa_events.is_empty() {
            log::info!("[sign-debug] drained {} spa_events", spa_events.len());
        }
        for (ev_ptr, id, method, params_json) in spa_events {
            log::info!("[sign-debug] spa event: method={method} id={id} from webview {ev_ptr:#x}");
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == ev_ptr {
                        let app_id = entity.read(cx).app_id().to_string();
                        let chain_str = entity.read(cx).chain().to_string();
                        let wallet_enabled = cx
                            .try_global::<crate::settings::SettingsGlobal>()
                            .map(|g| g.settings.experimental_wallet)
                            .unwrap_or(false);

                        let perms = crate::js_bridge::BridgePermissions {
                            wallet_enabled,
                            chain: entity.read(cx).has_permission_chain(),
                            statements: entity.read(cx).has_permission_statements(),
                            data: entity.read(cx).has_permission_data(),
                            media: entity.read(cx).media_permissions(),
                        };

                        let (wallet_address, root_pubkey): (Result<String, String>, Option<[u8; 32]>) =
                            if wallet_enabled && cx.has_global::<crate::wallet::WalletGlobal>() {
                                let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
                                // Auto-unlock if locked (triggers Touch ID on macOS)
                                if matches!(wg.manager.state(), epoca_wallet::WalletState::Locked) {
                                    let _ = wg.manager.unlock();
                                }
                                let addr = wg.manager.app_address(&app_id).map_err(|e| e.to_string());
                                let rpk = wg.manager.root_public_key();
                                (addr, rpk)
                            } else {
                                (Err("no wallet configured".into()), None)
                            };

                        // Ensure the wallet root pubkey has statement store allowance (once per process).
                        if wallet_address.is_ok() {
                            if let Some(root_pk) = root_pubkey {
                                static WALLET_ALLOWANCE_DONE: std::sync::Once = std::sync::Once::new();
                                WALLET_ALLOWANCE_DONE.call_once(move || {
                                    std::thread::spawn(move || {
                                        match epoca_chain::statement_store::ensure_allowance_for_pubkey(&root_pk) {
                                            Ok(()) => log::info!("[wallet] allowance confirmed for root pubkey"),
                                            Err(e) => log::warn!("[wallet] ensure_allowance for root pubkey failed: {e}"),
                                        }
                                    });
                                });
                            }
                        }

                        let author: String = match &wallet_address {
                            Ok(addr) => addr.clone(),
                            Err(_) => "unknown".into(),
                        };

                        let req = match crate::js_bridge::parse_request(&method, &params_json) {
                            Ok(r) => r,
                            Err(unknown) => {
                                let msg = format!("method '{unknown}' not yet implemented")
                                    .replace('\'', "\\'");
                                let js = format!(
                                    "window.__epocaResolve({id}, '{msg}', null)"
                                );
                                entity.read(cx).evaluate_script(&js, cx);
                                break;
                            }
                        };

                        let result = crate::js_bridge::dispatch(
                            &req,
                            &app_id,
                            id,
                            ev_ptr,
                            &chain_str,
                            &perms,
                            wallet_address,
                            &author,
                            self.pending_spa_sign.is_some(),
                            self.pending_chain_submit.is_some(),
                            self.pending_data_connect.is_some(),
                        );

                        match result {
                            crate::js_bridge::BridgeResult::Js(js) => {
                                entity.read(cx).evaluate_script(&js, cx);
                            }
                            crate::js_bridge::BridgeResult::Async(action) => match action {
                                crate::js_bridge::BridgeAsyncAction::Sign { payload } => {
                                    self.pending_spa_sign = Some(PendingSpaSign {
                                        webview_ptr: ev_ptr,
                                        id,
                                        app_id: app_id.clone(),
                                        payload,
                                    });
                                    self.trigger_sidebar_show(false, cx);
                                    cx.notify();
                                }
                                crate::js_bridge::BridgeAsyncAction::ChainQuery { .. } => {
                                    // Response arrives async via chain_api::drain_responses
                                }
                                crate::js_bridge::BridgeAsyncAction::ChainSubmit {
                                    call_data,
                                    chain,
                                } => {
                                    let chain_id = crate::chain_api::parse_chain_id(&chain)
                                        .unwrap_or(epoca_chain::ChainId::PaseoAssetHub);
                                    self.pending_chain_submit = Some(PendingChainSubmit {
                                        webview_ptr: ev_ptr,
                                        id,
                                        app_id: app_id.clone(),
                                        chain: chain_id,
                                        call_data,
                                    });
                                    self.trigger_sidebar_show(false, cx);
                                    cx.notify();
                                }
                                crate::js_bridge::BridgeAsyncAction::DataConnect {
                                    peer_address,
                                    conn_id,
                                } => {
                                    self.pending_data_connect = Some(PendingDataConnect {
                                        webview_ptr: ev_ptr,
                                        id,
                                        app_id: app_id.clone(),
                                        peer_address,
                                        conn_id,
                                    });
                                    self.trigger_sidebar_show(false, cx);
                                    cx.notify();
                                }
                                crate::js_bridge::BridgeAsyncAction::MediaGetUserMedia {
                                    call_id,
                                    webview_ptr: wv_ptr,
                                    audio,
                                    video,
                                    audio_track_id,
                                    video_track_id,
                                } => {
                                    // Step 1: resolve the JS promise with the allocated track IDs.
                                    let result = serde_json::json!({
                                        "audioTrackId": audio_track_id,
                                        "videoTrackId": video_track_id,
                                    });
                                    let resolve_js = format!(
                                        "window.__epocaResolve({call_id}, null, {})",
                                        serde_json::to_string(&result).unwrap_or_else(|_| "{}".into())
                                    );
                                    entity.read(cx).evaluate_script(&resolve_js, cx);
                                    // Step 2: evaluate the getUserMedia JS to open camera/mic.
                                    let gum_js = crate::media_api::get_user_media_js(
                                        audio_track_id,
                                        video_track_id,
                                        audio,
                                        video,
                                    );
                                    entity.read(cx).evaluate_script(&gum_js, cx);
                                    let _ = wv_ptr; // already used entity above
                                }
                                crate::js_bridge::BridgeAsyncAction::MediaAttachTrack {
                                    call_id,
                                    webview_ptr: wv_ptr,
                                    track_id,
                                    element_id,
                                } => {
                                    // Evaluate the attach JS, then resolve the promise.
                                    let attach_js = crate::media_api::attach_track_js(track_id, &element_id);
                                    entity.read(cx).evaluate_script(&attach_js, cx);
                                    let resolve_js = format!(
                                        "window.__epocaResolve({call_id}, null, true)"
                                    );
                                    entity.read(cx).evaluate_script(&resolve_js, cx);
                                    let _ = wv_ptr;
                                }
                                crate::js_bridge::BridgeAsyncAction::MediaConnect {
                                    call_id,
                                    webview_ptr: _wv_ptr,
                                    session_id,
                                    ref peer_address,
                                    ref track_ids,
                                    ref author,
                                } => {
                                    // Resolve the promise with the session ID.
                                    let resolve_js = format!(
                                        "window.__epocaResolve({call_id}, null, {session_id})"
                                    );
                                    entity.read(cx).evaluate_script(&resolve_js, cx);
                                    // Set up RTCPeerConnection via evaluateScript.
                                    let offerer = crate::media_api::is_offerer(author, peer_address);
                                    let setup_js = crate::media_api::setup_session_js(
                                        session_id, track_ids, offerer,
                                    );
                                    entity.read(cx).evaluate_script(&setup_js, cx);
                                }
                                crate::js_bridge::BridgeAsyncAction::MediaClose {
                                    call_id,
                                    webview_ptr: _wv_ptr,
                                    session_id,
                                } => {
                                    let close_js = crate::media_api::close_session_js(session_id);
                                    entity.read(cx).evaluate_script(&close_js, cx);
                                    crate::media_api::close_session(session_id, "closed by app");
                                    let resolve_js = format!(
                                        "window.__epocaResolve({call_id}, null, true)"
                                    );
                                    entity.read(cx).evaluate_script(&resolve_js, cx);
                                }
                                crate::js_bridge::BridgeAsyncAction::WssPermission { url } => {
                                    if self.pending_wss_permission.is_none() {
                                        self.pending_wss_permission = Some(PendingWssPermission {
                                            app_id: app_id.clone(),
                                            url,
                                            webview_ptr: ev_ptr,
                                            call_id: id,
                                        });
                                        self.trigger_sidebar_show(false, cx);
                                        cx.notify();
                                    }
                                }
                                crate::js_bridge::BridgeAsyncAction::HttpPermission { origin } => {
                                    if self.pending_http_permission.is_none() {
                                        self.pending_http_permission = Some(PendingHttpPermission {
                                            app_id: app_id.clone(),
                                            origin,
                                            webview_ptr: ev_ptr,
                                        });
                                        self.trigger_sidebar_show(false, cx);
                                        cx.notify();
                                    }
                                }
                            },
                            crate::js_bridge::BridgeResult::UnknownMethod(m) => {
                                let msg = format!("method '{m}' not yet implemented")
                                    .replace('\'', "\\'");
                                let js = format!(
                                    "window.__epocaResolve({id}, '{msg}', null)"
                                );
                                entity.read(cx).evaluate_script(&js, cx);
                            }
                        }
                        break;
                    }
                }
            }
        }

        // Drain chain API responses (smoldot → SPA apps).
        for (webview_ptr, js_id, result) in crate::chain_api::drain_responses() {
            // Check if this response belongs to a hostapi JSON-RPC request.
            if let Some((wv_ptr, request_id)) = self.pending_hostapi_rpc.remove(&js_id) {
                let resp = match result {
                    Ok(val) => {
                        let json_result = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": js_id,
                            "result": val,
                        }).to_string();
                        epoca_hostapi::protocol::encode_jsonrpc_send_response(&request_id, &json_result)
                    }
                    Err(e) => {
                        let json_error = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": js_id,
                            "error": { "code": -32603, "message": e },
                        }).to_string();
                        epoca_hostapi::protocol::encode_jsonrpc_send_response(&request_id, &json_error)
                    }
                };
                crate::host::send_response(wv_ptr, &resp);
                continue;
            }

            // Standard JSON-path response (window.epoca.chain.query).
            let js = match result {
                Ok(val) => {
                    let json = serde_json::to_string(&val).unwrap_or_else(|_| "null".into());
                    format!("window.__epocaResolve({js_id}, null, {json})")
                }
                Err(e) => {
                    let msg = e.replace('\'', "\\'");
                    format!("window.__epocaResolve({js_id}, '{msg}', null)")
                }
            };
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == webview_ptr {
                        entity.read(cx).evaluate_script(&js, cx);
                        break;
                    }
                }
            }
        }

        // Drain statement events (push to subscribed webviews).
        for ev in crate::statements_api::drain_events() {
            let json = serde_json::json!({
                "author": ev.statement.author,
                "channel": ev.statement.channel,
                "data": ev.statement.data,
                "timestamp": ev.statement.timestamp_ms,
            });
            let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "{}".into());
            let js = format!("window.__epocaPush('statement', {json_str})");
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == ev.webview_ptr {
                        entity.read(cx).evaluate_script(&js, cx);
                        break;
                    }
                }
            }
        }

        // Drain data connection events (push to webviews).
        for ev in crate::data_api::drain_events() {
            let js = match ev.event_type {
                crate::data_api::DataEventType::Connected { conn_id, ref peer_address } => {
                    let json = serde_json::json!({ "connId": conn_id, "peer": peer_address });
                    format!("window.__epocaPush('dataConnected', {})", serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
                }
                crate::data_api::DataEventType::Data { conn_id, ref data } => {
                    let json = serde_json::json!({ "connId": conn_id, "data": data });
                    format!("window.__epocaPush('dataMessage', {})", serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
                }
                crate::data_api::DataEventType::Closed { conn_id, ref reason } => {
                    let json = serde_json::json!({ "connId": conn_id, "reason": reason });
                    format!("window.__epocaPush('dataClosed', {})", serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
                }
                crate::data_api::DataEventType::Error { conn_id, ref error } => {
                    let json = serde_json::json!({ "connId": conn_id, "error": error });
                    format!("window.__epocaPush('dataError', {})", serde_json::to_string(&json).unwrap_or_else(|_| "{}".into()))
                }
            };
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == ev.webview_ptr {
                        entity.read(cx).evaluate_script(&js, cx);
                        break;
                    }
                }
            }
        }

        // Check for incoming data connections that need approval (from the listener).
        if self.pending_data_connect.is_none() {
            let approvals = crate::data_api::pending_approvals();
            // Find the first incoming approval that doesn't already have a pending dialog.
            for (conn_id, app_id, peer_address) in approvals {
                // Find the webview_ptr for this app.
                let wv_ptr = self.tabs.iter().find_map(|t| {
                    t.entity.clone().downcast::<SpaTab>().ok().and_then(|e| {
                        let spa = e.read(cx);
                        if spa.app_id() == app_id { Some(spa.webview_ptr) } else { None }
                    })
                });
                if let Some(webview_ptr) = wv_ptr {
                    self.pending_data_connect = Some(PendingDataConnect {
                        webview_ptr,
                        id: 0, // incoming — no JS promise to resolve
                        app_id,
                        peer_address,
                        conn_id,
                    });
                    cx.notify();
                    break;
                }
            }
        }

        // Drain media events (push mediaTrackReady, mediaError, etc. to SPA webviews).
        for ev in crate::media_api::drain_events() {
            let js = match ev.event_type {
                crate::media_api::MediaEventType::TrackReady { track_id, ref kind } => {
                    let json = serde_json::json!({ "trackId": track_id, "kind": kind });
                    format!(
                        "window.__epocaPush('mediaTrackReady', {})",
                        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
                    )
                }
                crate::media_api::MediaEventType::SessionConnected { session_id, ref peer_address } => {
                    let json = serde_json::json!({ "sessionId": session_id, "peer": peer_address });
                    format!(
                        "window.__epocaPush('mediaConnected', {})",
                        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
                    )
                }
                crate::media_api::MediaEventType::RemoteTrack { session_id, track_id, ref kind } => {
                    let json = serde_json::json!({ "sessionId": session_id, "trackId": track_id, "kind": kind });
                    format!(
                        "window.__epocaPush('mediaRemoteTrack', {})",
                        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
                    )
                }
                crate::media_api::MediaEventType::SessionClosed { session_id, ref reason } => {
                    let json = serde_json::json!({ "sessionId": session_id, "reason": reason });
                    format!(
                        "window.__epocaPush('mediaClosed', {})",
                        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
                    )
                }
                crate::media_api::MediaEventType::Error { session_id, ref error } => {
                    let json = serde_json::json!({ "sessionId": session_id, "error": error });
                    format!(
                        "window.__epocaPush('mediaError', {})",
                        serde_json::to_string(&json).unwrap_or_else(|_| "{}".into())
                    )
                }
                crate::media_api::MediaEventType::EvalJs { ref js } => {
                    js.clone()
                }
            };
            for tab in &self.tabs {
                if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    if entity.read(cx).webview_ptr == ev.webview_ptr {
                        entity.read(cx).evaluate_script(&js, cx);
                        break;
                    }
                }
            }
        }

        // Drain Polkadot app host-api events (epocaHostApi WKScriptMessageHandler → SCALE binary)
        {
            let hostapi_events = crate::host::drain_hostapi_events();
            if !hostapi_events.is_empty() {
                log::info!("[sign-debug] drained {} hostapi events", hostapi_events.len());
                // Sync accounts from wallet before processing events.
                if cx.has_global::<crate::wallet::WalletGlobal>() {
                    let wallet = cx.global::<crate::wallet::WalletGlobal>();
                    let mut api = cx.global::<crate::host::HostApiGlobal>().api.lock().unwrap();
                    crate::host::sync_accounts_from_wallet(&mut api, wallet);
                }
            }

            for (ev_ptr, b64) in hostapi_events {
                log::info!("[sign-debug] processing hostapi event from webview {ev_ptr:#x}, b64 len={}", b64.len());
                // Match event to a live SpaTab — skip stale messages from closed tabs.
                let app_id = self.tabs.iter().find_map(|tab| {
                    let entity = tab.entity.clone().downcast::<SpaTab>().ok()?;
                    if entity.read(cx).webview_ptr == ev_ptr {
                        Some(entity.read(cx).app_id().to_string())
                    } else {
                        None
                    }
                });
                let app_id = match app_id {
                    Some(id) => id,
                    None => {
                        log::warn!("[sign-debug] no matching SpaTab for webview {ev_ptr:#x} — dropping");
                        continue;
                    }
                };

                // Decode base64 → binary
                let raw = match crate::host::base64_decode(&b64) {
                    Some(bytes) => bytes,
                    None => {
                        log::warn!("[sign-debug] invalid base64 from webview {ev_ptr:#x}");
                        continue;
                    }
                };

                log::info!("[sign-debug] decoded {} bytes for app={}, first bytes: {:?}", raw.len(), app_id, &raw[..raw.len().min(20)]);

                // Process through HostApi (scoped to app_id)
                let outcome = cx
                    .global::<crate::host::HostApiGlobal>()
                    .api
                    .lock()
                    .unwrap()
                    .handle_message(&raw, &app_id);

                match outcome {
                    epoca_hostapi::HostApiOutcome::Response(ref response) => {
                        log::info!("[sign-debug] outcome=Response ({} bytes)", response.len());
                        crate::host::send_response(ev_ptr, response);
                    }
                    epoca_hostapi::HostApiOutcome::NeedsSign {
                        request_id,
                        request_tag,
                        public_key: _,
                        payload,
                    } => {
                        log::info!("[sign-debug] outcome=NeedsSign request_id={} tag={} payload_len={}", request_id, request_tag, payload.len());
                        if self.pending_hostapi_sign.is_some() {
                            // Already have a pending sign — reject this one.
                            let is_raw = request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ;
                            let response = epoca_hostapi::protocol::encode_sign_error(&request_id, is_raw);
                            crate::host::send_response(ev_ptr, &response);
                        } else {
                            self.pending_hostapi_sign = Some(PendingHostApiSign {
                                webview_ptr: ev_ptr,
                                request_id,
                                request_tag,
                                app_id: app_id.clone(),
                                payload,
                            });
                            self.trigger_sidebar_show(false, cx);
                            cx.notify();
                        }
                    }
                    epoca_hostapi::HostApiOutcome::NeedsChainQuery {
                        request_id,
                        method,
                        params,
                    } => {
                        log::info!("[hostapi] NeedsChainQuery method={method} request_id={request_id}");
                        // Resolve chain from the SPA tab's manifest.
                        let chain_str = self.tabs.iter().find_map(|tab| {
                            let entity = tab.entity.clone().downcast::<SpaTab>().ok()?;
                            if entity.read(cx).webview_ptr == ev_ptr {
                                Some(entity.read(cx).chain().to_string())
                            } else {
                                None
                            }
                        }).unwrap_or_else(|| "paseo-asset-hub".into());

                        let chain_id = crate::chain_api::parse_chain_id(&chain_str);
                        match chain_id {
                            Some(chain) => {
                                // Use a unique js_id to correlate the response.
                                use std::sync::atomic::{AtomicU64, Ordering};
                                static HOSTAPI_RPC_ID: AtomicU64 = AtomicU64::new(1_000_000_000);
                                let js_id = HOSTAPI_RPC_ID.fetch_add(1, Ordering::Relaxed);

                                match crate::chain_api::submit_query(chain, ev_ptr, js_id, &method, &params) {
                                    Ok(()) => {
                                        self.pending_hostapi_rpc.insert(js_id, (ev_ptr, request_id));
                                    }
                                    Err(e) => {
                                        log::warn!("[hostapi] chain query rejected: {e}");
                                        let resp = epoca_hostapi::protocol::encode_jsonrpc_send_error(&request_id);
                                        crate::host::send_response(ev_ptr, &resp);
                                    }
                                }
                            }
                            None => {
                                log::warn!("[hostapi] unknown chain: {chain_str}");
                                let resp = epoca_hostapi::protocol::encode_jsonrpc_send_error(&request_id);
                                crate::host::send_response(ev_ptr, &resp);
                            }
                        }
                    }
                    epoca_hostapi::HostApiOutcome::NeedsNavigate {
                        request_id,
                        url,
                    } => {
                        log::info!("[hostapi] NeedsNavigate url={url} request_id={request_id}");
                        // Send success response back to the app immediately.
                        let response = epoca_hostapi::protocol::encode_navigate_response(&request_id);
                        crate::host::send_response(ev_ptr, &response);
                        // Open the URL — handles .dot addresses, http(s), etc.
                        if url.ends_with(".dot") && !url.contains('/') && !url.contains(' ') {
                            let dot_url = format!("dot://{url}");
                            self.resolve_dot_url(&dot_url, window, cx);
                        } else if url.starts_with("dot://") {
                            self.resolve_dot_url(&url, window, cx);
                        } else if url.starts_with("http://") || url.starts_with("https://") {
                            self.navigate_or_open(&url, window, cx);
                        }
                    }
                    epoca_hostapi::HostApiOutcome::Silent => {}
                }
            }
        }

        // Drain wallet events from regular WebView tabs (injectedWeb3 → epocaWallet handler)
        let wallet_enabled = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.experimental_wallet)
            .unwrap_or(false);
        if wallet_enabled {
            // Advance wallet state machine — check auto-lock / sleep-lock
            if cx.has_global::<crate::wallet::WalletGlobal>() {
                let locked = cx.global_mut::<crate::wallet::WalletGlobal>().manager.tick();
                if locked {
                    self.connected_sites.clear();
                    for tab in &self.tabs {
                        if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                            entity.update(cx, |wv, _| wv.wallet_connected = false);
                        }
                    }
                    self.wallet_popover_open = false;
                    cx.notify();
                }
            }
            let wallet_events = crate::wallet::drain_wallet_events();
            for ev in wallet_events {
                // Find the WebViewTab matching this event's webview_ptr
                let mut found = false;
                for tab in &self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        if entity.read(cx).webview_ptr == ev.webview_ptr {
                            found = true;
                            match ev.method.as_str() {
                                "enable" => {
                                    if !cx.has_global::<crate::wallet::WalletGlobal>() {
                                        let js = format!(
                                            "window.__epocaWalletResolve({}, 'no wallet configured', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else {
                                        // Auto-unlock if locked
                                        let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
                                        if matches!(wg.manager.state(), epoca_wallet::WalletState::Locked) {
                                            let _ = wg.manager.unlock();
                                        }
                                        if !matches!(wg.manager.state(), epoca_wallet::WalletState::Unlocked { .. }) {
                                            let js = format!(
                                                "window.__epocaWalletResolve({}, 'wallet is locked or not configured', null)",
                                                ev.id,
                                            );
                                            entity.read(cx).evaluate_script(&js, cx);
                                        } else {
                                            let origin = hostname_from_url(entity.read(cx).url()).to_string();
                                            // Auto-approve if already connected this session
                                            if self.connected_sites.contains(&origin) {
                                                self.resolve_wallet_enable(ev.webview_ptr, ev.id, &origin, cx);
                                            } else if self.pending_wallet_connect.is_some() {
                                                let js = format!(
                                                    "window.__epocaWalletResolve({}, 'another connection request is pending', null)",
                                                    ev.id,
                                                );
                                                entity.read(cx).evaluate_script(&js, cx);
                                            } else {
                                                self.pending_wallet_connect = Some(PendingWalletConnect {
                                                    webview_ptr: ev.webview_ptr,
                                                    id: ev.id,
                                                    origin,
                                                    channel: WalletChannel::Polkadot,
                                                });
                                                cx.notify();
                                            }
                                        }
                                    }
                                }
                                method @ ("signPayload" | "signRaw") => {
                                    let origin_url = entity.read(cx).url().to_string();
                                    let origin_host = hostname_from_url(&origin_url).to_string();
                                    if !self.connected_sites.contains(&origin_host) {
                                        let js = format!(
                                            "window.__epocaWalletResolve({}, 'site not connected', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else if self.pending_wallet_sign.is_some() {
                                        let js = format!(
                                            "window.__epocaWalletResolve({}, 'another signing request is pending', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else {
                                        let display_message = if method == "signPayload" {
                                            "Sign an extrinsic (transaction)".to_string()
                                        } else {
                                            "Sign a raw message".to_string()
                                        };
                                        self.pending_wallet_sign = Some(PendingWalletSign {
                                            webview_ptr: ev.webview_ptr,
                                            id: ev.id,
                                            method: method.to_string(),
                                            params_json: ev.params_json,
                                            display_message,
                                            origin: origin_host,
                                        });
                                        self.trigger_sidebar_show(false, cx);
                                        cx.notify();
                                    }
                                }
                                other => {
                                    let msg = format!("unknown wallet method '{}'", other)
                                        .replace('\'', "\\'");
                                    let js = format!(
                                        "window.__epocaWalletResolve({}, '{}', null)",
                                        ev.id, msg,
                                    );
                                    entity.read(cx).evaluate_script(&js, cx);
                                }
                            }
                            break;
                        }
                    }
                }
                if !found {
                    log::warn!("Wallet event for unknown webview_ptr={}", ev.webview_ptr);
                }
            }
        }

        // Drain BTC wallet events (window.bitcoin → epocaBtcWallet handler)
        let btc_wallet_enabled = cx
            .try_global::<crate::settings::SettingsGlobal>()
            .map(|g| g.settings.experimental_wallet && g.settings.experimental_btc)
            .unwrap_or(false);

        if btc_wallet_enabled {
            let btc_events = crate::wallet::drain_btc_wallet_events();
            for ev in btc_events {
                let mut found = false;
                for tab in &self.tabs {
                    if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                        if entity.read(cx).webview_ptr == ev.webview_ptr {
                            found = true;
                            match ev.method.as_str() {
                                "requestAccounts" | "getAccounts" => {
                                    if !cx.has_global::<crate::wallet::WalletGlobal>() {
                                        let js = format!(
                                            "window.__epocaBtcResolve({}, 'no wallet configured', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else {
                                        let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
                                        if matches!(wg.manager.state(), epoca_wallet::WalletState::Locked) {
                                            let _ = wg.manager.unlock();
                                        }
                                        if !matches!(wg.manager.state(), epoca_wallet::WalletState::Unlocked { .. }) {
                                            let js = format!(
                                                "window.__epocaBtcResolve({}, 'wallet is locked', null)",
                                                ev.id,
                                            );
                                            entity.read(cx).evaluate_script(&js, cx);
                                        } else {
                                            let origin = hostname_from_url(entity.read(cx).url()).to_string();
                                            if self.connected_sites.contains(&origin) {
                                                self.resolve_btc_accounts(ev.webview_ptr, ev.id, &origin, cx);
                                            } else if ev.method == "getAccounts" {
                                                // getAccounts is non-prompting — return empty
                                                let js = format!(
                                                    "window.__epocaBtcResolve({}, null, [])", ev.id
                                                );
                                                entity.read(cx).evaluate_script(&js, cx);
                                            } else if self.pending_wallet_connect.is_some() {
                                                let js = format!(
                                                    "window.__epocaBtcResolve({}, 'another connection request is pending', null)",
                                                    ev.id,
                                                );
                                                entity.read(cx).evaluate_script(&js, cx);
                                            } else {
                                                self.pending_wallet_connect = Some(PendingWalletConnect {
                                                    webview_ptr: ev.webview_ptr,
                                                    id: ev.id,
                                                    origin,
                                                    channel: WalletChannel::Btc,
                                                });
                                                cx.notify();
                                            }
                                        }
                                    }
                                }
                                "getNetwork" => {
                                    let js = format!(
                                        "window.__epocaBtcResolve({}, null, 'livenet')", ev.id
                                    );
                                    entity.read(cx).evaluate_script(&js, cx);
                                }
                                "getBalance" => {
                                    // Stub — zeros until Kyoto UTXO scanning (Phase 3.5)
                                    let js = format!(
                                        "window.__epocaBtcResolve({}, null, {{confirmed:0,unconfirmed:0,total:0}})",
                                        ev.id
                                    );
                                    entity.read(cx).evaluate_script(&js, cx);
                                }
                                "signMessage" => {
                                    let origin_url = entity.read(cx).url().to_string();
                                    let origin_host = hostname_from_url(&origin_url).to_string();
                                    if !self.connected_sites.contains(&origin_host) {
                                        let js = format!(
                                            "window.__epocaBtcResolve({}, 'site not connected — call requestAccounts first', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else if self.pending_btc_wallet_sign.is_some() {
                                        let js = format!(
                                            "window.__epocaBtcResolve({}, 'another signing request is pending', null)",
                                            ev.id,
                                        );
                                        entity.read(cx).evaluate_script(&js, cx);
                                    } else {
                                        let mut message = serde_json::from_str::<serde_json::Value>(&ev.params_json)
                                            .ok()
                                            .and_then(|v| v["message"].as_str().map(|s| s.to_string()))
                                            .unwrap_or_default();
                                        // Cap message size held in memory before sign dialog (64 KiB matches WalletManager limit)
                                        message.truncate(65_536);
                                        self.pending_btc_wallet_sign = Some(PendingBtcWalletSign {
                                            webview_ptr: ev.webview_ptr,
                                            id: ev.id,
                                            message,
                                            origin: origin_host,
                                        });
                                        self.trigger_sidebar_show(false, cx);
                                        cx.notify();
                                    }
                                }
                                other => {
                                    let msg = format!("unknown method '{}'", other)
                                        .replace('\'', "\\'");
                                    let js = format!(
                                        "window.__epocaBtcResolve({}, '{}', null)",
                                        ev.id, msg,
                                    );
                                    entity.read(cx).evaluate_script(&js, cx);
                                }
                            }
                            break;
                        }
                    }
                }
                if !found {
                    log::warn!("BtcWallet event for unknown webview_ptr={}", ev.webview_ptr);
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
                    self.url_bar_clicked = true;
                    crate::shield::set_url_bar_focused(true);
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
                ShortcutAction::OpenTestSpa => {
                    self.resolve_dot_url("dot://test-spa.dot", window, cx);
                }
                ShortcutAction::UrlBarSubmit => {
                    let text = self.url_input.read(cx).value().to_string();
                    log::info!("[nav] UrlBarSubmit (via key monitor): {:?}", text);
                    self.url_bar_clicked = false;
                    crate::shield::set_url_bar_focused(false);
                    if !text.is_empty() {
                        self.pending_nav = Some(text);
                        cx.notify();
                    }
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
        match event {
            InputEvent::PressEnter { .. } => {
                let text = self.omnibox_input.read(cx).value().to_string();
                if !text.is_empty() {
                    self.omnibox_pending_nav = Some(text);
                    cx.notify();
                }
            }
            InputEvent::Change => {
                let query = self.omnibox_input.read(cx).value().to_string();
                self.omnibox_history_results = cx
                    .try_global::<crate::history::HistoryGlobal>()
                    .map(|hg| hg.manager.search(&query, 8))
                    .unwrap_or_default();
                cx.notify();
            }
            _ => {}
        }
    }

    fn navigate_or_open(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if text == "epoca://onboard" || text == "epoca://welcome" {
            self.open_onboard(window, cx);
            return;
        }
        if text.starts_with("dot://") {
            self.resolve_dot_url(text, window, cx);
            return;
        }
        if text.ends_with(".dot") && !text.contains('/') && !text.contains(' ') {
            let dot_url = format!("dot://{text}");
            self.resolve_dot_url(&dot_url, window, cx);
            return;
        }
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
                            if bundle.manifest.app.app_type == "spa" {
                                self.open_spa(bundle, window, cx);
                            } else if bundle.manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer) {
                                self.open_framebuffer_app(bundle, window, cx);
                            } else {
                                // Non-framebuffer .prod — open as regular sandbox app
                                let name = bundle.manifest.app.name.clone();
                                let config = epoca_sandbox::SandboxConfig::default();
                                let program_bytes = bundle.program_bytes.as_deref().unwrap_or(&[]);
                                match epoca_sandbox::SandboxInstance::from_bytes(program_bytes, &config) {
                                    Ok(_) => {
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
        self.page_info_open = false;
        self.active_tab_id = Some(tab_id);
        let tab = self.tabs.iter().find(|t| t.id == tab_id);
        let value = tab
            .map(|tab| match &tab.kind {
                TabKind::WebView { url } => url.clone(),
                TabKind::DeclarativeApp { path } => path.clone(),
                TabKind::SandboxApp { app_id } => app_id.clone(),
                TabKind::FramebufferApp { app_id } => app_id.clone(),
                TabKind::Spa { .. } => {
                    // Show dot:// URL for .dot apps instead of the raw app_id
                    if let Some(ref dv) = tab.dot_verification {
                        format!("dot://{}.dot", dv.name)
                    } else {
                        tab.title.clone()
                    }
                }
                TabKind::CodeEditor { path } => path.clone().unwrap_or_default(),
                TabKind::DotLoading { ref name } => format!("dot://{name}.dot"),
                TabKind::Welcome | TabKind::Settings | TabKind::AppLibrary | TabKind::Bookmarks => String::new(),
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

    /// Move a tab from one index to another within the same pinned/regular group.
    fn reorder_tab(&mut self, tab_id: u64, target_idx: usize) {
        let Some(src_idx) = self.tabs.iter().position(|t| t.id == tab_id) else { return };
        if src_idx == target_idx { return; }
        let tab = self.tabs.remove(src_idx);
        let clamped = target_idx.min(self.tabs.len());
        self.tabs.insert(clamped, tab);
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

    /// Open the onboarding page, or switch to it if already open.
    pub fn open_onboard(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::Welcome).map(|t| t.id) {
            self.active_tab_id = Some(id);
            cx.notify();
            return;
        }
        let id = self.alloc_id();
        let entity = cx.new(|cx| WelcomeTab::new(cx));
        self._welcome_subscription = Some(cx.subscribe(&entity, Self::on_welcome_event));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::Welcome,
            title: "Welcome".to_string(),
            icon: IconName::Globe,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
            loading_progress: 0.0,
            reader_active: false,
            readerable: false,
            dot_verification: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    fn on_welcome_event(
        &mut self,
        _entity: Entity<WelcomeTab>,
        event: &WelcomeEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            WelcomeEvent::Navigate(url) => {
                self.pending_nav = Some(url.clone());
                cx.notify();
            }
        }
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
            loading_progress: 0.0,
            reader_active: false,
            readerable: false,
            dot_verification: None,
        });
        self.active_tab_id = Some(id);
        record_history_visit(&url_clone, "", cx);
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
            kind: TabKind::WebView { url: url_clone.clone() },
            title,
            icon: IconName::Globe,
            entity: entity.into(),
            pinned: false,
            nav: Some(nav),
            favicon_url: None,
            context_id,
            loading_progress: 0.0,
            reader_active: false,
            readerable: false,
            dot_verification: None,
        });
        // Do NOT change active_tab_id — stay on current tab.
        record_history_visit(&url_clone, "", cx);
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    /// Open the App Library tab, or switch to it if already open.
    pub fn open_app_library(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::AppLibrary).map(|t| t.id) {
            // Refresh the app list when switching back.
            if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                if let Ok(entity) = tab.entity.clone().downcast::<AppLibraryTab>() {
                    entity.update(cx, |tab, _cx| tab.refresh());
                }
            }
            self.active_tab_id = Some(id);
            cx.notify();
            return;
        }
        let id = self.alloc_id();
        let entity = cx.new(|cx| AppLibraryTab::new(cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::AppLibrary,
            title: "App Library".to_string(),
            icon: IconName::SquareTerminal,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    /// Open the Bookmarks tab, or switch to it if already open.
    /// Also handles "open bookmark URL" when triggered from within the bookmarks panel.
    pub fn open_bookmarks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // If triggered from a bookmark click, open the URL instead.
        if let Some(url) = crate::tabs::take_pending_bookmark_open() {
            self.open_webview(url, window, cx);
            return;
        }

        if let Some(tab) = self.tabs.iter().find(|t| t.kind == TabKind::Bookmarks) {
            let id = tab.id;
            if let Ok(entity) = tab.entity.clone().downcast::<BookmarksTab>() {
                entity.update(cx, |tab, _cx| tab.refresh());
            }
            self.active_tab_id = Some(id);
            cx.notify();
            return;
        }
        let id = self.alloc_id();
        let entity = cx.new(|cx| BookmarksTab::new(cx));
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::Bookmarks,
            title: "Bookmarks".to_string(),
            icon: IconName::Star,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
            loading_progress: 0.0,
            reader_active: false,
            readerable: false,
            dot_verification: None,
        });
        self.active_tab_id = Some(id);
        cx.notify();
    }

    /// Toggle bookmark for the active tab's URL.
    pub fn toggle_bookmark(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_id) = self.active_tab_id else { return };
        let Some(tab) = self.tabs.iter().find(|t| t.id == active_id) else { return };

        // Only WebView tabs have bookmarkable URLs.
        let TabKind::WebView { ref url } = tab.kind else { return };
        let url = url.clone();
        let title = tab.title.clone();

        crate::bookmarks::toggle(&url, &title);
        cx.notify();
    }

    /// Handle the OpenApp action: either launch a pending bundle from the library,
    /// or show a file picker to install a new .prod file.
    pub fn handle_open_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Check if there's a pending launch from the App Library tab.
        if let Some(bundle) = crate::tabs::take_pending_launch() {
            if bundle.manifest.app.app_type == "spa" {
                self.open_spa(bundle, window, cx);
            } else if bundle.manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer) {
                self.open_framebuffer_app(bundle, window, cx);
            }
            return;
        }

        // No pending launch — show native file picker.
        let paths: Option<std::path::PathBuf> = {
            #[cfg(target_os = "macos")]
            {
                use objc2::runtime::AnyClass;
                unsafe {
                    let panel_cls = AnyClass::get("NSOpenPanel").unwrap();
                    let panel: *mut objc2::runtime::AnyObject = objc2::msg_send![panel_cls, openPanel];
                    let _: () = objc2::msg_send![panel, setCanChooseFiles: true];
                    let _: () = objc2::msg_send![panel, setCanChooseDirectories: false];
                    let _: () = objc2::msg_send![panel, setAllowsMultipleSelection: false];
                    // Set allowed file types to .prod
                    let ns_string_cls = AnyClass::get("NSString").unwrap();
                    let mut buf = b"prod\0".to_vec();
                    let ext: *mut objc2::runtime::AnyObject = objc2::msg_send![
                        ns_string_cls, stringWithUTF8String: buf.as_ptr()
                    ];
                    let ns_array_cls = AnyClass::get("NSArray").unwrap();
                    let types: *mut objc2::runtime::AnyObject = objc2::msg_send![
                        ns_array_cls, arrayWithObject: ext
                    ];
                    let _: () = objc2::msg_send![panel, setAllowedFileTypes: types];
                    let result: isize = objc2::msg_send![panel, runModal];
                    if result == 1 {
                        let url: *mut objc2::runtime::AnyObject = objc2::msg_send![panel, URL];
                        let path_ns: *mut objc2::runtime::AnyObject = objc2::msg_send![url, path];
                        let utf8: *const u8 = objc2::msg_send![path_ns, UTF8String];
                        if !utf8.is_null() {
                            let c_str = std::ffi::CStr::from_ptr(utf8 as *const i8);
                            c_str.to_str().ok().map(|s| std::path::PathBuf::from(s))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            { None }
        };

        if let Some(path) = paths {
            // Install the bundle first.
            match crate::app_library::install_prod(&path) {
                Ok(_install_dir) => {
                    // Now load from installed location.
                    match ProdBundle::from_file(&path) {
                        Ok(bundle) => {
                            let app_id = bundle.manifest.app.id.clone();
                            crate::app_library::touch_last_launched(&app_id);
                            if bundle.manifest.app.app_type == "spa" {
                                self.open_spa(bundle, window, cx);
                            } else if bundle.manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer) {
                                self.open_framebuffer_app(bundle, window, cx);
                            }
                        }
                        Err(e) => log::error!("Failed to load .prod: {e}"),
                    }
                }
                Err(e) => log::error!("Failed to install .prod: {e}"),
            }

            // Refresh the App Library tab if it's open.
            if let Some(tab) = self.tabs.iter().find(|t| t.kind == TabKind::AppLibrary) {
                if let Ok(entity) = tab.entity.clone().downcast::<AppLibraryTab>() {
                    entity.update(cx, |tab, _cx| tab.refresh());
                }
            }
        }
    }

    pub fn reload_active_tab(&mut self, hard: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.active_tab_id {
            if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                if let Ok(entity) = tab.entity.clone().downcast::<crate::tabs::WebViewTab>() {
                    entity.update(cx, |tab, cx| {
                        if hard { tab.hard_reload(cx); } else { tab.reload(cx); }
                    });
                } else if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                    entity.update(cx, |tab, cx| tab.reload(cx));
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(app_id_clone, window, inner_cx));
        cx.notify();
    }

    pub fn open_spa(
        &mut self,
        bundle: ProdBundle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_spa_with_verification(bundle, None, window, cx);
    }

    pub fn open_spa_with_verification(
        &mut self,
        bundle: ProdBundle,
        verification: Option<DotVerification>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.alloc_id();
        let app_id = bundle.manifest.app.id.clone();
        let app_name = bundle.manifest.app.name.clone();
        let has_data_perm = bundle
            .manifest
            .permissions
            .as_ref()
            .map(|p| p.data)
            .unwrap_or(false);
        let is_ipfs = bundle.ipfs_cid.is_some();
        let entity = cx.new(|cx| {
            SpaTab::new(bundle, window, cx)
        });
        // Start incoming data connection listener if the app has data permission.
        if has_data_perm {
            let wv_ptr = entity.read(cx).webview_ptr;
            crate::data_api::start_incoming_listener(&app_id, wv_ptr);
        }
        // Show dot://name.dot if we have verification, otherwise derive from app name.
        let url_display = if let Some(ref dv) = verification {
            format!("dot://{}.dot", dv.name)
        } else {
            format!("dot://{}.dot", app_name)
        };
        self.tabs.push(TabEntry {
            id,
            kind: TabKind::Spa { app_id: app_id.clone() },
            title: app_name,
            icon: IconName::Globe,
            entity: entity.into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
            loading_progress: if is_ipfs { 0.5 } else { 0.0 },
            reader_active: false,
            readerable: false,
            dot_verification: verification,
        });
        self.active_tab_id = Some(id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(url_display, window, inner_cx));
        // For IPFS-loaded SPAs, start the border glow and schedule it to stop
        // once the lazy asset fetch is done (the custom protocol handler blocks
        // on IPFS, so WebView loading-progress events don't fire).
        if is_ipfs {
            if self._loading_glow_task.is_none() {
                self.start_loading_glow(cx);
            }
            let tab_id = id;
            cx.spawn(async move |this: WeakEntity<Self>, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(3))
                    .await;
                let _ = cx.update(|cx| {
                    if let Some(entity) = this.upgrade() {
                        entity.update(cx, |wb, cx| {
                            if let Some(te) = wb.tabs.iter_mut().find(|t| t.id == tab_id) {
                                te.loading_progress = 0.0;
                            }
                            cx.notify();
                        });
                    }
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Resolve a `dot://<name>.dot` URL: immediately opens a loading tab,
    /// resolves the bundle (local or on-chain), then shows permissions for
    /// user approval before opening the actual SPA.
    pub fn resolve_dot_url(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Parse: dot://testproduct.dot → name = "testproduct"
        let host = url.strip_prefix("dot://").unwrap_or("");
        let name = host.strip_suffix(".dot").unwrap_or(host);
        let name = name.split('/').next().unwrap_or(name);

        if name.is_empty() {
            log::warn!("[dot] empty dot:// URL: {url}");
            return;
        }

        // Validate name: only alphanumeric, hyphens, underscores.
        if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            log::warn!("[dot] invalid dot name (illegal chars): {name}");
            return;
        }

        // Close any previous loading tab and clear pending state.
        if let Some(prev) = self.pending_dot_load.take() {
            self.tabs.retain(|t| t.id != prev.tab_id);
        }
        self.pending_dotns_validation = None;
        self.dot_load_generation += 1;
        let generation = self.dot_load_generation;

        // ── Create loading tab immediately ──
        let tab_id = self.alloc_id();
        let name_owned = name.to_string();
        let loading_entity = cx.new(|cx| DotLoadingTab::new(name_owned.clone(), cx));

        // Subscribe to approve/deny events from the loading tab.
        let sub = cx.subscribe(&loading_entity, Self::on_dot_loading_event);

        let dot_url = format!("dot://{name}.dot");
        self.tabs.push(TabEntry {
            id: tab_id,
            kind: TabKind::DotLoading { name: name_owned.clone() },
            title: format!("{name}.dot"),
            icon: IconName::Globe,
            entity: loading_entity.clone().into(),
            pinned: false,
            nav: None,
            favicon_url: None,
            context_id: None,
            loading_progress: 0.5, // triggers violet border glow during load
            reader_active: false,
            readerable: false,
            dot_verification: None,
        });
        self.active_tab_id = Some(tab_id);
        self.url_input
            .update(cx, |s, inner_cx| s.set_value(dot_url, window, inner_cx));
        if self._loading_glow_task.is_none() {
            self.start_loading_glow(cx);
        }

        self.pending_dot_load = Some(PendingDotLoad {
            tab_id,
            bundle: None,
            verification: None,
            _subscription: sub,
        });
        cx.notify();

        // ── Phase 1: look for a local .prod bundle ──
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()));
        let cwd = std::env::current_dir().ok();

        let mut candidates: Vec<std::path::PathBuf> = Vec::new();
        if let Some(ref dir) = cwd {
            candidates.push(dir.join(format!("examples/{name}.prod")));
            candidates.push(dir.join(format!("{name}.prod")));
        }
        if let Some(ref dir) = exe_dir {
            candidates.push(dir.join(format!("examples/{name}.prod")));
            candidates.push(dir.join(format!("{name}.prod")));
        }
        #[cfg(debug_assertions)]
        {
            let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent().unwrap().parent().unwrap().to_path_buf();
            candidates.push(workspace_root.join(format!("examples/{name}.prod")));
            candidates.push(workspace_root.join(format!("{name}.prod")));
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = std::path::PathBuf::from(home);
            candidates.push(home.join(format!(".epoca/apps/{name}.prod")));
        }

        log::info!("[dot] checking {} local candidates for: {name}", candidates.len());
        for candidate in &candidates {
            log::debug!("[dot] candidate: {} (exists={})", candidate.display(), candidate.exists());
            if candidate.exists() {
                match ProdBundle::from_file(candidate) {
                    Ok(bundle) => {
                        if bundle.manifest.app.app_type == "spa" {
                            log::info!("[dot] loaded local SPA: {}", candidate.display());
                            let verification = DotVerification {
                                name: name_owned.clone(),
                                status: DotStatus::Local,
                                cid: None,
                                owner: None,
                            };
                            // Extract permissions before moving bundle.
                            let perms = bundle.manifest.permissions.clone();
                            // Store bundle first so it's available if the user
                            // clicks Allow immediately after permission review.
                            if let Some(ref mut load) = self.pending_dot_load {
                                load.bundle = Some(bundle);
                                load.verification = Some(verification);
                            }
                            // Fast-transition: resolved → fetched → permission review.
                            loading_entity.update(cx, |tab, cx| {
                                tab.set_fetching("local".into());
                                tab.set_permission_review(perms, true);
                                cx.notify();
                            });
                            return;
                        } else {
                            log::warn!("[dot] {} is not an SPA bundle (app_type={:?})", candidate.display(), bundle.manifest.app.app_type);
                        }
                    }
                    Err(e) => {
                        log::warn!("[dot] failed to load {}: {e}", candidate.display());
                    }
                }
            }
        }

        // ── Phase 2: on-chain DOTNS resolution ──
        log::info!("[dot] no local bundle, trying DOTNS for: {name}");
        self.pending_dotns_validation = Some(DotVerification {
            name: name_owned.clone(),
            status: DotStatus::Pending,
            cid: None,
            owner: None,
        });

        let loading_entity_weak = loading_entity.downgrade();
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let name_for_resolve = name_owned.clone();
            // Lazy resolution: fetch only CID + manifest.toml (not all assets).
            let result = cx
                .background_executor()
                .spawn(async move { epoca_chain::dotns::resolve_lazy(&name_for_resolve) })
                .await;

            match result {
                Ok(resolution) => {
                    let _ = cx.update(|cx| {
                        if let Some(wb_entity) = this.upgrade() {
                            // Check generation to discard stale results.
                            let is_current = wb_entity.read(cx).dot_load_generation == generation;
                            if !is_current {
                                log::info!("[dotns] discarding stale result for {name_owned} (generation mismatch)");
                                return;
                            }
                        }
                        // Update loading tab (manifest already fetched).
                        if let Some(le) = loading_entity_weak.upgrade() {
                            le.update(cx, |tab, cx| {
                                tab.set_fetching(resolution.cid.clone());
                                cx.notify();
                            });
                        }
                        if let Some(wb_entity) = this.upgrade() {
                            let name_for_bundle = name_owned.clone();
                            wb_entity.update(cx, |wb, cx| {
                                // Parse manifest from fetched bytes, or synthesise
                                // a default for raw web apps (no manifest.toml).
                                let has_manifest = resolution.manifest_bytes.is_some();
                                let manifest: epoca_sandbox::bundle::ProdManifest = if let Some(raw) = resolution.manifest_bytes {
                                    let manifest_str = match String::from_utf8(raw) {
                                        Ok(s) => s,
                                        Err(_) => {
                                            log::warn!("[dotns] manifest.toml is not valid UTF-8");
                                            return;
                                        }
                                    };
                                    match toml::from_str(&manifest_str) {
                                        Ok(m) => m,
                                        Err(e) => {
                                            log::warn!("[dotns] failed to parse manifest.toml: {e}");
                                            return;
                                        }
                                    }
                                } else {
                                    // Raw web app (e.g. Product SDK app deployed
                                    // directly to IPFS without a .prod bundle).
                                    log::info!("[dotns] no manifest — using default SPA manifest for {name_for_bundle}");
                                    epoca_sandbox::bundle::ProdManifest {
                                        app: epoca_sandbox::bundle::AppMeta {
                                            id: name_for_bundle.clone(),
                                            name: name_for_bundle.clone(),
                                            version: "0.0.0".into(),
                                            app_type: "spa".into(),
                                            icon: None,
                                        },
                                        permissions: Some(epoca_sandbox::bundle::PermissionsMeta {
                                            network: None,
                                            sign: true,
                                            statements: false,
                                            chain: false,
                                            data: false,
                                            media: vec![],
                                        }),
                                        sandbox: None,
                                        webapp: Some(epoca_sandbox::bundle::WebAppMeta {
                                            entry: "index.html".into(),
                                            sandbox: "strict".into(),
                                            chain: "paseo-asset-hub".into(),
                                        }),
                                        assets: None,
                                    }
                                };

                                let verification = DotVerification {
                                    name: name_for_bundle.clone(),
                                    status: DotStatus::Verified,
                                    cid: Some(resolution.cid.clone()),
                                    owner: resolution.owner,
                                };

                                // For non-SPA bundles, extract program + assets from fetched files.
                                let mut all_files = resolution.all_files;
                                let program_bytes = all_files.remove("app.polkavm");

                                // Early return with a user-visible error when a framebuffer
                                // bundle is missing its PolkaVM program binary.
                                let is_framebuffer = manifest.app.app_type == "framebuffer"
                                    || manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer);
                                if is_framebuffer && program_bytes.is_none() {
                                    log::error!("[dotns] app.polkavm not found in IPFS bundle for {name_for_bundle}");
                                    if let Some(le) = loading_entity_weak.upgrade() {
                                        le.update(cx, |tab, cx| {
                                            tab.set_error("app.polkavm not found in bundle".into());
                                            cx.notify();
                                        });
                                    }
                                    return;
                                }

                                // Strip "assets/" prefix from remaining files.
                                let mut assets = HashMap::new();
                                for (k, v) in all_files {
                                    if k == "manifest.toml" { continue; }
                                    if k.ends_with(".prod") { continue; }
                                    let key = k.strip_prefix("assets/").unwrap_or(&k).to_string();
                                    assets.insert(key, v);
                                }
                                let bundle = ProdBundle {
                                    manifest,
                                    program_bytes,
                                    assets,
                                    ipfs_cid: Some(resolution.cid.clone()),
                                };

                                // Check if this CID was already approved — auto-open.
                                let already_approved = wb.approved_dot_apps
                                    .get(&name_for_bundle)
                                    .map(|approved_cid| approved_cid == &resolution.cid)
                                    .unwrap_or(false);

                                if already_approved {
                                    log::info!("[dotns] auto-approved (same CID): {name_for_bundle}");
                                    if let Some(load) = wb.pending_dot_load.take() {
                                        wb.tabs.retain(|t| t.id != load.tab_id);
                                    }
                                    wb.pending_dot_approve = Some((bundle, Some(verification)));
                                } else {
                                    let perms = bundle.manifest.permissions.clone();
                                    // Store bundle before showing permission review,
                                    // so it's available if the user clicks Allow.
                                    if let Some(ref mut load) = wb.pending_dot_load {
                                        load.bundle = Some(bundle);
                                        load.verification = Some(verification);
                                    }
                                    if let Some(le) = loading_entity_weak.upgrade() {
                                        le.update(cx, |tab, cx| {
                                            tab.set_permission_review(perms, has_manifest);
                                            cx.notify();
                                        });
                                    }
                                    // Stop the border glow — loading is done.
                                    if let Some(ref load) = wb.pending_dot_load {
                                        if let Some(te) = wb.tabs.iter_mut().find(|t| t.id == load.tab_id) {
                                            te.loading_progress = 0.0;
                                        }
                                    }
                                }
                                wb.pending_dotns_validation = None;
                                cx.notify();
                            });
                        }
                    });
                }
                Err(e) => {
                    log::warn!("[dotns] resolution failed for {name_owned}: {e}");
                    let _ = cx.update(|cx| {
                        // Check generation — discard stale errors.
                        if let Some(wb_entity) = this.upgrade() {
                            if wb_entity.read(cx).dot_load_generation != generation {
                                return;
                            }
                        }
                        if let Some(le) = loading_entity_weak.upgrade() {
                            le.update(cx, |tab, cx| {
                                tab.set_error(e.clone());
                                cx.notify();
                            });
                        }
                        if let Some(wb_entity) = this.upgrade() {
                            wb_entity.update(cx, |wb, cx| {
                                // Stop the border glow on error.
                                if let Some(ref load) = wb.pending_dot_load {
                                    if let Some(te) = wb.tabs.iter_mut().find(|t| t.id == load.tab_id) {
                                        te.loading_progress = 0.0;
                                    }
                                }
                                wb.pending_dotns_validation = Some(DotVerification {
                                    name: name_owned.clone(),
                                    status: DotStatus::Failed(e),
                                    cid: None,
                                    owner: None,
                                });
                                cx.notify();
                            });
                        }
                    });
                }
            }
        }).detach();
    }

    /// Handle Approved / Denied events from a DotLoadingTab.
    fn on_dot_loading_event(
        &mut self,
        _entity: Entity<DotLoadingTab>,
        event: &DotLoadingEvent,
        cx: &mut Context<Self>,
    ) {
        let load = match self.pending_dot_load.take() {
            Some(l) => l,
            None => return,
        };

        match event {
            DotLoadingEvent::Approved => {
                if let Some(bundle) = load.bundle {
                    let verification = load.verification;
                    // Record approval for this CID so subsequent loads skip the prompt.
                    if let Some(ref v) = verification {
                        if let Some(ref cid) = v.cid {
                            self.approved_dot_apps.insert(v.name.clone(), cid.clone());
                            crate::session::save_approved_apps(&self.approved_dot_apps);
                        }
                    }
                    // Replace the loading tab with the real SPA tab.
                    let tab_id = load.tab_id;
                    self.tabs.retain(|t| t.id != tab_id);
                    // Restart the border glow while the SPA/framebuffer spins up.
                    if self._loading_glow_task.is_none() {
                        self.start_loading_glow(cx);
                    }
                    // Need a window ref — defer to pending field picked up in render.
                    self.pending_dot_approve = Some((bundle, verification));
                } else {
                    log::warn!("[dot] approved but no bundle available");
                }
            }
            DotLoadingEvent::Denied => {
                // Defer close to render where we have window access.
                self.pending_dot_deny_tab = Some(load.tab_id);
            }
        }
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
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
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
                    });
                    restored_count += 1;
                }
                TabKind::FramebufferApp { .. } => {
                    // Skip — restoring FramebufferApp causes RefCell reentrancy;
                    // user can re-launch from App Library.
                }
                TabKind::AppLibrary => {
                    let id = self.alloc_id();
                    let entity = cx.new(|cx| AppLibraryTab::new(cx));
                    self.tabs.push(TabEntry {
                        id,
                        kind: TabKind::AppLibrary,
                        title: "App Library".to_string(),
                        icon: IconName::SquareTerminal,
                        entity: entity.into(),
                        pinned: stab.pinned,
                        nav: None,
                        favicon_url: None,
                        context_id: None,
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
                    });
                    restored_count += 1;
                }
                TabKind::Bookmarks => {
                    // Deduplicate — only one bookmarks tab allowed.
                    if self.tabs.iter().any(|t| t.kind == TabKind::Bookmarks) {
                        continue;
                    }
                    let id = self.alloc_id();
                    let entity = cx.new(|cx| BookmarksTab::new(cx));
                    self.tabs.push(TabEntry {
                        id,
                        kind: TabKind::Bookmarks,
                        title: "Bookmarks".to_string(),
                        icon: IconName::Star,
                        entity: entity.into(),
                        pinned: stab.pinned,
                        nav: None,
                        favicon_url: None,
                        context_id: None,
                        loading_progress: 0.0,
                        reader_active: false,
                        readerable: false,
                        dot_verification: None,
                    });
                    restored_count += 1;
                }
                _ => {
                    // Welcome, SandboxApp, Spa — skip
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

    pub fn toggle_reader_mode(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.active_tab_id else { return };
        let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
            if tab.reader_active {
                // Exit reader mode — reload original page
                entity.update(cx, |wv, cx| wv.reload(cx));
                tab.reader_active = false;
            } else {
                // Enter reader mode
                entity.update(cx, |wv, cx| {
                    wv.evaluate_script(crate::reader::reader_mode_js(), cx);
                });
                tab.reader_active = true;
            }
            cx.notify();
        }
    }

    /// Forward a clipboard command to the active WebViewTab.
    /// `cmd` is one of "copy", "cut", "paste", "selectAll".
    fn clipboard_to_webview(&self, cmd: &str, cx: &mut App) {
        let Some(id) = self.active_tab_id else { return };
        let Some(tab) = self.tabs.iter().find(|t| t.id == id) else { return };
        if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
            if cmd == "paste" {
                // Read the system clipboard and insert text via JS.
                if let Some(item) = cx.read_from_clipboard() {
                    if let Some(text) = item.text() {
                        let escaped = text
                            .replace('\\', "\\\\")
                            .replace('\'', "\\'")
                            .replace('\n', "\\n")
                            .replace('\r', "\\r")
                            .replace('\t', "\\t");
                        let js = format!(
                            r#"(function(){{
                                var el = document.activeElement;
                                if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) {{
                                    var dt = new DataTransfer();
                                    dt.setData('text/plain', '{}');
                                    var ev = new ClipboardEvent('paste', {{clipboardData: dt, bubbles: true, cancelable: true}});
                                    if (el.dispatchEvent(ev)) {{
                                        document.execCommand('insertText', false, '{}');
                                    }}
                                }}
                            }})()"#,
                            escaped, escaped
                        );
                        entity.read(cx).evaluate_script(&js, cx);
                    }
                }
            } else {
                let js = format!("document.execCommand('{cmd}')");
                entity.read(cx).evaluate_script(&js, cx);
            }
        }
    }

    /// Start the pulsing glow animation loop (~60fps).
    fn start_loading_glow(&mut self, cx: &mut Context<Self>) {
        self.loading_glow_intensity = 1.0;
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let done = cx
                    .update(|cx| -> bool {
                        let Some(entity) = this.upgrade() else { return true };
                        let mut finished = false;
                        entity.update(cx, |wb, cx| {
                            let tab_loading = wb
                                .active_tab_id
                                .and_then(|id| wb.tabs.iter().find(|t| t.id == id))
                                .map(|t| t.loading_progress > 0.0 && t.loading_progress < 1.0)
                                .unwrap_or(false);
                            let active_loading = tab_loading || wb.pending_dot_approve.is_some();
                            if active_loading {
                                // Pulse phase while loading
                                wb.loading_glow_intensity = 1.0;
                                wb.loading_glow_phase += 0.035; // ~3s cycle
                                if wb.loading_glow_phase > std::f32::consts::TAU {
                                    wb.loading_glow_phase -= std::f32::consts::TAU;
                                }
                            } else {
                                // Fade out: decay intensity ~1s
                                wb.loading_glow_intensity -= 0.015;
                                if wb.loading_glow_intensity <= 0.0 {
                                    wb.loading_glow_intensity = 0.0;
                                    wb.loading_glow_phase = 0.0;
                                    wb._loading_glow_task = None;
                                    finished = true;
                                }
                            }
                            cx.notify();
                        });
                        finished
                    })
                    .unwrap_or(true);
                if done { break; }
            }
        });
        self._loading_glow_task = Some(task);
    }

    /// Returns the loading glow border color (pulsing), or `None` if idle.
    fn loading_glow_color(&self) -> Option<gpui::Rgba> {
        if self.loading_glow_intensity <= 0.0 {
            return None;
        }
        // Sine wave: 0.0..1.0 pulsing between dim and bright.
        let t = (self.loading_glow_phase.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        // Pulse between alpha 0.35 and 0.85 — vivid, not gray.
        let alpha = (0.35 + t * 0.50) * self.loading_glow_intensity;
        // GPUI borders don't alpha-blend properly — low alpha renders as
        // dark/black rather than transparent. Cut off while still visibly
        // colored so the glow goes straight from blue to gone.
        if alpha < 0.25 {
            return None;
        }
        // Electric violet: rgb(138, 92, 255)
        Some(gpui::rgba(
            ((138u32) << 24) | ((92u32) << 16) | (255u32 << 8) | ((alpha * 255.0) as u32),
        ))
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
        let fs = is_window_fullscreen();
        let top_row = div()
            .flex()
            .items_center()
            .gap(px(2.0))
            .px(px(8.0))
            .h(px(if is_overlay { 28.0 } else { 38.0 }))
            .flex_shrink_0()
            .when(!fs, |d| d.child(div().w(px(68.0)).flex_shrink_0())) // traffic-light reserve (not in fullscreen)
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

        // ── .dot verification badge ───────────────────────────────────────
        // When active tab is a .dot SPA, show a shield icon in the URL prefix.
        // Falls back to pending DOTNS validation (shown during on-chain resolution).
        let active_dot_verification = self.active_tab_id
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
            .and_then(|t| t.dot_verification.clone())
            .or_else(|| self.pending_dotns_validation.clone());

        let page_info_is_open = self.page_info_open;
        let url_prefix: AnyElement = if let Some(ref dv) = active_dot_verification {
            let (dot_icon, dot_color): (IconName, Rgba) = match &dv.status {
                DotStatus::Verified => (IconName::CircleCheck, rgba(0x22c55eff)),
                DotStatus::Pending => (IconName::LoaderCircle, rgba(0xf59e0bff)),
                DotStatus::Failed(_) => (IconName::CircleX, rgba(0xef4444ff)),
                DotStatus::Local => (IconName::Info, rgba(0x8b8b8bff)),
            };
            let tip = match &dv.status {
                DotStatus::Verified => "Verified app — click for details",
                DotStatus::Pending => "Verifying app...",
                DotStatus::Failed(_) => "Verification failed — click for details",
                DotStatus::Local => "Local app — click for details",
            };
            let glow_bg = rgba(
                ((dot_color.r * 255.0) as u32) << 24
                    | ((dot_color.g * 255.0) as u32) << 16
                    | ((dot_color.b * 255.0) as u32) << 8
                    | 0x22,
            );
            div()
                .id("dot-shield")
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(20.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .hover(|d| d.bg(glow_bg))
                .when(page_info_is_open, |d| d.bg(glow_bg))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page_info_open = !this.page_info_open;
                    cx.notify();
                }))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(Icon::new(dot_icon).size(px(13.0)).text_color(dot_color))
                .into_any_element()
        } else if experimental_contexts_on {
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
            let glow_bg = rgba(0xffffff18);
            div()
                .id("globe-badge")
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(20.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .hover(|d| d.bg(glow_bg))
                .when(page_info_is_open, |d| d.bg(glow_bg))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page_info_open = !this.page_info_open;
                    cx.notify();
                }))
                .tooltip(move |window, cx| Tooltip::new("Page info & security").build(window, cx))
                .child(Icon::new(IconName::Globe).size(px(13.0)))
                .into_any_element()
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

        // ── Page-info panel ──────────────────────────────────────────────
        // Replaces the tab list when page_info_open is true. Shows trust
        // info for .dot apps (CID, owner, sandbox) or privacy info for
        // regular websites (HTTPS status, shield stats).

        // Backdrop is now rendered at the root level (Render::render)
        // so it covers the full window, not just the sidebar.

        let active_readerable = self.active_tab_id
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
            .map(|t| t.readerable || t.reader_active)
            .unwrap_or(false);
        let active_reader_on = self.active_tab_id
            .and_then(|id| self.tabs.iter().find(|t| t.id == id))
            .map(|t| t.reader_active)
            .unwrap_or(false);
        // (bookmark / reader actions moved into page-info panel)

        let url_row = div()
            .id("url-bar")
            .flex()
            .items_center()
            .mx(px(8.0))
            .mt(px(4.0))
            .mb(px(10.0))
            .rounded(px(8.0))
            .bg(url_bar_bg)
            .border_1()
            .border_color(rgba(0xffffff22))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, window, _cx| {
                this.url_bar_clicked = true;
                crate::shield::set_url_bar_focused(true);
                if event.click_count >= 3 {
                    window.dispatch_action(Box::new(gpui_component::input::SelectAll), _cx);
                }
            }))
            .child(
                div().flex_1().child(
                    Input::new(&self.url_input)
                        .appearance(false)
                        .small()
                        .prefix(url_prefix)
                        .cleanable(true),
                ),
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
        let dragging_id = self.dragging_tab.as_ref().filter(|d| d.active).map(|d| d.tab_id);
        let drag_accent = rgba(0x8a5cff66_u32); // electric violet, 40% opacity
        let make_tab_row = |tab_id: u64,
                            icon: IconName,
                            favicon_url: Option<String>,
                            title: SharedString,
                            is_active: bool,
                            _pinned: bool,
                            context_color: Option<Rgba>,
                            wallet_connected: bool,
                            audio_active: bool,
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
            let is_dragging = dragging_id == Some(tab_id);
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
                .when(is_dragging, |d| d.opacity(0.5).bg(item_active_bg))
                .when(is_active && !is_dragging, |d| d.bg(item_active_bg))
                .when(!is_active && !is_dragging, |d| d.hover(|d| d.bg(item_hover_bg)))
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
                // Audio indicator — small teal dot when guest is producing audio
                .when(audio_active, |d| {
                    d.child(
                        div()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgba(0x00d4aacc_u32))
                            .flex_shrink_0()
                    )
                })
                // Wallet connected indicator — CircleCheck icon next to close button
                .when(wallet_connected, |d| {
                    d.child(
                        div()
                            .id(SharedString::from(format!("wallet-{tab_id}")))
                            .cursor_pointer()
                            .flex_shrink_0()
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                cx.stop_propagation();
                                this.wallet_popover_open = !this.wallet_popover_open;
                                this.context_picker_open = false;
                                cx.notify();
                            }))
                            .child(
                                Icon::new(IconName::CircleCheck)
                                    .size(px(11.0))
                                    .text_color(rgba(0x44bb66ccu32))
                            )
                    )
                })
                .child(close_btn)
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, ev: &gpui::MouseDownEvent, _window, cx| {
                    // Start potential drag on mouse-down (actual drag activates after threshold).
                    this.dragging_tab = Some(DragState {
                        tab_id,
                        active: false,
                        start_y: ev.position.y.as_f32(),
                        current_y: ev.position.y.as_f32(),
                    });
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    // Only switch tab if we weren't actively dragging.
                    if this.dragging_tab.as_ref().map_or(true, |d| !d.active) {
                        this.switch_tab(tab_id, window, cx);
                    }
                    this.dragging_tab = None;
                }))
                .into_any_element()
        };

        // ── Helper: build tab list with drop indicator ───────────────────
        // When dragging, a 2px accent line is shown above the dragged tab
        // to indicate the drop position.
        let drop_indicator = || -> AnyElement {
            div()
                .h(px(2.0))
                .mx(px(6.0))
                .rounded(px(1.0))
                .bg(drag_accent)
                .into_any_element()
        };

        // ── Pinned tabs ───────────────────────────────────────────────────
        let mut pinned_items: Vec<AnyElement> = Vec::new();
        for t in self.tabs.iter().filter(|t| t.pinned) {
            if dragging_id == Some(t.id) {
                pinned_items.push(drop_indicator());
            }
            let cc = context_color_for(&t.context_id);
            let wc = t.entity.clone().downcast::<WebViewTab>()
                .ok().map(|e| e.read(cx).wallet_connected).unwrap_or(false);
            let aa = t.entity.clone().downcast::<FramebufferAppTab>()
                .ok().map(|e| e.read(cx).audio_active()).unwrap_or(false);
            pinned_items.push(make_tab_row(
                t.id,
                t.icon.clone(),
                t.favicon_url.clone(),
                SharedString::from(t.title.clone()),
                Some(t.id) == self.active_tab_id,
                true,
                cc,
                wc,
                aa,
                cx,
            ));
        }

        // ── Regular (non-pinned) tabs ─────────────────────────────────────
        let mut regular_items: Vec<AnyElement> = Vec::new();
        for t in self.tabs.iter().filter(|t| !t.pinned) {
            if dragging_id == Some(t.id) {
                regular_items.push(drop_indicator());
            }
            let cc = context_color_for(&t.context_id);
            let wc = t.entity.clone().downcast::<WebViewTab>()
                .ok().map(|e| e.read(cx).wallet_connected).unwrap_or(false);
            let aa = t.entity.clone().downcast::<FramebufferAppTab>()
                .ok().map(|e| e.read(cx).audio_active()).unwrap_or(false);
            regular_items.push(make_tab_row(
                t.id,
                t.icon.clone(),
                t.favicon_url.clone(),
                SharedString::from(t.title.clone()),
                Some(t.id) == self.active_tab_id,
                false,
                cc,
                wc,
                aa,
                cx,
            ));
        }

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
            .id("tabs-area")
            .flex_1()
            .overflow_y_hidden()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(4.0))
            .pt(px(4.0))
            // Drag tracking: mouse move updates drag position + reorders.
            .on_mouse_move(cx.listener(move |this, ev: &gpui::MouseMoveEvent, _window, cx| {
                let Some(ref mut drag) = this.dragging_tab else { return; };
                let y = ev.position.y.as_f32();
                let delta = (y - drag.start_y).abs();
                // Activate drag after 4px threshold to avoid accidental drags.
                if !drag.active && delta > 4.0 {
                    drag.active = true;
                }
                if !drag.active { return; }
                drag.current_y = y;

                // Determine target index based on Y position within the tab list.
                // Each tab row is 28px + 2px gap = 30px. Find which slot we're over.
                let tab_id = drag.tab_id;
                let is_pinned = this.tabs.iter().find(|t| t.id == tab_id).map_or(false, |t| t.pinned);

                // Get indices of tabs in the same group.
                let group_indices: Vec<usize> = this.tabs.iter().enumerate()
                    .filter(|(_, t)| t.pinned == is_pinned)
                    .map(|(i, _)| i)
                    .collect();

                if group_indices.is_empty() { return; }

                let src_idx = match this.tabs.iter().position(|t| t.id == tab_id) {
                    Some(i) => i,
                    None => return,
                };

                // Relative Y from drag start determines direction + distance.
                let row_h: f32 = 30.0; // 28px row + 2px gap
                let offset_rows = ((y - drag.start_y) / row_h).round() as i32;
                if offset_rows == 0 { return; }

                let target_idx = (src_idx as i32 + offset_rows).max(0) as usize;
                // Clamp within the same pinned/regular group.
                let group_min = *group_indices.first().unwrap();
                let group_max = *group_indices.last().unwrap();
                let clamped = target_idx.clamp(group_min, group_max);

                if clamped != src_idx {
                    this.reorder_tab(tab_id, clamped);
                    // Update start_y so further moves are relative to new position.
                    if let Some(ref mut d) = this.dragging_tab {
                        d.start_y = y;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                if this.dragging_tab.take().is_some() {
                    cx.notify();
                }
            }))
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

        // ── Page-info panel (replaces tab list when open) ────────────────
        let page_info_panel: Option<AnyElement> = if self.page_info_open {
            // Gather active tab info
            let active_tab = self.active_tab_id
                .and_then(|id| self.tabs.iter().find(|t| t.id == id));

            let label_style = |label: &str| -> AnyElement {
                div()
                    .w(px(56.0))
                    .text_xs()
                    .text_color(text_muted)
                    .flex_shrink_0()
                    .child(label.to_string())
                    .into_any_element()
            };
            let info_row = |label: &str, value: String, color: Rgba| -> AnyElement {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(3.0))
                    .child(label_style(label))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(color)
                            .truncate()
                            .child(value),
                    )
                    .into_any_element()
            };
            let section_header = |label: &str| -> AnyElement {
                div()
                    .px(px(12.0))
                    .pt(px(10.0))
                    .pb(px(4.0))
                    .text_xs()
                    .text_color(text_muted)
                    .child(label.to_string())
                    .into_any_element()
            };
            let separator = || -> AnyElement {
                div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(rgba(0xffffff14)).into_any_element()
            };
            let perm_row = |icon: IconName, color: Rgba, label: &str, value: &str, value_color: Rgba| -> AnyElement {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(3.0))
                    .child(Icon::new(icon).size(px(11.0)).text_color(color))
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(rgba(0xffffffaa))
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(value_color)
                            .child(value.to_string()),
                    )
                    .into_any_element()
            };

            let green = rgba(0x22c55eff);
            let red = rgba(0xef4444ff);
            let amber = rgba(0xf59e0bff);

            let mut panel = div()
                .id("page-info-panel")
                .flex_1()
                .overflow_y_hidden()
                .flex()
                .flex_col()
                .px(px(4.0))
                .pt(px(4.0))
                // "Back to tabs" row
                .child(
                    div()
                        .id("back-to-tabs")
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(item_hover_bg))
                        .child(Icon::new(IconName::ArrowLeft).size(px(12.0)).text_color(text_muted))
                        .child(div().text_sm().text_color(text_muted).child("Back to tabs"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.page_info_open = false;
                            cx.notify();
                        })),
                );

            // ── .dot app info ──────────────────────────────────────────
            if let Some(ref dv) = active_dot_verification {
                let (header_icon, header_color): (IconName, Rgba) = match &dv.status {
                    DotStatus::Verified => (IconName::CircleCheck, green),
                    DotStatus::Pending => (IconName::LoaderCircle, amber),
                    DotStatus::Failed(_) => (IconName::CircleX, red),
                    DotStatus::Local => (IconName::Info, rgba(0x8b8b8bff)),
                };
                let status_text = match &dv.status {
                    DotStatus::Verified => "Verified on-chain",
                    DotStatus::Pending => "Validating...",
                    DotStatus::Failed(_) => "Resolution failed",
                    DotStatus::Local => "Loaded locally",
                };
                let status_color: Rgba = header_color;
                let is_pending = matches!(dv.status, DotStatus::Pending);

                let cid_display = dv.cid.as_deref().map(|c| {
                    if c.len() > 20 { format!("{}...{}", &c[..10], &c[c.len()-6..]) } else { c.to_string() }
                });
                let owner_display = dv.owner.as_deref().map(|o| {
                    if o.len() > 20 { format!("{}...{}", &o[..8], &o[o.len()-6..]) } else { o.to_string() }
                });

                // Trust header
                panel = panel
                    .child(separator())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .pt(px(6.0))
                            .pb(px(2.0))
                            .child(Icon::new(header_icon).size(px(15.0)).text_color(header_color))
                            .child(
                                div().flex().flex_col()
                                    .child(
                                        div().flex()
                                            .child(div().text_sm().text_color(rgba(0xffffffee)).child(dv.name.clone()))
                                            .child(div().text_sm().text_color(text_muted).child(".dot"))
                                    )
                                    .child(
                                        div().text_xs().text_color(status_color).child(status_text)
                                    )
                            )
                    );

                // Identity section
                panel = panel
                    .child(section_header("IDENTITY"))
                    .child(info_row("CID", cid_display.unwrap_or_else(|| if is_pending { "Resolving...".into() } else { "—".into() }), rgba(0xffffffcc)))
                    .child(info_row("OWNER", owner_display.unwrap_or_else(|| if is_pending { "Resolving...".into() } else { "Unknown".into() }), rgba(0xffffffcc)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .px(px(12.0))
                            .py(px(3.0))
                            .child(label_style("HONOR"))
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(rgba(0xffffff14))
                                    .text_xs()
                                    .text_color(text_muted)
                                    .child("Unknown"),
                            )
                            .into_any_element()
                    );

                // Sandbox section — read actual permissions from the active SPA tab.
                let has_chain = active_tab
                    .and_then(|t| t.entity.clone().downcast::<SpaTab>().ok())
                    .map(|e| e.read(cx).has_permission_chain())
                    .unwrap_or(false);
                let has_sign = active_tab
                    .and_then(|t| t.entity.clone().downcast::<SpaTab>().ok())
                    .map(|e| e.read(cx).has_permission_sign())
                    .unwrap_or(false);
                panel = panel
                    .child(section_header("SANDBOX"))
                    .child(perm_row(
                        if has_chain { IconName::Check } else { IconName::CircleX },
                        if has_chain { green } else { red },
                        "Chain (WSS)",
                        if has_chain { "Allowed" } else { "Blocked" },
                        if has_chain { green } else { red },
                    ))
                    .child(perm_row(IconName::CircleX, red, "HTTP / Fetch", "Blocked", red))
                    .child(perm_row(
                        if has_sign { IconName::Check } else { IconName::CircleX },
                        if has_sign { amber } else { red },
                        "Wallet signing",
                        if has_sign { "With approval" } else { "Blocked" },
                        if has_sign { amber } else { red },
                    ))
                    .child(perm_row(IconName::CircleX, red, "iFrames", "Blocked", red));
            } else if let Some(tab) = active_tab {
                // ── Regular web page info ──────────────────────────────
                let (url_str, is_https) = match &tab.kind {
                    TabKind::WebView { url } => (url.clone(), url.starts_with("https://")),
                    _ => (String::new(), false),
                };
                let hostname = if !url_str.is_empty() {
                    hostname_from_url(&url_str).to_string()
                } else {
                    tab.title.clone()
                };

                // Connection header
                let (conn_icon, conn_color, conn_text) = if is_https {
                    (IconName::CircleCheck, green, "Secure connection (HTTPS)")
                } else if url_str.starts_with("http://") {
                    (IconName::TriangleAlert, amber, "Not secure (HTTP)")
                } else {
                    (IconName::Info, rgba(0x8b8b8bff), "Local page")
                };

                panel = panel
                    .child(separator())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(12.0))
                            .pt(px(6.0))
                            .pb(px(2.0))
                            .child(Icon::new(conn_icon).size(px(15.0)).text_color(conn_color))
                            .child(
                                div().flex().flex_col()
                                    .child(div().text_sm().text_color(rgba(0xffffffee)).truncate().child(hostname))
                                    .child(div().text_xs().text_color(conn_color).child(conn_text))
                            )
                    );

                // Shield / privacy section
                let blocked = active_tab.and_then(|t| {
                    t.entity.clone().downcast::<WebViewTab>().ok()
                }).map(|e| e.read(cx).blocked_count).unwrap_or(0);

                panel = panel
                    .child(section_header("PRIVACY"))
                    .child(info_row("Trackers", format!("{blocked} blocked"), if blocked > 0 { green } else { text_muted }))
                    .child(info_row("Cookies", if self.isolated_tabs { "Isolated".into() } else { "Shared".into() }, rgba(0xffffffcc)));

                // Actions section
                if matches!(tab.kind, TabKind::WebView { .. }) {
                    let is_bookmarked = if let TabKind::WebView { ref url } = tab.kind {
                        crate::bookmarks::is_bookmarked(url)
                    } else {
                        false
                    };
                    panel = panel
                        .child(section_header("ACTIONS"))
                        .child(
                            div()
                                .id("pi-bookmark")
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(0xffffff14)))
                                .child(
                                    Icon::new(if is_bookmarked { IconName::Star } else { IconName::StarOff })
                                        .size(px(12.0))
                                        .text_color(if is_bookmarked { amber } else { text_muted })
                                )
                                .child(div().text_xs().text_color(rgba(0xffffffcc)).child(
                                    if is_bookmarked { "Remove bookmark" } else { "Add bookmark" }
                                ))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_bookmark(window, cx);
                                    cx.notify();
                                }))
                        );

                    if active_readerable {
                        panel = panel.child(
                            div()
                                .id("pi-reader")
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(12.0))
                                .py(px(5.0))
                                .rounded(px(4.0))
                                .cursor_pointer()
                                .hover(|d| d.bg(rgba(0xffffff14)))
                                .child(
                                    Icon::new(IconName::BookOpen)
                                        .size(px(12.0))
                                        .text_color(if active_reader_on { rgba(0x8b5cf6ff) } else { text_muted })
                                )
                                .child(div().text_xs().text_color(rgba(0xffffffcc)).child(
                                    if active_reader_on { "Exit reader mode" } else { "Reader mode" }
                                ))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.toggle_reader_mode(cx);
                                    cx.notify();
                                }))
                        );
                    }
                }
            }

            Some(panel.into_any_element())
        } else {
            None
        };

        // Approval panel replaces the tab list when a pending action requires user consent.
        let has_pending = self.has_pending_approval();
        let approval_panel = if has_pending && !self.page_info_open {
            self.render_approval_panel(_window, cx)
        } else {
            None
        };

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
            // Wallet connection consent banner — between URL bar and content area
            .children(self.render_wallet_connect_banner(_window, cx).map(|b| b.into_any_element()))
            // Either page-info panel, approval panel, or tab list
            .when_some(page_info_panel, |d, panel| d.child(panel))
            .when_some(approval_panel, |d, panel| d.child(panel))
            .when(!self.page_info_open && !has_pending, |d| d.child(tabs_area))
            .child(bottom_bar)
            // Context picker dropdown — painted last so it sits on top of tabs
            .children(context_dropdown)
            // Wallet popover — painted on top of everything in the sidebar
            .children(self.render_wallet_popover(_window, cx).map(|p| p.into_any_element()))
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

        // Build history suggestion rows (cached from on_omnibox_input_event)
        let history_rows: Vec<AnyElement> = self
            .omnibox_history_results
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let url = entry.url.clone();
                let title = entry.title.clone();
                let display_title = if title.is_empty() {
                    url.clone()
                } else {
                    title.clone()
                };
                let display_url = url.clone();
                let nav_url = url.clone();
                div()
                    .id(ElementId::Integer((i as u64) + 2_000_000))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(16.0))
                    .h(px(46.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(0xffffff0d)))
                    .child(Icon::new(IconName::Globe).size(px(14.0)).text_color(rgba(0xffffffaa)))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .overflow_hidden()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(rgba(0xffffffdd))
                                    .truncate()
                                    .child(display_title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgba(0xffffff55))
                                    .truncate()
                                    .child(display_url),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.omnibox_pending_nav = Some(nav_url.clone());
                        this.close_omnibox(cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let has_history = !history_rows.is_empty();
        let has_any_results = has_tabs || has_history;

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
                    .when(has_history, |d| {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .px(px(16.0))
                                .h(px(24.0))
                                .text_xs()
                                .text_color(rgba(0xffffff44))
                                .when(has_tabs, |d| {
                                    d.border_t_1().border_color(rgba(0xffffff14))
                                })
                                .child("History"),
                        )
                        .children(history_rows)
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .px(px(16.0))
                            .h(px(38.0))
                            .text_xs()
                            .text_color(rgba(0xffffff55))
                            .when(has_any_results, |d| {
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

// ---------------------------------------------------------------------------
// Wallet connection consent + popover + indicator
// ---------------------------------------------------------------------------
impl Workbench {
    fn resolve_wallet_enable(&mut self, webview_ptr: usize, id: u64, origin: &str, cx: &mut Context<Self>) {
        self.connected_sites.insert(origin.to_string());
        // Mark the WebViewTab as connected
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                if entity.read(cx).webview_ptr == webview_ptr {
                    entity.update(cx, |wv, _| wv.wallet_connected = true);
                    break;
                }
            }
        }
        // Resolve the JS promise with accounts
        if cx.has_global::<crate::wallet::WalletGlobal>() {
            let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
            if let epoca_wallet::WalletState::Unlocked { ref root_address } = wg.manager.state() {
                let addr = root_address.clone();
                let js = format!(
                    "window.__epocaWalletResolve({}, null, {{accounts: [{{address: '{}', name: 'Epoca'}}]}})",
                    id, addr,
                );
                self.evaluate_on_webview(webview_ptr, &js, cx);
                return;
            }
        }
        let js = format!(
            "window.__epocaWalletResolve({}, 'wallet is not available', null)", id,
        );
        self.evaluate_on_webview(webview_ptr, &js, cx);
    }

    fn approve_wallet_connect(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_wallet_connect.take() else { return };
        match req.channel {
            WalletChannel::Polkadot => {
                self.resolve_wallet_enable(req.webview_ptr, req.id, &req.origin, cx);
            }
            WalletChannel::Btc => {
                self.resolve_btc_accounts(req.webview_ptr, req.id, &req.origin, cx);
            }
        }
        cx.notify();
    }

    fn deny_wallet_connect(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_wallet_connect.take() else { return };
        let resolver = match req.channel {
            WalletChannel::Polkadot => "__epocaWalletResolve",
            WalletChannel::Btc => "__epocaBtcResolve",
        };
        let js = format!(
            "window.{}({}, 'user rejected the request', null)", resolver, req.id,
        );
        self.evaluate_on_webview(req.webview_ptr, &js, cx);
        cx.notify();
    }

    fn disconnect_wallet_site(&mut self, hostname: &str, cx: &mut Context<Self>) {
        self.connected_sites.remove(hostname);
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                let tab_host = hostname_from_url(entity.read(cx).url()).to_string();
                if tab_host == hostname {
                    entity.update(cx, |wv, _| wv.wallet_connected = false);
                }
            }
        }
    }

    fn resolve_btc_accounts(&mut self, webview_ptr: usize, id: u64, origin: &str, cx: &mut Context<Self>) {
        self.connected_sites.insert(origin.to_string());
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                if entity.read(cx).webview_ptr == webview_ptr {
                    entity.update(cx, |wv, _| wv.wallet_connected = true);
                    break;
                }
            }
        }
        if cx.has_global::<crate::wallet::WalletGlobal>() {
            let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
            if let Ok(addr) = wg.manager.btc_address() {
                let js = format!(
                    "window.__epocaBtcResolve({}, null, ['{}'])", id, addr,
                );
                self.evaluate_on_webview(webview_ptr, &js, cx);
                return;
            }
        }
        let js = format!(
            "window.__epocaBtcResolve({}, 'wallet is not available', null)", id,
        );
        self.evaluate_on_webview(webview_ptr, &js, cx);
    }

    fn approve_btc_wallet_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_btc_wallet_sign.take() else { return };
        if !cx.has_global::<crate::wallet::WalletGlobal>() {
            let js = format!("window.__epocaBtcResolve({}, 'no wallet', null)", req.id);
            self.evaluate_on_webview(req.webview_ptr, &js, cx);
            cx.notify();
            return;
        }
        let result = cx
            .global_mut::<crate::wallet::WalletGlobal>()
            .manager
            .btc_sign_message(req.message.as_bytes());
        match result {
            Ok(b64) => {
                let js = format!("window.__epocaBtcResolve({}, null, '{}')", req.id, b64);
                self.evaluate_on_webview(req.webview_ptr, &js, cx);
            }
            Err(e) => {
                let msg = e.to_string().replace('\'', "\\'");
                let js = format!("window.__epocaBtcResolve({}, '{}', null)", req.id, msg);
                self.evaluate_on_webview(req.webview_ptr, &js, cx);
            }
        }
        cx.notify();
    }

    fn deny_btc_wallet_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_btc_wallet_sign.take() else { return };
        let js = format!(
            "window.__epocaBtcResolve({}, 'user rejected signing request', null)", req.id,
        );
        self.evaluate_on_webview(req.webview_ptr, &js, cx);
        cx.notify();
    }

    fn lock_wallet(&mut self, cx: &mut Context<Self>) {
        if cx.has_global::<crate::wallet::WalletGlobal>() {
            cx.global_mut::<crate::wallet::WalletGlobal>().manager.lock();
        }
        self.connected_sites.clear();
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                entity.update(cx, |wv, _| wv.wallet_connected = false);
            }
        }
        self.wallet_popover_open = false;
        cx.notify();
    }

    fn render_wallet_connect_banner(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_wallet_connect.as_ref()?;
        let origin = req.origin.clone();

        Some(
            div()
                .mx(px(8.0))
                .mb(px(4.0))
                .p(px(10.0))
                .rounded(px(8.0))
                .bg(rgba(0x1e1e1eff))
                .border_1()
                .border_color(rgba(0xffffff22u32))
                .flex()
                .flex_col()
                .gap(px(8.0))
                // Title
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            Icon::new(IconName::CircleUser)
                                .size(px(14.0))
                                .text_color(rgba(0x44bb66ffu32)),
                        )
                        .child(
                            gpui_component::label::Label::new("Connect wallet?")
                                .text_size(px(13.0)),
                        ),
                )
                // Origin pill
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(4.0))
                        .bg(rgba(0xffffff0cu32))
                        .child(
                            gpui_component::label::Label::new(origin)
                                .text_size(px(11.0))
                                .text_color(rgba(0xffffff99u32)),
                        ),
                )
                // Buttons
                .child(
                    div()
                        .flex()
                        .gap(px(6.0))
                        .justify_end()
                        .child(
                            Button::new("wallet-connect-deny")
                                .ghost()
                                .compact()
                                .label("Deny")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.deny_wallet_connect(cx);
                                })),
                        )
                        .child(
                            Button::new("wallet-connect-allow")
                                .primary()
                                .compact()
                                .label("Allow")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.approve_wallet_connect(cx);
                                })),
                        ),
                ),
        )
    }

    fn render_wallet_popover(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if !self.wallet_popover_open { return None; }

        let wallet_state = cx
            .try_global::<crate::wallet::WalletGlobal>()
            .map(|g| g.manager.state())
            .unwrap_or(epoca_wallet::WalletState::NoWallet);

        // Get active tab's connection status and hostname
        let (tab_connected, tab_hostname) = if let Some(id) = self.active_tab_id {
            self.tabs.iter().find(|t| t.id == id).and_then(|tab| {
                tab.entity.clone().downcast::<WebViewTab>().ok().map(|e| {
                    let wv = e.read(cx);
                    (wv.wallet_connected, hostname_from_url(wv.url()).to_string())
                })
            }).unwrap_or((false, String::new()))
        } else {
            (false, String::new())
        };

        let content: gpui::AnyElement = match wallet_state {
            epoca_wallet::WalletState::NoWallet => {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        gpui_component::label::Label::new("No wallet configured")
                            .text_size(px(12.0))
                            .text_color(rgba(0xffffff66u32)),
                    )
                    .child(
                        Button::new("wallet-pop-settings")
                            .compact()
                            .label("Set up in Settings")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.wallet_popover_open = false;
                                this.open_settings(window, cx);
                            })),
                    )
                    .into_any_element()
            }
            epoca_wallet::WalletState::Locked => {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        gpui_component::label::Label::new("Wallet is locked")
                            .text_size(px(12.0))
                            .text_color(rgba(0xffffff66u32)),
                    )
                    .child(
                        Button::new("wallet-pop-unlock")
                            .primary()
                            .compact()
                            .label("Unlock")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if cx.has_global::<crate::wallet::WalletGlobal>() {
                                    let _ = cx.global_mut::<crate::wallet::WalletGlobal>().manager.unlock();
                                }
                                this.wallet_popover_open = false;
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            }
            epoca_wallet::WalletState::Unlocked { ref root_address } => {
                let addr = root_address.clone();
                let addr_display = if addr.len() > 16 {
                    format!("{}...{}", &addr[..8], &addr[addr.len()-6..])
                } else {
                    addr.clone()
                };
                let addr_for_copy = addr.clone();
                let hostname = tab_hostname.clone();

                let mut col = div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    // Address row
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                gpui_component::label::Label::new("Account")
                                    .text_size(px(10.0))
                                    .text_color(rgba(0xffffff66u32)),
                            )
                            .child(
                                div()
                                    .id("wallet-addr-copy")
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .cursor_pointer()
                                    .child(
                                        gpui_component::label::Label::new(addr_display)
                                            .text_size(px(12.0))
                                            .text_color(rgba(0x44bb66ffu32)),
                                    )
                                    .child(
                                        Icon::new(IconName::Copy)
                                            .size(px(12.0))
                                            .text_color(rgba(0xffffff44u32)),
                                    )
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(addr_for_copy.clone()));
                                    })),
                            ),
                    )
                    // Separator
                    .child(div().h(px(1.0)).bg(rgba(0xffffff14u32)));

                // Connection status
                if tab_connected {
                    let disconnect_host = hostname.clone();
                    col = col
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(Icon::new(IconName::Check).size(px(12.0)).text_color(rgba(0x44bb66ffu32)))
                                .child(
                                    gpui_component::label::Label::new(format!("Connected to {}", hostname))
                                        .text_size(px(11.0)),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .child(
                                    Button::new("wallet-pop-disconnect")
                                        .ghost()
                                        .compact()
                                        .label("Disconnect")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.disconnect_wallet_site(&disconnect_host, cx);
                                            this.wallet_popover_open = false;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    Button::new("wallet-pop-lock")
                                        .ghost()
                                        .compact()
                                        .label("Lock")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.lock_wallet(cx);
                                        })),
                                ),
                        );
                } else {
                    col = col
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(Icon::new(IconName::Info).size(px(12.0)).text_color(rgba(0xffffff44u32)))
                                .child(
                                    gpui_component::label::Label::new("Not connected to this site")
                                        .text_size(px(11.0))
                                        .text_color(rgba(0xffffff66u32)),
                                ),
                        )
                        .child(
                            Button::new("wallet-pop-lock2")
                                .ghost()
                                .compact()
                                .label("Lock Wallet")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.lock_wallet(cx);
                                })),
                        );
                }

                col.into_any_element()
            }
        };

        Some(
            div()
                .absolute()
                .top(px(76.0))
                .left(px(8.0))
                .right(px(8.0))
                .p(px(12.0))
                .rounded(px(8.0))
                .bg(rgba(0x1e1e1eff))
                .border_1()
                .border_color(rgba(0xffffff22u32))
                .child(content)
        )
    }
}

// ---------------------------------------------------------------------------
// Unified approval panel (replaces the 6 separate backdrop dialogs)
// ---------------------------------------------------------------------------
impl Workbench {
    fn has_pending_approval(&self) -> bool {
        self.pending_wallet_sign.is_some()
            || self.pending_btc_wallet_sign.is_some()
            || self.pending_spa_sign.is_some()
            || self.pending_chain_submit.is_some()
            || self.pending_data_connect.is_some()
            || self.pending_hostapi_sign.is_some()
            || self.pending_wss_permission.is_some()
            || self.pending_http_permission.is_some()
    }

    fn is_pending_high_risk(&self) -> bool {
        self.pending_wallet_sign.is_some()
            || self.pending_btc_wallet_sign.is_some()
            || self.pending_spa_sign.is_some()
            || self.pending_hostapi_sign.is_some()
            || self.pending_chain_submit.is_some()
        // data_connect and wss_permission are low-risk
    }

    /// Renders the approval panel inside the sidebar (replaces the tab list).
    /// A 3px accent strip on the panel's left edge signals risk level.
    fn render_approval_panel(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let amber = rgba(0xf5a623ff_u32);
        let teal = rgba(0x00d4aaff_u32);
        let text_primary = rgba(0xffffffe0_u32);
        let text_secondary = rgba(0xffffff66_u32);
        let text_muted = rgba(0xffffff44_u32);
        let payload_bg = rgba(0x00000033_u32);
        let deny_bg = rgba(0xffffff0f_u32);

        // Determine which approval to show and extract display data.
        enum ApprovalKind {
            WalletSign,
            BtcSign,
            SpaSign,
            HostApiSign,
            ChainSubmit,
            DataConnect,
            WssPermission,
            HttpPermission,
        }

        let kind;
        let title: &str;
        let app_label: String;
        let detail_rows: Vec<(&str, String)>;
        let payload_text: String;
        let note: &str;
        let approve_label: &str;
        let is_high_risk: bool;

        if let Some(req) = &self.pending_wallet_sign {
            kind = ApprovalKind::WalletSign;
            title = "Signature Request";
            app_label = req.origin.clone();
            detail_rows = vec![("Method", req.method.clone())];
            payload_text = req.display_message.clone();
            note = "This website is requesting a cryptographic signature.";
            approve_label = "Sign";
            is_high_risk = true;
        } else if let Some(req) = &self.pending_btc_wallet_sign {
            kind = ApprovalKind::BtcSign;
            title = "Bitcoin Sign Message";
            app_label = req.origin.clone();
            detail_rows = vec![];
            payload_text = if req.message.chars().count() > 200 {
                let t: String = req.message.chars().take(200).collect();
                format!("{t}…")
            } else {
                req.message.clone()
            };
            note = "This website is requesting a Bitcoin message signature.";
            approve_label = "Sign";
            is_high_risk = true;
        } else if let Some(req) = &self.pending_spa_sign {
            kind = ApprovalKind::SpaSign;
            title = "Signature Request";
            app_label = req.app_id.clone();
            detail_rows = vec![];
            payload_text = if req.payload.len() > 200 {
                format!("{}…", &req.payload[..200])
            } else {
                req.payload.clone()
            };
            note = "This app is requesting a cryptographic signature.";
            approve_label = "Sign";
            is_high_risk = true;
        } else if let Some(req) = &self.pending_hostapi_sign {
            kind = ApprovalKind::HostApiSign;
            let is_raw = req.request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ;
            title = if is_raw { "Sign Raw Data" } else { "Sign Transaction" };
            app_label = req.app_id.clone();
            detail_rows = vec![];
            payload_text = if req.payload.len() > 60 {
                format!("{}…", hex_encode(&req.payload[..60]))
            } else {
                hex_encode(&req.payload)
            };
            note = "This app is requesting your approval to sign a transaction.";
            approve_label = "Sign";
            is_high_risk = true;
        } else if let Some(req) = &self.pending_chain_submit {
            kind = ApprovalKind::ChainSubmit;
            title = "Transaction Request";
            app_label = req.app_id.clone();
            detail_rows = vec![("Chain", req.chain.display_name().to_string())];
            payload_text = if req.call_data.len() > 200 {
                format!("{}…", &req.call_data[..200])
            } else {
                req.call_data.clone()
            };
            note = "This app wants to submit a transaction to the network.";
            approve_label = "Submit";
            is_high_risk = true;
        } else if let Some(req) = &self.pending_data_connect {
            kind = ApprovalKind::DataConnect;
            title = "Connection Request";
            app_label = req.app_id.clone();
            let peer = &req.peer_address;
            let peer_display = if peer.len() > 20 {
                format!("{}…{}", &peer[..8], &peer[peer.len() - 8..])
            } else {
                peer.clone()
            };
            detail_rows = vec![("Peer", peer_display)];
            payload_text = String::new();
            note = "This app wants to open a peer-to-peer data connection.";
            approve_label = "Allow";
            is_high_risk = false;
        } else if let Some(req) = &self.pending_wss_permission {
            kind = ApprovalKind::WssPermission;
            title = "Chain Access (WSS)";
            app_label = req.app_id.clone();
            let url_display = if req.url.len() > 50 {
                format!("{}…", &req.url[..50])
            } else {
                req.url.clone()
            };
            detail_rows = vec![
                ("Capability", "WebSocket (WSS)".into()),
                ("Endpoint", url_display),
            ];
            payload_text = String::new();
            note = "This app wants to connect to a blockchain node via WebSocket. Granting this will reload the page.";
            approve_label = "Allow";
            is_high_risk = false;
        } else if let Some(req) = &self.pending_http_permission {
            kind = ApprovalKind::HttpPermission;
            title = "Network Access (HTTPS)";
            app_label = req.app_id.clone();
            detail_rows = vec![
                ("Capability", "HTTPS Fetch".into()),
                ("Origin", req.origin.clone()),
            ];
            payload_text = String::new();
            note = "This app wants to make HTTPS requests to an external server. Granting this will reload the page.";
            approve_label = "Allow";
            is_high_risk = false;
        } else {
            return None;
        }

        let accent = if is_high_risk { amber } else { teal };

        let icon_name = match kind {
            ApprovalKind::ChainSubmit => IconName::ExternalLink,
            ApprovalKind::DataConnect | ApprovalKind::WssPermission | ApprovalKind::HttpPermission => IconName::Globe,
            _ => IconName::TriangleAlert,
        };

        // Build detail row elements.
        let detail_els: Vec<AnyElement> = detail_rows
            .into_iter()
            .map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(80.0))
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(text_muted)
                            .child(label.to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_xs()
                            .text_color(text_secondary)
                            .truncate()
                            .child(value),
                    )
                    .into_any_element()
            })
            .collect();

        // ── Sidebar panel: flex_row with accent strip + content ─────────
        let panel = div()
            .id("approval-panel")
            .flex()
            .flex_row()
            .flex_1()
            .overflow_hidden()
            // Accent strip on the left edge of just this panel
            .child(
                div()
                    .w(px(3.0))
                    .flex_shrink_0()
                    .h_full()
                    .bg(accent),
            )
            // Panel content
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .px(px(12.0))
                    .py(px(14.0))
                    .gap(px(10.0))
                    // Header: icon + title + app name
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(Icon::new(icon_name).size(px(16.0)).text_color(accent))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(text_primary)
                                            .child(title.to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(text_muted)
                                            .truncate()
                                            .child(app_label),
                                    ),
                            ),
                    )
                    // Separator
                    .child(
                        div()
                            .h(px(1.0))
                            .w_full()
                            .bg(rgba(0xffffff14_u32)),
                    )
                    // Detail rows
                    .children(detail_els)
                    // Payload box (only when non-empty)
                    .when(!payload_text.is_empty(), |d| {
                        d.child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(4.0))
                                .bg(payload_bg)
                                .max_h(px(120.0))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_family("monospace")
                                        .text_color(text_secondary)
                                        .child(payload_text),
                                ),
                        )
                    })
                    // Security note
                    .child(
                        div()
                            .text_xs()
                            .text_color(text_muted)
                            .child(note.to_string()),
                    )
                    // Spacer to push buttons to bottom
                    .child(div().flex_1())
                    // Button row
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            // Deny button
                            .child(
                                div()
                                    .id("approval-deny-btn")
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(36.0))
                                    .rounded(px(8.0))
                                    .bg(deny_bg)
                                    .cursor_pointer()
                                    .hover(|d| d.bg(rgba(0xffffff1a_u32)))
                                    .text_xs()
                                    .text_color(text_secondary)
                                    .child("Deny")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.pending_wallet_sign.is_some() {
                                            this.deny_wallet_sign(cx);
                                        } else if this.pending_btc_wallet_sign.is_some() {
                                            this.deny_btc_wallet_sign(cx);
                                        } else if this.pending_spa_sign.is_some() {
                                            this.deny_spa_sign(cx);
                                        } else if this.pending_hostapi_sign.is_some() {
                                            this.deny_hostapi_sign(cx);
                                        } else if this.pending_chain_submit.is_some() {
                                            this.deny_chain_submit(cx);
                                        } else if this.pending_data_connect.is_some() {
                                            this.deny_data_connect(cx);
                                        } else if this.pending_wss_permission.is_some() {
                                            this.deny_wss_permission(cx);
                                        } else if this.pending_http_permission.is_some() {
                                            this.deny_http_permission(cx);
                                        }
                                    })),
                            )
                            // Approve button
                            .child(
                                div()
                                    .id("approval-approve-btn")
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .h(px(36.0))
                                    .rounded(px(8.0))
                                    .bg(accent)
                                    .cursor_pointer()
                                    .hover(|d| d.opacity(0.85))
                                    .text_xs()
                                    .font_weight(gpui::FontWeight::BOLD)
                                    .text_color(rgba(0x000000dd_u32))
                                    .child(approve_label.to_string())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.pending_wallet_sign.is_some() {
                                            this.approve_wallet_sign(cx);
                                        } else if this.pending_btc_wallet_sign.is_some() {
                                            this.approve_btc_wallet_sign(cx);
                                        } else if this.pending_spa_sign.is_some() {
                                            this.approve_spa_sign(cx);
                                        } else if this.pending_hostapi_sign.is_some() {
                                            this.approve_hostapi_sign(cx);
                                        } else if this.pending_chain_submit.is_some() {
                                            this.approve_chain_submit(cx);
                                        } else if this.pending_data_connect.is_some() {
                                            this.approve_data_connect(cx);
                                        } else if this.pending_wss_permission.is_some() {
                                            this.approve_wss_permission(window, cx);
                                        } else if this.pending_http_permission.is_some() {
                                            this.approve_http_permission(window, cx);
                                        }
                                    })),
                            ),
                    ),
            );

        Some(panel.into_any_element())
    }
}

// ---------------------------------------------------------------------------
// Wallet sign confirmation dialog
// ---------------------------------------------------------------------------
impl Workbench {
    fn approve_wallet_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_wallet_sign.take() else { return };

        if !cx.has_global::<crate::wallet::WalletGlobal>() {
            self.resolve_wallet_sign_js(req.webview_ptr, req.id, Err("no wallet"), cx);
            return;
        }

        let result = if req.method == "signRaw" {
            // Parse raw.data from params JSON
            self.wallet_sign_raw(&req.params_json, cx)
        } else {
            // signPayload — sign the method (call data) hex bytes
            self.wallet_sign_payload(&req.params_json, cx)
        };

        match result {
            Ok(sig_hex) => {
                // Return { id: 1, signature: "0x01..." } — 0x01 prefix = sr25519 type byte
                let js = format!(
                    "window.__epocaWalletResolve({}, null, {{id: 1, signature: '0x01{}'}})",
                    req.id, sig_hex.strip_prefix("0x").unwrap_or(&sig_hex),
                );
                self.evaluate_on_webview(req.webview_ptr, &js, cx);
            }
            Err(e) => {
                self.resolve_wallet_sign_js(req.webview_ptr, req.id, Err(&e), cx);
            }
        }
        cx.notify();
    }

    fn deny_wallet_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_wallet_sign.take() else { return };
        self.resolve_wallet_sign_js(req.webview_ptr, req.id, Err("user rejected signing request"), cx);
        cx.notify();
    }

    fn approve_spa_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_spa_sign.take() else { return };

        if !cx.has_global::<crate::wallet::WalletGlobal>() {
            self.resolve_spa_js(req.webview_ptr, req.id, Err("no wallet configured"), cx);
            cx.notify();
            return;
        }

        let result = cx
            .global_mut::<crate::wallet::WalletGlobal>()
            .manager
            .sign(&req.app_id, req.payload.as_bytes());

        match result {
            Ok(sig_bytes) => {
                let sig_hex = hex_encode(&sig_bytes);
                let js = format!(
                    "window.__epocaResolve({}, null, '0x{}')",
                    req.id, sig_hex,
                );
                self.evaluate_on_spa(req.webview_ptr, &js, cx);
            }
            Err(e) => {
                let msg = e.to_string().replace('\'', "\\'");
                self.resolve_spa_js(req.webview_ptr, req.id, Err(&msg), cx);
            }
        }
        cx.notify();
    }

    fn deny_spa_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_spa_sign.take() else { return };
        self.resolve_spa_js(req.webview_ptr, req.id, Err("user rejected signing request"), cx);
        cx.notify();
    }

    fn approve_chain_submit(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_chain_submit.take() else { return };

        // Get the wallet's SS58 address for the nonce query.
        let address = if cx.has_global::<crate::wallet::WalletGlobal>() {
            match cx.global::<crate::wallet::WalletGlobal>().manager.root_address() {
                Ok(addr) => addr,
                Err(e) => {
                    self.resolve_spa_js(req.webview_ptr, req.id, Err(&e.to_string()), cx);
                    cx.notify();
                    return;
                }
            }
        } else {
            self.resolve_spa_js(req.webview_ptr, req.id, Err("wallet not available"), cx);
            cx.notify();
            return;
        };

        // Allocate internal correlation IDs from a high sentinel range.
        static INTERNAL_COUNTER: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(0xFFFF_0000_0000_0000);
        let nonce_js_id = INTERNAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let head_js_id = INTERNAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let chain = req.chain;

        // Issue internal queries: nonce + finalized head.
        let _ = crate::chain_api::submit_internal_query(
            chain,
            nonce_js_id,
            "system_accountNextIndex",
            &serde_json::json!([address]),
        );
        let _ = crate::chain_api::submit_internal_query(
            chain,
            head_js_id,
            "chain_getFinalizedHead",
            &serde_json::json!([]),
        );

        // Fire-and-forget cache warmers for runtime version + genesis hash if not cached.
        if crate::chain_api::get_chain_meta(chain).is_none() {
            let rv_js_id = INTERNAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let gh_js_id = INTERNAL_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let _ = crate::chain_api::submit_internal_query(
                chain,
                rv_js_id,
                "state_getRuntimeVersion",
                &serde_json::json!([]),
            );
            let _ = crate::chain_api::submit_internal_query(
                chain,
                gh_js_id,
                "chain_getBlockHash",
                &serde_json::json!([0]), // block 0 = genesis
            );
        }

        // Move into the build state. The approval dialog clears (pending_chain_submit is None).
        self.chain_submit_building = Some(ChainSubmitBuild {
            webview_ptr: req.webview_ptr,
            js_id: req.id,
            chain,
            call_data: req.call_data,
            phase: ChainSubmitPhase::AwaitingChainMeta {
                nonce_js_id,
                head_js_id,
                nonce: None,
                head_hash: None,
            },
        });
        cx.notify();
    }

    fn deny_chain_submit(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_chain_submit.take() else { return };
        self.resolve_spa_js(req.webview_ptr, req.id, Err("user rejected transaction"), cx);
        cx.notify();
    }

    /// Handle an internal chain RPC response (webview_ptr == 0) and advance
    /// the extrinsic construction state machine in `chain_submit_building`.
    fn handle_internal_chain_response(
        &mut self,
        js_id: u64,
        result: Result<serde_json::Value, String>,
        cx: &mut Context<Self>,
    ) {
        let Some(ref mut build) = self.chain_submit_building else {
            // Could be a stale response after the build was cancelled.
            return;
        };

        match &mut build.phase {
            ChainSubmitPhase::AwaitingChainMeta {
                nonce_js_id,
                head_js_id,
                nonce,
                head_hash,
            } => {
                if js_id == *nonce_js_id {
                    match result {
                        Ok(val) => {
                            *nonce = val.as_u64();
                            if nonce.is_none() {
                                log::warn!("[extrinsic] nonce response was not a number: {val}");
                            }
                        }
                        Err(e) => {
                            log::error!("[extrinsic] nonce query failed: {e}");
                            let b = self.chain_submit_building.take().unwrap();
                            self.resolve_spa_js(
                                b.webview_ptr,
                                b.js_id,
                                Err(&format!("nonce query failed: {e}")),
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                    }
                } else if js_id == *head_js_id {
                    match result {
                        Ok(val) => {
                            if let Some(hash_str) = val.as_str() {
                                let hex = hash_str.strip_prefix("0x").unwrap_or(hash_str);
                                if let Ok(bytes) = hex_decode(hex) {
                                    if bytes.len() == 32 {
                                        let mut arr = [0u8; 32];
                                        arr.copy_from_slice(&bytes);
                                        *head_hash = Some(arr);
                                    }
                                }
                            }
                            if head_hash.is_none() {
                                log::warn!("[extrinsic] finalized head not a 32-byte hash: {val}");
                            }
                        }
                        Err(e) => {
                            log::error!("[extrinsic] finalized head query failed: {e}");
                            let b = self.chain_submit_building.take().unwrap();
                            self.resolve_spa_js(
                                b.webview_ptr,
                                b.js_id,
                                Err(&format!("chain query failed: {e}")),
                                cx,
                            );
                            cx.notify();
                            return;
                        }
                    }
                } else {
                    // Cache-warmer response (runtime version or genesis hash).
                    if let Ok(ref val) = result {
                        if let (Some(sv), Some(tv)) = (
                            val.get("specVersion").and_then(|v| v.as_u64()),
                            val.get("transactionVersion").and_then(|v| v.as_u64()),
                        ) {
                            // Runtime version — stash until genesis hash arrives.
                            PENDING_SPEC_VERSION.store(sv as u32, std::sync::atomic::Ordering::Relaxed);
                            PENDING_TX_VERSION.store(tv as u32, std::sync::atomic::Ordering::Relaxed);
                            log::debug!("[extrinsic] cached pending spec={sv} tx={tv}");
                        } else if let Some(hash_str) = val.as_str() {
                            // Genesis hash from chain_getBlockHash(0).
                            let hex = hash_str.strip_prefix("0x").unwrap_or(hash_str);
                            if let Ok(bytes) = hex_decode(hex) {
                                if bytes.len() == 32 {
                                    let chain = build.chain;
                                    let sv = PENDING_SPEC_VERSION.load(std::sync::atomic::Ordering::Relaxed);
                                    let tv = PENDING_TX_VERSION.load(std::sync::atomic::Ordering::Relaxed);
                                    if sv > 0 && tv > 0 {
                                        let mut genesis = [0u8; 32];
                                        genesis.copy_from_slice(&bytes);
                                        crate::chain_api::cache_chain_meta(
                                            chain,
                                            crate::chain_api::ChainMetaCache {
                                                spec_version: sv,
                                                tx_version: tv,
                                                genesis_hash: genesis,
                                            },
                                        );
                                        log::info!("[extrinsic] cached chain meta: spec={sv} tx={tv}");
                                    }
                                }
                            }
                        }
                    }
                    return;
                }

                // Re-borrow to read the updated values (we may have just set one of them).
                let (nonce_val, head_val) = if let Some(ChainSubmitPhase::AwaitingChainMeta {
                    nonce, head_hash, ..
                }) = self.chain_submit_building.as_ref().map(|b| &b.phase)
                {
                    (*nonce, *head_hash)
                } else {
                    return;
                };

                // Advance to AwaitingBlockHeader once both are ready.
                if let (Some(n), Some(hh)) = (nonce_val, head_val) {
                    static INTERNAL_COUNTER2: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0xFFFF_8000_0000_0000);
                    let header_js_id =
                        INTERNAL_COUNTER2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let head_hex = format!("0x{}", hex_encode(&hh));
                    let chain = self.chain_submit_building.as_ref().unwrap().chain;
                    let _ = crate::chain_api::submit_internal_query(
                        chain,
                        header_js_id,
                        "chain_getHeader",
                        &serde_json::json!([head_hex]),
                    );
                    self.chain_submit_building.as_mut().unwrap().phase =
                        ChainSubmitPhase::AwaitingBlockHeader {
                            nonce: n,
                            head_hash: hh,
                            header_js_id,
                        };
                    log::info!("[extrinsic] nonce={n} head_hash={head_hex}, querying header");
                }
            }

            ChainSubmitPhase::AwaitingBlockHeader {
                nonce,
                head_hash,
                header_js_id,
            } => {
                if js_id != *header_js_id {
                    // Could be a late cache-warmer response — apply caching and ignore.
                    if let Ok(ref val) = result {
                        if let (Some(sv), Some(tv)) = (
                            val.get("specVersion").and_then(|v| v.as_u64()),
                            val.get("transactionVersion").and_then(|v| v.as_u64()),
                        ) {
                            PENDING_SPEC_VERSION.store(sv as u32, std::sync::atomic::Ordering::Relaxed);
                            PENDING_TX_VERSION.store(tv as u32, std::sync::atomic::Ordering::Relaxed);
                        } else if let Some(hash_str) = val.as_str() {
                            let hex = hash_str.strip_prefix("0x").unwrap_or(hash_str);
                            if let Ok(bytes) = hex_decode(hex) {
                                if bytes.len() == 32 {
                                    let chain = build.chain;
                                    let sv = PENDING_SPEC_VERSION.load(std::sync::atomic::Ordering::Relaxed);
                                    let tv = PENDING_TX_VERSION.load(std::sync::atomic::Ordering::Relaxed);
                                    if sv > 0 && tv > 0 {
                                        let mut genesis = [0u8; 32];
                                        genesis.copy_from_slice(&bytes);
                                        crate::chain_api::cache_chain_meta(
                                            chain,
                                            crate::chain_api::ChainMetaCache {
                                                spec_version: sv,
                                                tx_version: tv,
                                                genesis_hash: genesis,
                                            },
                                        );
                                        log::info!("[extrinsic] late-cached chain meta: spec={sv} tx={tv}");
                                    }
                                }
                            }
                        }
                    }
                    return;
                }

                let block_number = match result {
                    Ok(ref val) => {
                        // chain_getHeader returns {"number": "0x...", ...}
                        val.get("number")
                            .and_then(|n| n.as_str())
                            .and_then(|s| {
                                let hex = s.strip_prefix("0x").unwrap_or(s);
                                u64::from_str_radix(hex, 16).ok()
                            })
                            .unwrap_or(0)
                    }
                    Err(e) => {
                        log::error!("[extrinsic] header query failed: {e}");
                        let b = self.chain_submit_building.take().unwrap();
                        self.resolve_spa_js(
                            b.webview_ptr,
                            b.js_id,
                            Err(&format!("header query failed: {e}")),
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                };

                let nonce_val = *nonce;
                let head_hash_val = *head_hash;
                let chain = build.chain;

                // Require chain meta to be cached (from the warmers fired earlier).
                let meta = match crate::chain_api::get_chain_meta(chain) {
                    Some(m) => m,
                    None => {
                        log::error!("[extrinsic] chain meta not cached");
                        let b = self.chain_submit_building.take().unwrap();
                        self.resolve_spa_js(
                            b.webview_ptr,
                            b.js_id,
                            Err("chain metadata not available — please retry"),
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                };

                // Decode call_data from hex.
                let call_data_hex = build
                    .call_data
                    .strip_prefix("0x")
                    .unwrap_or(&build.call_data);
                let call_data_bytes = match hex_decode(call_data_hex) {
                    Ok(b) => b,
                    Err(e) => {
                        let b = self.chain_submit_building.take().unwrap();
                        self.resolve_spa_js(
                            b.webview_ptr,
                            b.js_id,
                            Err(&format!("invalid call data hex: {e}")),
                            cx,
                        );
                        cx.notify();
                        return;
                    }
                };

                let params = crate::extrinsic::ExtrinsicParams {
                    spec_version: meta.spec_version,
                    tx_version: meta.tx_version,
                    genesis_hash: meta.genesis_hash,
                    mortality_checkpoint: head_hash_val,
                    block_number,
                    nonce: nonce_val,
                    tip: 0,
                };

                // Build signing payload.
                let signing_payload =
                    crate::extrinsic::build_signing_payload(&call_data_bytes, &params);

                // Sign with the root wallet key.
                let (pubkey, sig_bytes) = if cx.has_global::<crate::wallet::WalletGlobal>() {
                    let pk = match cx
                        .global::<crate::wallet::WalletGlobal>()
                        .manager
                        .root_public_key()
                    {
                        Some(pk) => pk,
                        None => {
                            let b = self.chain_submit_building.take().unwrap();
                            self.resolve_spa_js(b.webview_ptr, b.js_id, Err("wallet is locked"), cx);
                            cx.notify();
                            return;
                        }
                    };
                    let sig = match cx
                        .global_mut::<crate::wallet::WalletGlobal>()
                        .manager
                        .sign_root(&signing_payload)
                    {
                        Ok(s) => s,
                        Err(e) => {
                            let b = self.chain_submit_building.take().unwrap();
                            self.resolve_spa_js(b.webview_ptr, b.js_id, Err(&e.to_string()), cx);
                            cx.notify();
                            return;
                        }
                    };
                    (pk, sig)
                } else {
                    let b = self.chain_submit_building.take().unwrap();
                    self.resolve_spa_js(b.webview_ptr, b.js_id, Err("wallet not available"), cx);
                    cx.notify();
                    return;
                };

                if sig_bytes.len() < 64 {
                    let b = self.chain_submit_building.take().unwrap();
                    self.resolve_spa_js(b.webview_ptr, b.js_id, Err("signature too short"), cx);
                    cx.notify();
                    return;
                }
                let mut sig_arr = [0u8; 64];
                sig_arr.copy_from_slice(&sig_bytes[..64]);

                // Encode signed extrinsic as a 0x-prefixed hex string.
                let hex_xt = crate::extrinsic::encode_signed_extrinsic(
                    &call_data_bytes,
                    &pubkey,
                    &sig_arr,
                    &params,
                );
                log::info!("[extrinsic] built signed extrinsic ({} hex chars)", hex_xt.len());

                // Submit via author_submitExtrinsic.
                let b = self.chain_submit_building.take().unwrap();
                match crate::chain_api::submit_extrinsic(chain, b.webview_ptr, b.js_id, &hex_xt) {
                    Ok(()) => {} // Response arrives async via drain_responses.
                    Err(e) => {
                        self.resolve_spa_js(b.webview_ptr, b.js_id, Err(&e), cx);
                    }
                }
                cx.notify();
            }
        }
    }

    fn approve_data_connect(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_data_connect.take() else { return };

        match crate::data_api::approve_connection(req.conn_id) {
            Ok(()) => {
                if req.id > 0 {
                    // Outgoing — resolve the JS promise with conn_id.
                    let js = format!("window.__epocaResolve({}, null, {})", req.id, req.conn_id);
                    self.evaluate_on_spa(req.webview_ptr, &js, cx);
                }
                // Incoming connections (id == 0) have no promise to resolve;
                // the SPA will receive a dataConnected push event when ICE completes.
            }
            Err(e) => {
                if req.id > 0 {
                    self.resolve_spa_js(req.webview_ptr, req.id, Err(&e), cx);
                }
            }
        }
        cx.notify();
    }

    fn deny_data_connect(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_data_connect.take() else { return };
        let _ = crate::data_api::deny_connection(req.conn_id);
        if req.id > 0 {
            self.resolve_spa_js(req.webview_ptr, req.id, Err("user rejected connection"), cx);
        }
        cx.notify();
    }

    fn resolve_spa_js(&self, webview_ptr: usize, id: u64, result: Result<&str, &str>, cx: &Context<Self>) {
        let js = match result {
            Ok(val) => format!("window.__epocaResolve({}, null, '{}')", id, val),
            Err(e) => {
                let msg = e.replace('\'', "\\'");
                format!("window.__epocaResolve({}, '{}', null)", id, msg)
            }
        };
        self.evaluate_on_spa(webview_ptr, &js, cx);
    }

    fn approve_hostapi_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_hostapi_sign.take() else { return };
        let is_raw = req.request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ;

        let response = if cx.has_global::<crate::wallet::WalletGlobal>() {
            match cx
                .global_mut::<crate::wallet::WalletGlobal>()
                .manager
                .sign_root(&req.payload)
            {
                Ok(sig) => {
                    log::info!("hostapi: signed {} bytes for app {}", sig.len(), req.app_id);
                    epoca_hostapi::protocol::encode_sign_response(&req.request_id, is_raw, &sig)
                }
                Err(e) => {
                    log::warn!("hostapi: sign failed: {e}");
                    epoca_hostapi::protocol::encode_sign_error(&req.request_id, is_raw)
                }
            }
        } else {
            epoca_hostapi::protocol::encode_sign_error(&req.request_id, is_raw)
        };
        crate::host::send_response(req.webview_ptr, &response);
        cx.notify();
    }

    fn deny_hostapi_sign(&mut self, cx: &mut Context<Self>) {
        let Some(req) = self.pending_hostapi_sign.take() else { return };
        let is_raw = req.request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ;
        let response = epoca_hostapi::protocol::encode_sign_error(&req.request_id, is_raw);
        crate::host::send_response(req.webview_ptr, &response);
        cx.notify();
    }

    fn approve_wss_permission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(req) = self.pending_wss_permission.take() else { return };
        log::info!("[wss-perm] approved WSS for app={}, url={}", req.app_id, req.url);

        // Find the SPA tab matching this webview pointer.
        let tab_idx = self.tabs.iter().position(|t| {
            t.entity.clone().downcast::<SpaTab>().ok()
                .map(|e| e.read(cx).webview_ptr == req.webview_ptr)
                .unwrap_or(false)
        });

        if let Some(idx) = tab_idx {
            let old_tab_id = self.tabs[idx].id;
            let was_active = self.active_tab_id == Some(old_tab_id);
            let verification = self.tabs[idx].dot_verification.clone();

            // Build a manifest clone with chain permission granted.
            let spa_entity = self.tabs[idx].entity.clone().downcast::<SpaTab>().unwrap();
            let mut manifest = spa_entity.read(cx).manifest_snapshot().clone();
            let ipfs_cid = spa_entity.read(cx).ipfs_cid_snapshot().map(|s| s.to_string());
            if let Some(ref mut perms) = manifest.permissions {
                perms.chain = true;
            } else {
                manifest.permissions = Some(epoca_sandbox::bundle::PermissionsMeta {
                    network: None,
                    sign: false,
                    statements: false,
                    chain: true,
                    data: false,
                    media: Vec::new(),
                });
            }

            // Build a new bundle with updated manifest (empty assets — they are already registered).
            let bundle = ProdBundle {
                manifest,
                program_bytes: None,
                assets: std::collections::HashMap::new(),
                ipfs_cid,
            };

            // Remove old tab.
            self.tabs.remove(idx);

            // Open a new SPA tab with the updated permissions.
            let new_id = self.alloc_id();
            let app_name = bundle.manifest.app.name.clone();
            let app_id = bundle.manifest.app.id.clone();
            let new_entity = cx.new(|cx| SpaTab::new(bundle, window, cx));
            let url_display = if let Some(ref dv) = verification {
                format!("dot://{}.dot", dv.name)
            } else {
                format!("dot://{}.dot", app_name)
            };
            self.tabs.push(TabEntry {
                id: new_id,
                kind: TabKind::Spa { app_id: app_id.clone() },
                title: app_name,
                icon: IconName::Globe,
                entity: new_entity.into(),
                pinned: false,
                nav: None,
                favicon_url: None,
                context_id: None,
                loading_progress: 0.0,
                reader_active: false,
                readerable: false,
                dot_verification: verification,
            });
            if was_active {
                self.active_tab_id = Some(new_id);
                self.url_input.update(cx, |s, inner_cx| {
                    s.set_value(url_display, window, inner_cx);
                });
            }
        }
        cx.notify();
    }

    fn deny_wss_permission(&mut self, cx: &mut Context<Self>) {
        if let Some(req) = self.pending_wss_permission.take() {
            log::info!("[wss-perm] denied WSS for app={}", req.app_id);
        }
        cx.notify();
    }

    fn approve_http_permission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(req) = self.pending_http_permission.take() else { return };
        log::info!("[http-perm] approved HTTPS for app={}, origin={}", req.app_id, req.origin);

        // Find the SPA tab matching this webview pointer.
        let tab_idx = self.tabs.iter().position(|t| {
            t.entity.clone().downcast::<SpaTab>().ok()
                .map(|e| e.read(cx).webview_ptr == req.webview_ptr)
                .unwrap_or(false)
        });

        if let Some(idx) = tab_idx {
            let old_tab_id = self.tabs[idx].id;
            let was_active = self.active_tab_id == Some(old_tab_id);
            let verification = self.tabs[idx].dot_verification.clone();

            // Build a manifest clone with the origin added to network permissions.
            let spa_entity = self.tabs[idx].entity.clone().downcast::<SpaTab>().unwrap();
            let mut manifest = spa_entity.read(cx).manifest_snapshot().clone();
            let ipfs_cid = spa_entity.read(cx).ipfs_cid_snapshot().map(|s| s.to_string());
            if let Some(ref mut perms) = manifest.permissions {
                let origins = perms.network.get_or_insert_with(Vec::new);
                if !origins.contains(&req.origin) {
                    origins.push(req.origin.clone());
                }
            } else {
                manifest.permissions = Some(epoca_sandbox::bundle::PermissionsMeta {
                    network: Some(vec![req.origin.clone()]),
                    sign: false,
                    statements: false,
                    chain: false,
                    data: false,
                    media: Vec::new(),
                });
            }

            // Build a new bundle with updated manifest (empty assets — they are already registered).
            let bundle = ProdBundle {
                manifest,
                program_bytes: None,
                assets: std::collections::HashMap::new(),
                ipfs_cid,
            };

            // Remove old tab.
            self.tabs.remove(idx);

            // Open a new SPA tab with the updated permissions.
            let new_id = self.alloc_id();
            let app_name = bundle.manifest.app.name.clone();
            let app_id = bundle.manifest.app.id.clone();
            let new_entity = cx.new(|cx| SpaTab::new(bundle, window, cx));
            let url_display = if let Some(ref dv) = verification {
                format!("dot://{}.dot", dv.name)
            } else {
                format!("dot://{}.dot", app_name)
            };
            self.tabs.push(TabEntry {
                id: new_id,
                kind: TabKind::Spa { app_id: app_id.clone() },
                title: app_name,
                icon: IconName::Globe,
                entity: new_entity.into(),
                pinned: false,
                nav: None,
                favicon_url: None,
                context_id: None,
                loading_progress: 0.0,
                reader_active: false,
                readerable: false,
                dot_verification: verification,
            });
            if was_active {
                self.active_tab_id = Some(new_id);
                self.url_input.update(cx, |s, inner_cx| {
                    s.set_value(url_display, window, inner_cx);
                });
            }
        }
        cx.notify();
    }

    fn deny_http_permission(&mut self, cx: &mut Context<Self>) {
        if let Some(req) = self.pending_http_permission.take() {
            log::info!("[http-perm] denied HTTPS for app={}, origin={}", req.app_id, req.origin);
        }
        cx.notify();
    }

    fn evaluate_on_spa(&self, webview_ptr: usize, js: &str, cx: &Context<Self>) {
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<SpaTab>() {
                if entity.read(cx).webview_ptr == webview_ptr {
                    entity.read(cx).evaluate_script(js, cx);
                    return;
                }
            }
        }
    }

    fn wallet_sign_raw(&mut self, params_json: &str, cx: &mut Context<Self>) -> Result<String, String> {
        // params_json: {"raw":{"address":"...","data":"0x...","type":"bytes"}}
        let parsed: serde_json::Value = serde_json::from_str(params_json)
            .map_err(|e| format!("invalid params: {e}"))?;
        let data_hex = parsed["raw"]["data"]
            .as_str()
            .ok_or_else(|| "missing raw.data".to_string())?;
        let data_hex = data_hex.strip_prefix("0x").unwrap_or(data_hex);
        let payload = hex_decode(data_hex).map_err(|e| format!("invalid hex: {e}"))?;

        let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
        // Sign with root keypair (not per-app derived — dapps use the root address)
        let sig_bytes = wg.manager.sign_root(&payload)
            .map_err(|e| e.to_string())?;
        Ok(format!("0x{}", hex_encode(&sig_bytes)))
    }

    fn wallet_sign_payload(&mut self, params_json: &str, cx: &mut Context<Self>) -> Result<String, String> {
        // params_json: {"payload":{"address":"...","method":"0x...","era":"0x...","nonce":"0x...","tip":"0x...",
        //   "specVersion":"0x...","transactionVersion":"0x...","genesisHash":"0x...","blockHash":"0x...",
        //   "signedExtensions":[...],"version":4}}
        //
        // The signing payload for Substrate extrinsics:
        //   method ++ era ++ nonce ++ tip ++ specVersion ++ transactionVersion ++ genesisHash ++ blockHash
        // If the total is > 256 bytes, hash with blake2b-256 first, then sign the hash.
        let parsed: serde_json::Value = serde_json::from_str(params_json)
            .map_err(|e| format!("invalid params: {e}"))?;
        let payload = &parsed["payload"];

        let method = decode_hex_field(payload, "method")?;
        let era = decode_hex_field(payload, "era")?;
        let nonce = decode_compact_or_hex(payload, "nonce")?;
        let tip = decode_compact_or_hex(payload, "tip")?;
        let spec_version = decode_u32_le(payload, "specVersion")?;
        let tx_version = decode_u32_le(payload, "transactionVersion")?;
        let genesis_hash = decode_hex_field(payload, "genesisHash")?;
        let block_hash = decode_hex_field(payload, "blockHash")?;

        let mut signing_payload = Vec::new();
        signing_payload.extend_from_slice(&method);
        signing_payload.extend_from_slice(&era);
        signing_payload.extend_from_slice(&nonce);
        signing_payload.extend_from_slice(&tip);
        signing_payload.extend_from_slice(&spec_version);
        signing_payload.extend_from_slice(&tx_version);
        signing_payload.extend_from_slice(&genesis_hash);
        signing_payload.extend_from_slice(&block_hash);

        // If payload > 256 bytes, hash it first (Substrate standard)
        let to_sign = if signing_payload.len() > 256 {
            use blake2::Digest;
            let hash = blake2::Blake2b::<blake2::digest::consts::U32>::digest(&signing_payload);
            hash.to_vec()
        } else {
            signing_payload
        };

        let wg = cx.global_mut::<crate::wallet::WalletGlobal>();
        let sig_bytes = wg.manager.sign_root(&to_sign)
            .map_err(|e| e.to_string())?;
        Ok(format!("0x{}", hex_encode(&sig_bytes)))
    }

    fn resolve_wallet_sign_js(&self, webview_ptr: usize, id: u64, result: Result<&str, &str>, cx: &mut Context<Self>) {
        let js = match result {
            Ok(val) => format!("window.__epocaWalletResolve({}, null, {})", id, val),
            Err(e) => {
                let msg = e.replace('\'', "\\'");
                format!("window.__epocaWalletResolve({}, '{}', null)", id, msg)
            }
        };
        self.evaluate_on_webview(webview_ptr, &js, cx);
    }

    fn evaluate_on_webview(&self, webview_ptr: usize, js: &str, cx: &Context<Self>) {
        for tab in &self.tabs {
            if let Ok(entity) = tab.entity.clone().downcast::<WebViewTab>() {
                if entity.read(cx).webview_ptr == webview_ptr {
                    entity.read(cx).evaluate_script(js, cx);
                    return;
                }
            }
        }
    }

    fn render_btc_sign_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_btc_wallet_sign.as_ref()?;
        let origin = req.origin.clone();
        let message = if req.message.chars().count() > 200 {
            let truncated: String = req.message.chars().take(200).collect();
            format!("{truncated}…")
        } else {
            req.message.clone()
        };

        Some(
            div()
                .id("btc-sign-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_btc_wallet_sign(cx);
                }))
                .child(
                    div()
                        .id("btc-sign-dialog")
                        .w(px(380.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_, _, _| {})
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new("Bitcoin Sign Message")
                                        .text_size(px(15.0)),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(origin)
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .max_h(px(120.0))
                                .overflow_y_hidden()
                                .child(
                                    gpui_component::label::Label::new(message)
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().foreground),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("btc-sign-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_btc_wallet_sign(cx);
                                        })),
                                )
                                .child(
                                    Button::new("btc-sign-approve")
                                        .primary()
                                        .label("Sign")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_btc_wallet_sign(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_wallet_sign_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_wallet_sign.as_ref()?;
        let origin = req.origin.clone();
        let message = req.display_message.clone();

        Some(
            div()
                .id("wallet-sign-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_wallet_sign(cx);
                }))
                .child(
                    div()
                        .id("wallet-sign-dialog")
                        .w(px(380.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        // Stop click propagation so clicking the dialog doesn't dismiss
                        .on_click(|_, _, _| {})
                        // Title
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new("Signature Request")
                                        .text_size(px(15.0)),
                                ),
                        )
                        // Origin
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(origin)
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        // Message
                        .child(
                            gpui_component::label::Label::new(message)
                                .text_size(px(13.0)),
                        )
                        // Buttons
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("wallet-sign-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_wallet_sign(cx);
                                        })),
                                )
                                .child(
                                    Button::new("wallet-sign-approve")
                                        .primary()
                                        .label("Sign")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_wallet_sign(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_spa_sign_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_spa_sign.as_ref()?;
        let app_id = req.app_id.clone();
        let payload_display = if req.payload.len() > 120 {
            format!("{}…", &req.payload[..120])
        } else {
            req.payload.clone()
        };

        Some(
            div()
                .id("spa-sign-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_spa_sign(cx);
                }))
                .child(
                    div()
                        .id("spa-sign-dialog")
                        .w(px(380.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_, _, _| {})
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new("Signature Request")
                                        .text_size(px(15.0)),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(app_id)
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .overflow_hidden()
                                .child(
                                    gpui_component::label::Label::new(payload_display)
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("spa-sign-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_spa_sign(cx);
                                        })),
                                )
                                .child(
                                    Button::new("spa-sign-approve")
                                        .primary()
                                        .label("Sign")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_spa_sign(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_chain_submit_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_chain_submit.as_ref()?;
        let app_id = req.app_id.clone();
        let chain_name = req.chain.display_name().to_string();
        let call_display = if req.call_data.len() > 200 {
            format!("{}…", &req.call_data[..200])
        } else {
            req.call_data.clone()
        };

        Some(
            div()
                .id("chain-submit-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_chain_submit(cx);
                }))
                .child(
                    div()
                        .id("chain-submit-dialog")
                        .w(px(400.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_, _, _| {})
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new("Transaction Request")
                                        .text_size(px(15.0)),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(format!("{app_id} → {chain_name}"))
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .overflow_hidden()
                                .max_h(px(120.0))
                                .child(
                                    gpui_component::label::Label::new(call_display)
                                        .text_size(px(11.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("chain-submit-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_chain_submit(cx);
                                        })),
                                )
                                .child(
                                    Button::new("chain-submit-approve")
                                        .primary()
                                        .label("Submit")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_chain_submit(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_data_connect_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_data_connect.as_ref()?;
        let app_id = req.app_id.clone();
        let peer_display = if req.peer_address.len() > 20 {
            format!("{}…{}", &req.peer_address[..8], &req.peer_address[req.peer_address.len()-8..])
        } else {
            req.peer_address.clone()
        };

        Some(
            div()
                .id("data-connect-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_data_connect(cx);
                }))
                .child(
                    div()
                        .id("data-connect-dialog")
                        .w(px(380.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_, _, _| {})
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new("Connection Request")
                                        .text_size(px(15.0)),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(format!("{app_id}"))
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(format!("Connect to peer: {peer_display}"))
                                        .text_size(px(12.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("data-connect-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_data_connect(cx);
                                        })),
                                )
                                .child(
                                    Button::new("data-connect-approve")
                                        .primary()
                                        .label("Allow")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_data_connect(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }

    fn render_hostapi_sign_dialog(&self, _window: &mut Window, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let req = self.pending_hostapi_sign.as_ref()?;
        let app_id = req.app_id.clone();
        let is_raw = req.request_tag == epoca_hostapi::protocol::TAG_SIGN_RAW_REQ;
        let kind = if is_raw { "Sign Raw Data" } else { "Sign Transaction" };
        let payload_hex = if req.payload.len() > 60 {
            format!("{}…", hex_encode(&req.payload[..60]))
        } else {
            hex_encode(&req.payload)
        };

        Some(
            div()
                .id("hostapi-sign-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .bg(rgba(0x00000088u32))
                .flex()
                .items_center()
                .justify_center()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.deny_hostapi_sign(cx);
                }))
                .child(
                    div()
                        .id("hostapi-sign-dialog")
                        .w(px(380.0))
                        .p(px(20.0))
                        .rounded(px(12.0))
                        .bg(cx.theme().background)
                        .border_1()
                        .border_color(cx.theme().border)
                        .flex()
                        .flex_col()
                        .gap(px(12.0))
                        .on_click(|_, _, _| {})
                        // Title
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(
                                    Icon::new(IconName::TriangleAlert)
                                        .size(px(20.0))
                                        .text_color(cx.theme().warning),
                                )
                                .child(
                                    gpui_component::label::Label::new(kind)
                                        .text_size(px(15.0)),
                                ),
                        )
                        // App ID
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .child(
                                    gpui_component::label::Label::new(app_id)
                                        .text_size(px(13.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        // Payload preview
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(cx.theme().secondary)
                                .overflow_hidden()
                                .child(
                                    gpui_component::label::Label::new(payload_hex)
                                        .text_size(px(11.0))
                                        .text_color(cx.theme().muted_foreground),
                                ),
                        )
                        // Buttons
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .justify_end()
                                .child(
                                    Button::new("hostapi-sign-deny")
                                        .ghost()
                                        .label("Reject")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.deny_hostapi_sign(cx);
                                        })),
                                )
                                .child(
                                    Button::new("hostapi-sign-approve")
                                        .primary()
                                        .label("Sign")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.approve_hostapi_sign(cx);
                                        })),
                                ),
                        ),
                ),
        )
    }
}

impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_pending_nav(window, cx);

        // Handle deferred dot-loading approve/deny (subscribe callbacks lack window).
        if let Some((bundle, verification)) = self.pending_dot_approve.take() {
            let is_framebuffer = bundle.manifest.app.app_type == "framebuffer"
                || bundle.manifest.sandbox.as_ref().map_or(false, |s| s.framebuffer);
            if is_framebuffer {
                // Framebuffer/application bundle — open in PolkaVM sandbox.
                self.open_framebuffer_app(bundle, window, cx);
            } else {
                self.open_spa_with_verification(bundle, verification, window, cx);
            }
        }
        if let Some(tab_id) = self.pending_dot_deny_tab.take() {
            self.close_tab(tab_id, window, cx);
        }

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
                // Dim the webview when an approval dialog is pending.
                cx.set_global(crate::WebViewDimmed(self.has_pending_approval()));

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
                            .rounded(px(RADIUS + 2.0))
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .border_2()
                            .border_color(self.loading_glow_color().unwrap_or(chrome_bg.into()))
                            .child(self.render_content(window, cx)),
                    );

                let omnibox = if self.omnibox_open {
                    Some(self.render_omnibox(window, cx).into_any_element())
                } else {
                    None
                };

                // Backdrop to dismiss wallet popover when clicking outside
                let wallet_backdrop = if self.wallet_popover_open {
                    Some(
                        div()
                            .id("wallet-backdrop")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wallet_popover_open = false;
                                cx.notify();
                            })),
                    )
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
                    .children(wallet_backdrop)
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
                    .on_action(cx.listener(|this, _: &OpenAppLibrary, window, cx| this.open_app_library(window, cx)))
                    .on_action(cx.listener(|this, _: &OpenApp, window, cx| this.handle_open_app(window, cx)))
                    .on_action(cx.listener(|this, _: &OpenBookmarks, window, cx| this.open_bookmarks(window, cx)))
                    .on_action(cx.listener(|this, _: &AddBookmark, window, cx| this.toggle_bookmark(window, cx)))
                    .on_action(cx.listener(|this, _: &ToggleReaderMode, _, cx| this.toggle_reader_mode(cx)))
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
                    .on_action(cx.listener(|this, _: &gpui_component::input::Copy, _, cx| {
                        this.clipboard_to_webview("copy", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::Cut, _, cx| {
                        this.clipboard_to_webview("cut", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::Paste, _, cx| {
                        this.clipboard_to_webview("paste", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::SelectAll, _, cx| {
                        this.clipboard_to_webview("selectAll", cx);
                    }))
            }

            // ---- Overlay: sidebar slides in as a modal over full-width content ----
            SidebarMode::Overlay => {
                let anim = self.sidebar_anim;

                // Publish the sidebar inset so WebViewTab can apply a CALayer mask
                // that clips the WKWebView away from the sidebar area.
                let webview_inset = (SIDEBAR_W * anim - CHROME).max(0.0);
                cx.set_global(OverlayLeftInset(webview_inset));
                // Dim the webview when an approval dialog is pending.
                cx.set_global(crate::WebViewDimmed(self.has_pending_approval()));

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
                            .rounded(px(RADIUS + 2.0))
                            .overflow_hidden()
                            .bg(cx.theme().background)
                            .border_2()
                            .border_color(self.loading_glow_color().unwrap_or(chrome_bg.into()))
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

                let wallet_backdrop_overlay = if self.wallet_popover_open {
                    Some(
                        div()
                            .id("wallet-backdrop-overlay")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.wallet_popover_open = false;
                                cx.notify();
                            })),
                    )
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

                div()
                    .relative()
                    .size_full()
                    .bg(chrome_bg)
                    .child(content)
                    .children(wallet_backdrop_overlay)
                    .children(ctx_backdrop_overlay)
                    .children(sidebar)
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
                    .on_action(cx.listener(|this, _: &OpenAppLibrary, window, cx| this.open_app_library(window, cx)))
                    .on_action(cx.listener(|this, _: &OpenApp, window, cx| this.handle_open_app(window, cx)))
                    .on_action(cx.listener(|this, _: &OpenBookmarks, window, cx| this.open_bookmarks(window, cx)))
                    .on_action(cx.listener(|this, _: &AddBookmark, window, cx| this.toggle_bookmark(window, cx)))
                    .on_action(cx.listener(|this, _: &ToggleReaderMode, _, cx| this.toggle_reader_mode(cx)))
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
                    .on_action(cx.listener(|this, _: &gpui_component::input::Copy, _, cx| {
                        this.clipboard_to_webview("copy", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::Cut, _, cx| {
                        this.clipboard_to_webview("cut", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::Paste, _, cx| {
                        this.clipboard_to_webview("paste", cx);
                    }))
                    .on_action(cx.listener(|this, _: &gpui_component::input::SelectAll, _, cx| {
                        this.clipboard_to_webview("selectAll", cx);
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


/// Escape a string for safe embedding in a JS single-quoted string literal.
fn escape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Hex helpers for wallet payload construction
// ---------------------------------------------------------------------------

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("odd-length hex".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode a "0x..." hex field from a JSON value.
fn decode_hex_field(obj: &serde_json::Value, key: &str) -> Result<Vec<u8>, String> {
    let s = obj[key].as_str().ok_or_else(|| format!("missing {key}"))?;
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex_decode(s)
}

/// Decode a SCALE-compact-encoded or hex integer field.
/// Polkadot.js sends nonce/tip as "0x..." hex strings of the SCALE-compact encoding.
fn decode_compact_or_hex(obj: &serde_json::Value, key: &str) -> Result<Vec<u8>, String> {
    let s = obj[key].as_str().ok_or_else(|| format!("missing {key}"))?;
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex_decode(s)
}

/// Decode a JSON value as a u32 and return its little-endian 4 bytes.
/// Polkadot.js sends specVersion/transactionVersion as hex strings or numbers.
fn decode_u32_le(obj: &serde_json::Value, key: &str) -> Result<Vec<u8>, String> {
    let val = &obj[key];
    if let Some(n) = val.as_u64() {
        return Ok((n as u32).to_le_bytes().to_vec());
    }
    if let Some(s) = val.as_str() {
        let s = s.strip_prefix("0x").unwrap_or(s);
        let n = u32::from_str_radix(s, 16).map_err(|e| format!("{key}: {e}"))?;
        return Ok(n.to_le_bytes().to_vec());
    }
    Err(format!("missing or invalid {key}"))
}

/// Record a browsing history visit, skipping non-http URLs.
fn record_history_visit(url: &str, title: &str, cx: &gpui::App) {
    if !crate::history::is_http_url(url) {
        return;
    }
    if let Some(hg) = cx.try_global::<crate::history::HistoryGlobal>() {
        hg.manager.record_visit(url, title);
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

// ---------------------------------------------------------------------------
// Test server helpers (feature-gated)
// ---------------------------------------------------------------------------
#[cfg(feature = "test-server")]
impl Workbench {
    /// Returns (kind, high_risk) if an approval dialog is pending.
    pub(crate) fn test_pending_approval_info(&self) -> Option<(&'static str, bool)> {
        if self.pending_wallet_sign.is_some() {
            Some(("wallet_sign", true))
        } else if self.pending_btc_wallet_sign.is_some() {
            Some(("btc_wallet_sign", true))
        } else if self.pending_spa_sign.is_some() {
            Some(("spa_sign", true))
        } else if self.pending_hostapi_sign.is_some() {
            Some(("hostapi_sign", true))
        } else if self.pending_chain_submit.is_some() {
            Some(("chain_submit", true))
        } else if self.pending_data_connect.is_some() {
            Some(("data_connect", false))
        } else if self.pending_wss_permission.is_some() {
            Some(("wss_permission", false))
        } else if self.pending_http_permission.is_some() {
            Some(("http_permission", false))
        } else {
            None
        }
    }

    /// Approve the currently pending approval dialog. Returns true if there was one.
    pub(crate) fn test_approve_pending(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_wallet_sign.is_some() {
            self.approve_wallet_sign(cx);
        } else if self.pending_btc_wallet_sign.is_some() {
            self.approve_btc_wallet_sign(cx);
        } else if self.pending_spa_sign.is_some() {
            self.approve_spa_sign(cx);
        } else if self.pending_hostapi_sign.is_some() {
            self.approve_hostapi_sign(cx);
        } else if self.pending_chain_submit.is_some() {
            self.approve_chain_submit(cx);
        } else if self.pending_data_connect.is_some() {
            self.approve_data_connect(cx);
        } else if self.pending_wss_permission.is_some() {
            self.approve_wss_permission(window, cx);
        } else if self.pending_http_permission.is_some() {
            self.approve_http_permission(window, cx);
        } else {
            return false;
        }
        true
    }

    /// Deny the currently pending approval dialog. Returns true if there was one.
    pub(crate) fn test_deny_pending(&mut self, cx: &mut Context<Self>) -> bool {
        if self.pending_wallet_sign.is_some() {
            self.deny_wallet_sign(cx);
        } else if self.pending_btc_wallet_sign.is_some() {
            self.deny_btc_wallet_sign(cx);
        } else if self.pending_spa_sign.is_some() {
            self.deny_spa_sign(cx);
        } else if self.pending_hostapi_sign.is_some() {
            self.deny_hostapi_sign(cx);
        } else if self.pending_chain_submit.is_some() {
            self.deny_chain_submit(cx);
        } else if self.pending_data_connect.is_some() {
            self.deny_data_connect(cx);
        } else if self.pending_wss_permission.is_some() {
            self.deny_wss_permission(cx);
        } else if self.pending_http_permission.is_some() {
            self.deny_http_permission(cx);
        } else {
            return false;
        }
        true
    }
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
