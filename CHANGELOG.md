# Changelog

All notable changes to Epoca are recorded here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased] — ongoing

### Added
- **SPA Tab (sandboxed single-page app)** — New `SpaTab` tab type and `TabKind::Spa` for hosting
  bundled client-side web apps in a sandboxed WKWebView. `.prod` bundle format extended: `type = "spa"`
  in manifest, `[webapp]` section with `entry` + `sandbox`, `program_bytes` now optional. Broker
  `Permissions` extended with `sign`, `statement_store`, `media`. `open_webapp()` dispatches SPA
  bundles. Placeholder UI renders while WKURLSchemeHandler integration is pending. `bundle.rs`,
  `tabs.rs`, `workbench.rs`, `session.rs`, `broker/lib.rs`. (2026-03-05)

- **Content Shield description updated** — Settings subtitle now accurately lists all 9 filter lists
  (EasyList, AdGuard, uBlock Origin, Fanboy, Peter Lowe) instead of just "EasyList + EasyPrivacy". (2026-03-05)

- **Browsing history (ephemeral with TTL)** — SQLite-backed browsing history at
  `~/.epoca/history.db`. Configurable retention: Session Only / 8h / 24h (default) / 7d / 30d.
  Hard DELETE on expiry, incremental vacuum. Frecency-ordered search in omnibox (⌘T) with
  "History" divider between tab matches and history results. File permissions 0600. Corrupt DB
  auto-recreated. `history.rs`, `HistoryRetention` enum in settings, retention selector in
  Settings UI. 29 unit tests. (2026-03-05)

- **Session restore** — Open tabs are saved to `session.json` every 30s and on quit (⌘Q). On next
  launch (no CLI arg), tabs are restored with their URLs, titles, favicons, pinned state, and
  session contexts. Atomic writes via `.tmp` + rename. Skips non-restorable tab types (SandboxApp,
  FramebufferApp, Welcome). `session.rs`, `WorkbenchRef` global. (2026-03-04)

- **Find-in-page (⌘F)** — Opens a find bar between chrome and content area. Live search on typing,
  Enter for next match, Shift+Enter/↑ for previous, Escape to close. Uses `window.find()` JS API
  on the active WebViewTab. No-op on non-WebView tabs. `FindInPage`, `FindPrev`, `CloseFindBar`
  actions. Edit menu with Find entry. (2026-03-04)

- **Embedded HTTP test server** — `localhost:9223` test API behind `#[cfg(feature = "test-server")]` +
  `EPOCA_TEST=1`. Endpoints: `GET /state` (full app snapshot), `POST /action` (navigate, new_tab,
  close_tab, etc.), `GET /webview/eval?js=X` (JS eval with correlation IDs). Channel-drain pattern
  matches existing shield.rs architecture. `test_server.rs`, `tools/test_cursor.sh`. (2026-03-04)

- **Session contexts** — Named browsing sessions with separate WKWebView data stores. Each context
  shares cookies/storage across its tabs; tabs without a context are fully private. Context picker
  dropdown in URL bar (colored dot prefix). "Open in Context" submenu in right-click menu.
  `experimental_contexts` setting toggle. `SessionContext` type in settings.rs. (2026-03-04)

- **"+ New Context" in dropdown** — Context picker dropdown includes a "+ New Context" row at the
  bottom, so users can create contexts without going to Settings. Auto-picks first unused color
  from preset palette. (2026-03-04)

- **"Private Tab" in context menu** — "Open in Context" right-click submenu always shows "Private Tab"
  option (opens link with no context/isolated data store). `MenuAction::OpenPrivate`. (2026-03-04)

- **URL bar triple-click select-all** — Triple-clicking the URL bar selects the entire URL, matching
  standard browser behavior. Uses `on_mouse_down` with `click_count >= 3` to dispatch
  `gpui_component::input::SelectAll`. (2026-03-04)

### Fixed
- **Cursor pointer (hand icon) on links** — GPUI's `reset_cursor_style()` overrode WKWebView's CSS
  cursor every paint frame. Fix: `canvas` paint closure calls `window.set_window_cursor_style(
  CursorStyle::PointingHand)` with `hitbox_id: None`, bypassing hit-test. (2026-03-04)

- **URL bar navigates in-place** — Entering a URL or search query in the URL bar now navigates the
  current WebView tab instead of always opening a new tab. New tabs only opened from omnibox (⌘T)
  or when no navigable tab exists. (2026-03-04)

- **Context indicator syncs on tab switch** — `active_context` now updates when switching tabs, but
  only from WebView tabs. Non-WebView tabs (Settings, Welcome) don't reset the context indicator.
  (2026-03-04)

- **Context switch reopens with live URL** — `switch_context()` reads the URL bar input (live URL)
  instead of the stale `TabKind::WebView { url }` that may reflect the original navigation. (2026-03-04)

- **Context dropdown backdrop covers full window** — Backdrop div now renders at the root Workbench
  level so clicks anywhere outside the dropdown dismiss it (previously only covered sidebar area).
  Dropdown also auto-closes when sidebar hides in overlay mode. (2026-03-04)

- **Orphaned tabs cleaned up on context delete** — When a context is deleted in Settings, tabs that
  referenced it have their `context_id` set to `None` (private). `active_context` also resets if it
  referenced the deleted context. Checked in `process_pending_nav`. (2026-03-04)

- **Duplicate context colors** — Both the dropdown "+ New Context" and Settings "Add Context" now
  pick the first unused color from `DEFAULT_CONTEXT_COLORS` instead of cycling by index, preventing
  duplicates after delete+create cycles. (2026-03-04)

- **Context dot sizes standardized** — URL bar dot 6px, tab sidebar dot 5px, dropdown dot 6px
  (previously inconsistent 4–7px). (2026-03-04)

- **Right-click link context menu** — Right-clicking a link in any WebView shows a native NSMenu with:
  Open in New Tab, Open in New Window, Open in Context (submenu, when experimental contexts enabled),
  and Copy Link Address. `CONTEXT_MENU_SCRIPT` (init script) intercepts `contextmenu` events on `<a>` tags,
  posts via `epocaContextMenu` WKScriptMessageHandler (`EpocaContextMenuHandler` ObjC class,
  `CONTEXT_MENU_CHANNEL`). NSMenu actions route back via `MENU_ACTION_CHANNEL` → `drain_menu_actions()`
  in `process_pending_nav`. `EpocaMenuTarget` ObjC class handles selector callbacks. (2026-03-04)

- **Favicon in tab list** — Sidebar tab rows show the site's favicon instead of the generic
  globe icon. `FAVICON_SCRIPT` finds the best `<link rel="icon">` URL or falls back to
  `/favicon.ico`, posts via `epocaFavicon` WKScriptMessageHandler (`EpocaFaviconHandler`
  ObjC class, `FAVICON_CHANNEL`, `drain_favicon_events()`). Stored in `TabEntry.favicon_url`;
  rendered with `img()` (GPUI URI image loading); falls back to `IconName::Globe`. (2026-03-03)

- **URL bar padding tightened** — Switched Input to `Size::Small` (8px horizontal padding,
  down from 12px) and `.appearance(false)` to avoid double bg/border. (2026-03-03)

- **Live page titles in tab list** — Sidebar shows the actual page title instead of the URL slug.
  `TITLE_TRACKER_SCRIPT` monitors `document.title` via MutationObserver + SPA navigation hooks,
  posts via `epocaMeta` WKScriptMessageHandler; `EpocaMetaHandler` ObjC class routes events by
  WKWebView pointer (`WebViewTab.webview_ptr`); drained each frame in `process_pending_nav`. (2026-03-03)

- **Link status bar** — Arc-style frosted-glass pill fixed at bottom-left of every WebView.
  Shows hovered link URL; updates to "Open in new tab: [url]" while ⌘ held and
  "Open in new tab → switch: [url]" while ⌘⇧ held. Fades in/out smoothly.
  `LINK_STATUS_SCRIPT` init script, idempotent. (2026-03-03)

- **Tab isolation ("Fresh Tab" mode)** — `Workbench.isolated_tabs: bool`. When enabled,
  every new WebView tab uses `WKWebsiteDataStore.nonPersistentDataStore()` (macOS) via
  wry's `.with_incognito(true)`. No cookies, localStorage, or cache shared between tabs or
  sessions. Defeats metered paywalls and cross-tab tracking. Toggle via
  `Workbench::set_isolated_tabs(bool)`. (2026-03-03)

- **Keyboard shortcuts & native menu bar** — ⌘T (New Tab), ⌘W (Close Tab), ⌘R (Reload),
  ⌘⇧R (Hard Reload), ⌘L (Focus URL Bar), ⌘Q (Quit). macOS menu bar: Epoca, File, View.
  Hard reload uses `reloadFromOrigin` (bypasses cache). (2026-03-03)

- **Cmd-click → open in background tab** — `⌘-click` any link opens it as a background tab
  (active tab unchanged). `⌘⇧-click` opens with focus. Implemented via `CMD_CLICK_SCRIPT`
  init script + `EpocaNavHandler` ObjC `WKScriptMessageHandler` + `NAV_CHANNEL` drain in
  Workbench. `Workbench.open_links_in_background: bool` (default `true`). (2026-03-03)

- **Cmd-click ripple** — Green expanding ring (`#44bb66`) radiates from the click point when
  a background tab is opened via ⌘-click. 400ms, non-blocking. `RIPPLE_SCRIPT` init script,
  idempotent via `window.__epocaRipple` guard. (2026-03-03)

- **Shield badge on globe icon** — Globe icon in URL bar turns green (`#44bb6699`) when the
  shield is active, red (`#cc444499`) when the site is excepted, muted when no shield is
  loaded. Color derived from `ShieldGlobal` state + hostname lookup each render. (2026-03-03)

- **Cosmetic blocked count in URL bar** — Small green number next to globe shows how many
  elements the shield hid on the current page. Sourced from `epocaShield` WKScriptMessageHandler
  (`EpocaShieldHandler` ObjC class, `SHIELD_CHANNEL`, `drain_shield_events()`), stored in
  `WebViewTab.blocked_count`. (2026-03-03)

- **Per-site shield exception toggle** — Eye/EyeOff button in URL bar suffix toggles the
  shield exception for the active tab's hostname. Calls `ShieldManager::toggle_site_exception()`
  via `ShieldGlobal`. Globe turns red when site is excepted. `ToggleSiteShield` GPUI action. (2026-03-03)

- **6-hour background filter list refresh** — `init_shield` bootstrap thread now loops,
  sleeping 6 hours between re-fetches of EasyList/EasyPrivacy, keeping rules fresh without
  a restart. Updates `COMPILED_CONFIG` static; applies to newly opened tabs. (2026-03-03)

---

## [0.0.1] — 2026-02-12

### Added
- **Workbench shell** — GPUI-based browser chrome with pinned and overlay sidebar modes.
  Sidebar animates in/out; traffic lights positioned inside the sidebar panel.
  Fullscreen-safe: sidebar hides cleanly, mini toolbar with pin button shown in fullscreen.

- **WebView tabs** — WKWebView-backed browser tabs via wry. CALayer mask clips rendering
  to sidebar-free region without changing the page viewport (no content reflow).
  `EpocaSidebarBlocker` NSView intercepts hit-testing in the sidebar overlay region.

- **Content blocking (epoca-shield)** — Four-layer pipeline:
  WKContentRuleList (EasyList + EasyPrivacy, 45k-rule bucket splitting),
  document_start JS (fingerprint countermeasures), document_end JS (overlay sweeper, cookie
  consent auto-dismiss), cosmetic count reporting via `epocaShield` WKScriptMessageHandler.
  _Not yet implemented: DNS CNAME uncloaking (hickory-dns), WKNavigationDelegate layer._

- **Omnibox** — ⌘T opens floating search/URL input. Accepts URLs, local file paths
  (`.toml`, `.zml`, `.polkavm`), and search queries (DuckDuckGo fallback).

- **Tab list with page titles** — Sidebar shows live page titles (tracked via
  `TITLE_TRACKER_SCRIPT` + `epocaMeta` WKScriptMessageHandler + `TITLE_CHANNEL` drain).
  Falls back to hostname while page loads.

- **Arc-style URL status bar** — Fixed bottom-left pill showing hovered link URL.
  Shows "Open in new tab: [url]" hint while ⌘ is held. Frosted-glass dark pill,
  monospace text, fades in/out smoothly. (`LINK_STATUS_SCRIPT` init script.)

- **Cmd-click ripple** — Green expanding ring (`#44bb66`) animates from the click point
  when a background tab is opened. 400ms, non-blocking. (`RIPPLE_SCRIPT` init script.)

- **Scrollbar styling** — Custom dark scrollbar matching browser chrome via
  `SCROLLBAR_CSS_SCRIPT`. Pointer cursor forced on all `<a href>` elements.

- **PolkaVM guest tabs** — Sandboxed PolkaVM guest applications run as first-class tabs.
  ZML declarative UI pipeline: `.zml` → Parser → Evaluator → ViewTree → GPUI.

- **Declarative app tabs** — `.toml` and `.zml` files open as tab apps. Dev mode
  (`--dev` flag) hot-reloads on file change.

- **Capability broker** — `epoca-broker` crate. App manifests declare permissions;
  broker enforces at runtime.

- **Android renderer** — `epoca-android` crate: winit 0.30 + wgpu 23 + cosmic-text 0.12.
  Custom 2D flexbox layout. Renders ViewTree from ZML pipeline. Desktop preview binary.

- **Crash reporting** — Sentry integration (opt-in via `SENTRY_DSN` env var).

- **CI** — GitHub Actions on `macos-latest`: test → build → fmt → clippy.

---

## Format notes
- Dates are when the feature landed in the working codebase, not when it was planned.
- Entries use past tense ("Added X") not present tense.
- Implementation detail (crate names, key types, ObjC class names) is included so release
  notes can be written from this file without re-reading source.
