# Epoca Agent Instructions

## Project
Cross-platform programmable workbench in Rust using GPUI + gpui-component + PolkaVM.
See `crates/epoca-core/src/workbench.rs` (shell), `crates/epoca-core/src/tabs.rs` (tab panels).

## Design Constraints
**Always read `docs/design.md` before making any UI changes.**
The values in that file (colors, spacing, radius, row heights, alignment rules) are locked in
and must not be changed without explicit user approval.

## Key Rules
- `CLAUDE.md` redirects here — this file is the authoritative instruction source
- Workspace dependencies are pinned; do not bump versions without asking
- `gpui_component::wry` is the webview crate; access raw view via `WebViewExtMacOS`
- Traffic light visibility is managed by `set_traffic_lights_hidden()` in `workbench.rs`;
  call it only when `!is_window_fullscreen()` — hiding traffic lights in fullscreen traps
  the user with no way to exit fullscreen mode
- `OverlayLeftInset` drives a **CALayer mask** on the WKWebView — do NOT convert it back
  into a frame shift or padding. Formula: `(SIDEBAR_W × anim − CHROME).max(0)`.
  The mask clips WKWebView rendering to x=inset..width; GPUI sidebar shows through the
  unmasked region. WKWebView frame is unchanged → no page reflow. See design.md §Overlay.
- **WKWebView z-order**: On macOS, WKWebView (native NSView) is above GPUI's Metal layer.
  The CALayer mask technique makes GPUI's sidebar visible over web content by hiding the
  WKWebView in the sidebar region — the page viewport does not change.
- New tab types must implement `Panel + EventEmitter<PanelEvent> + Focusable + Render`
- Always `use gpui::prelude::FluentBuilder` when using `.when()` on divs
- `InputState::set_value()` not `set_text()`; `Input::new(&entity_ref)` not direct construction
- **Entity notify rule**: When `entity.update(cx, |model, ecx| { ... })` changes ANY field
  that affects rendering, you MUST call `ecx.notify()` inside the closure. Without this,
  GPUI will not re-render the entity and the change is invisible.
- **GPUI overrides WKWebView cursor**: GPUI calls `[[NSCursor arrowCursor] set]` every paint
  frame. CSS `cursor:pointer` in WKWebView has no effect. Instead, track hover state via JS →
  `epocaCursor` channel → `WebViewTab.cursor_pointer: bool` → `.cursor_pointer()` on the GPUI
  wrapper div. This makes GPUI's own cursor system apply the hand cursor.
- **ObjC msg_send return types**: Always match the actual ObjC return type. `BOOL` methods
  must use `let _: bool = msg_send![...]`, NOT `let _: () = ...`. Wrong type = bus error.
- **NSString creation**: Use `stringWithUTF8String:` (takes `*const i8`), NOT
  `initWithBytes:length:encoding:` (expects `*const c_void`, causes type mismatch crash).

## Workspace Structure
```
crates/
  epoca/            binary entry point (main.rs)
  epoca-core/       workbench shell, tabs, view_bridge, declarative parser
  epoca-sandbox/    PolkaVM runtime
  epoca-protocol/   ViewTree + diffing + MessagePack
  epoca-broker/     capability/permission broker
  epoca-dsl/        ZML parser + evaluator (no GPUI dep)
  epoca-android/    Android renderer (winit + wgpu + cosmic-text)
guest/counter/      sample PolkaVM guest (no_std)
examples/           .toml and .zml sample apps
docs/design.md      LOCKED design system — read before any UI change
docs/backlog.md     Product backlog + strategy — update when completing features
```

## Key Architecture Note — WKWebView Positioning
`WebViewTab` observes `OverlayLeftInset` via `cx.observe_global` so GPUI marks it dirty
and re-paints the native WKWebView frame every animation frame. Do not remove this
subscription or the native view will get stuck at its last position on sidebar toggle.

---

## Implemented Features — Must Not Regress

This section is the authoritative list of working browser features. Before removing or
refactoring any code that touches these areas, verify the feature still works.
If unsure, ask rather than assume.

### Navigation & Tabs
- **New tab** (⌘T / File > New Tab): opens omnibox for URL entry
- **Close tab** (⌘W / File > Close Tab): closes active tab, selects adjacent tab
- **Focus URL bar** (⌘L / View > Focus URL Bar): focuses url_input in the active tab's top bar
- **Reload** (⌘R / View > Reload Page): calls `wry::WebView::reload()` on the active WebViewTab
- **Hard Reload** (⌘⇧R / View > Hard Reload): calls `reloadFromOrigin` (macOS ObjC) — bypasses cache
- **Quit** (⌘Q / Epoca > Quit Epoca): calls `cx.quit()`
- **Tab switching**: clicking a tab in the sidebar activates it and shows its content

### Cmd-Click — Open Link in New Tab
- **⌘-click any link** → opens in a **background tab** (current tab stays active)
- **⌘⇧-click any link** → opens in a **foreground tab** (switches to new tab)
- Implementation pipeline:
  1. `CMD_CLICK_SCRIPT` (init script in every WebView) intercepts `metaKey` clicks,
     posts `{type:'openInNewTab', url}` or `{type:'openInNewTabFocus', url}` to
     `window.webkit.messageHandlers.epocaNav`
  2. `EpocaNavHandler` ObjC class (registered in `register_nav_handler()` in shield.rs)
     receives the message and sends `(url, focus: bool)` to `NAV_CHANNEL`
  3. `drain_nav_events()` called every render frame from `process_pending_nav()` in Workbench
  4. `open_webview_background()` for background opens; `open_webview()` for foreground
- `Workbench.open_links_in_background: bool` (default: `true`)
- **Ripple animation**: `RIPPLE_SCRIPT` init script shows a gray expanding ring (`rgba(160,160,160,0.7)`) at the ⌘-click point when a background tab opens; 400ms; idempotent via `window.__epocaRipple`

### Content Blocking (epoca-shield)
- EasyList / EasyPrivacy rules compiled to WKContentRuleList JSON and installed on each WebView
- Cosmetic hiding (document_end_script): overlay sweeper, cookie consent auto-dismiss
- **Per-site exception toggle**: Clicking the globe button in the URL bar calls `ShieldManager::toggle_site_exception(hostname)` via `cx.update_global::<ShieldGlobal>`; `ToggleSiteShield` GPUI action. Globe color changes green↔red as feedback. No separate Eye button.
- **Shield cosmetic count**: `epocaShield` WKScriptMessageHandler (`EpocaShieldHandler` ObjC, `SHIELD_CHANNEL`, `drain_shield_events()`); stored in `WebViewTab.blocked_count`; displayed as green number next to globe
- **Shield badge**: Globe icon color — green (#44bb6699) = active, red (#cc444499) = site excepted, muted = no shield. Computed from `ShieldGlobal` + `hostname_from_url()` each render
- **6-hour list refresh**: `init_shield` bootstrap thread loops with 6-hour sleep, re-fetches EasyList/EasyPrivacy, updates `COMPILED_CONFIG`

### Link Status Bar (Arc-style)
- `LINK_STATUS_SCRIPT` injected into every WebView as an initialization script
- Fixed bottom-left frosted-glass pill; fades in on link hover, fades out on mouse-leave
- Shows raw URL while hovering; "Open in new tab: [url]" while ⌘ held; "Open in new tab → switch: [url]" while ⌘⇧ held
- Max width 55vw, text truncated with ellipsis; `pointer-events:none` so it never blocks clicks
- Idempotent via `window.__epocaStatus` guard (safe across SPA navigations)

### URL Bar & Navigation
- Globe icon with shield badge lives inside the Input border as a prefix (via `Input::prefix()`)
- Shield badge color: green (#44bb66) = blocking on; red (#cc4444) = site excepted
- URL bar shows current tab's URL; updates on tab switch and navigation
- Hand cursor (pointer) on all `<a href>` elements via `a[href]{cursor:pointer!important}` in SCROLLBAR_CSS_SCRIPT

### Tab Isolation ("Fresh Tab" mode)
- `Workbench.isolated_tabs: bool` (default: `false`)
- When `true`, every new WebView tab is opened with `.with_incognito(true)` on the wry WebViewBuilder
- On macOS this uses `WKWebsiteDataStore.nonPersistentDataStore()` — no cookies, localStorage, IndexedDB, or cache shared between tabs or carried across sessions
- Effect: metered paywalls reset per-tab (no cross-tab article count), login sessions are tab-local, fingerprinting resets each tab
- Toggle at runtime via `Workbench::set_isolated_tabs(bool)`; applies to all subsequently opened tabs
- Existing open tabs are unaffected — they keep their data store until closed and reopened
- `WebViewTab::new(url, isolated, window, cx)` — `isolated` param is always passed from the owning Workbench

### Favicon in Tab List
- `FAVICON_SCRIPT` init script finds the best `<link rel="icon">` URL (prefers higher resolution) and falls back to `/favicon.ico`; posts `{type:'faviconFound', url}` to `epocaFavicon` on DOMContentLoaded and SPA navigations
- `EpocaFaviconHandler` ObjC class (`register_favicon_handler(uc, webview_ptr)` in shield.rs); routes via UC_MAP; sends to `FAVICON_CHANNEL`
- `TabEntry.favicon_url: Option<String>` updated by `drain_favicon_events()` in `process_pending_nav`
- Tab row renders `img(url).size(px(13)).rounded(px(2))` when favicon available; falls back to `Icon::new(icon)` otherwise

### Tab Title Tracking
- Sidebar tab entries show the live page title instead of the URL slug
- `TITLE_TRACKER_SCRIPT` (init script in every WebView): monitors `document.title` via MutationObserver on `<title>`, `DOMContentLoaded`, `load`, and SPA `pushState`/`replaceState`/`popstate` hooks; posts `{type:'titleChanged', title}` to `window.webkit.messageHandlers.epocaMeta`
- `EpocaMetaHandler` ObjC class (`register_meta_handler(uc, webview_ptr)` in shield.rs): receives messages, uses `UC_MAP` to route by `WKUserContentController` pointer → `webview_ptr`
- `TITLE_CHANNEL` static in shield.rs: `SyncSender<(usize, String)>` / `Mutex<Receiver<...>>`, drained each frame by `drain_title_events()`
- `WebViewTab.webview_ptr: usize` stores the raw WKWebView pointer as identity key
- `Workbench::process_pending_nav` drains title events and updates `TabEntry.title` on match
- Idempotent via `window.__epocaTitleTracker` guard (safe across SPA navigations)

### URL Bar Triple-Click
- Triple-click in the URL bar selects the entire URL (standard browser behavior)
- `on_mouse_down` on the url_row div checks `click_count >= 3`, dispatches `gpui_component::input::SelectAll`

### Link Cursor (Hand Pointer)
- GPUI overrides WKWebView's CSS cursor every paint frame — CSS `cursor:pointer` has no effect
- `CURSOR_TRACKER_SCRIPT` tracks `mouseover`/`mouseout` on `<a>` elements, posts `{pointer: bool}` to `epocaCursor`
- `EpocaCursorHandler` ObjC class → `CURSOR_CHANNEL` → `drain_cursor_events()` → `WebViewTab.cursor_pointer: bool`
- `WebViewTab::render()` uses `.when(self.cursor_pointer, |d| d.cursor_pointer())` on the GPUI wrapper div
- Entity update closure MUST call `ecx.notify()` when changing `cursor_pointer` — otherwise GPUI won't re-render

### Right-Click Link Context Menu
- `CONTEXT_MENU_SCRIPT` (init script) intercepts `contextmenu` on `<a>` elements, `preventDefault()`, posts `{href, text, x, y}` to `epocaContextMenu`
- `EpocaContextMenuHandler` ObjC class (`register_context_menu_handler(uc, webview_ptr)` in shield.rs), routes via `UC_MAP`, sends to `CONTEXT_MENU_CHANNEL`
- Native NSMenu with: **Open in New Tab** (`open_webview_background` + ripple), **Open in New Window** (`open_webview` foreground, true new-window deferred), **Open in Context ▸** (submenu per named context, experimental_contexts), **Copy Link Address** (`cx.write_to_clipboard`)
- `EpocaMenuTarget` ObjC class with action selectors routes via `MENU_ACTION_CHANNEL` → `drain_menu_actions()` in `process_pending_nav`
- **Ripple on "Open in New Tab"**: `set_menu_origin()` stores click position before NSMenu shows; `take_menu_origin()` retrieves it when OpenInNewTab fires; `trigger_ripple()` evaluates ripple JS on the source WebView (same visual as cmd-click ripple)
- Non-link right-click falls through to native WKWebView context menu

### Sidebar
- Pinned mode: sidebar in flex flow, fixed width
- Overlay mode: sidebar slides in over content; hides on mouse-out
- Fullscreen: mini toolbar with pin button shown when sidebar hidden (no traffic-light trap)
- WKWebView masked via CALayer (no page reflow) when sidebar overlaps content

---

---

## Changelog & Release Notes Process

`CHANGELOG.md` at the repo root is the living record of what shipped and when.

### When a feature lands
1. Verify it compiles and works (`cargo build` passes, manual smoke test if possible).
2. Add a bullet to `CHANGELOG.md` under `## [Unreleased]` with the date in parentheses:
   ```
   - **Feature name** — one-sentence description. Key types/files if non-obvious. (YYYY-MM-DD)
   ```
3. Add or update an entry in the **Implemented Features** section above so it's covered by
   the no-regression policy.

### When cutting a release
1. Rename `## [Unreleased]` to `## [x.y.z] — YYYY-MM-DD`.
2. Add a fresh empty `## [Unreleased]` section above it.
3. Bump the version in `Cargo.toml` (workspace root) to match.
4. Commit with message `chore: release vx.y.z`.

### What goes in a changelog entry
- **Do include**: what the user can do that they couldn't before, key implementation types
  (so future agents can find the code), date landed.
- **Don't include**: refactors with no user-visible effect, dependency bumps, doc fixes.

---

## Regression Prevention Policy
- **Before deleting code**: check the Implemented Features list above. If the code implements
  a listed feature, don't delete it.
- **After a context-window reset**: re-read this file before assuming what is or isn't
  implemented. The previous session's *summary* may describe planned work that wasn't yet
  committed. Always `grep` for the relevant function/const to confirm it exists.
- **New features**: add an entry to both CHANGELOG.md and the Implemented Features list
  above when the feature lands and compiles cleanly.
