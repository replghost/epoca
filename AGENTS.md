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

### Browsing History (Ephemeral with TTL)
- SQLite-backed history at `~/.epoca/history.db` (WAL journal, auto_vacuum=INCREMENTAL)
- `HistoryRetention` enum: `SessionOnly` (in-memory), `Hours8`, `Hours24` (default), `Days7`, `Days30`
- `HistoryManager`: `record_visit()` (upsert with visit_count++), `update_title()`, `cleanup_expired()` (hard DELETE + incremental_vacuum), `search()` (frecency-ordered LIKE match, limit 8)
- `HistoryGlobal` GPUI global; `init_history(cx)` reads retention from settings, opens DB, runs initial cleanup
- Corrupt DB handling: if CREATE TABLE fails, delete file and retry; if retry fails, fall back to in-memory
- File permissions: 0600 on Unix after DB creation
- Omnibox integration: `on_omnibox_input_event` fires `search()` on `InputEvent::Change`, caches results in `omnibox_history_results`; `render_omnibox()` renders "History" divider + two-line rows (title + URL) with Globe icon
- Visit recording: `record_history_visit()` called in `open_webview()`, `open_webview_background()`, in-place URL bar nav, search nav; `update_title()` called in title drain block
- Hourly cleanup timer: `_history_cleanup: Option<Task<()>>` in Workbench, runs `cleanup_expired()` every hour
- Settings UI: "History Retention" pill-button row in Privacy section; click updates setting and re-inits history
- Switching from SessionOnly → disk mode intentionally loses in-memory history (privacy-first design)

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

### Session Contexts (experimental)
- `experimental_contexts` setting toggle; `SessionContext { id, name, color }` in settings.rs
- **Context picker dropdown** in URL bar: colored dot prefix, click to open. Shows Private option, all named contexts, "+ New Context" row
- `active_context: Option<String>` on Workbench — synced from WebView tabs on switch, not from non-WebView tabs
- `resolve_context_id()` determines context for new tabs; `active_tab_context_id()` for background opens
- `switch_context()` reads live URL from url_input, closes+reopens tab in new context (WKWebView data stores are immutable after creation)
- `create_new_context()` picks first unused color from `DEFAULT_CONTEXT_COLORS` palette
- **Right-click "Open in Context"** submenu: "Private Tab" (always shown) + named contexts. `MenuAction::OpenInContext`, `MenuAction::OpenPrivate`
- **Orphan cleanup**: `process_pending_nav` checks tabs against valid context IDs, resets orphaned `context_id` to `None`
- Context dropdown backdrop rendered at root Workbench level (covers full window); auto-closes on sidebar hide

### Sidebar
- Pinned mode: sidebar in flex flow, fixed width
- Overlay mode: sidebar slides in over content; hides on mouse-out
- Fullscreen: mini toolbar with pin button shown when sidebar hidden (no traffic-light trap)
- WKWebView masked via CALayer (no page reflow) when sidebar overlaps content

### Session Restore
- Saves open tabs to `~/Library/Application Support/Epoca/session.json` every 30s + on ⌘Q
- Atomic writes: `.json.tmp` → `fs::rename` prevents corruption on crash
- Restores WebView, Settings, CodeEditor, DeclarativeApp tabs; skips SandboxApp, FramebufferApp, Welcome
- Preserves: URL, title, pinned state, favicon_url, context_id, active_tab_index, next_tab_id, active_context, isolated_tabs
- `WorkbenchRef` GPUI global (WeakEntity) set in main.rs; Quit handler calls `save_session()` synchronously before `cx.quit()`
- `session.rs`: `SessionTab`, `SessionState`, `save_session()`, `load_session()`, `is_restorable()`
- On launch with no CLI arg: `restore_session()` → falls back to `new_tab()` if no session
- v1 saves only the most recently saved window (multi-window deferred)

### Find-in-Page (⌘F)
- `FindInPage` action bound to `cmd-f`; toggles find bar open/closed
- Find bar rendered between chrome padding and content area (not overlaid on WKWebView)
- Live search: `InputEvent::Change` → `window.find(query, false, backwards, true)` JS API
- Enter → next match, Shift+Enter or ↑ button → previous match
- Escape or ✕ button → close find bar, clear highlights via `window.getSelection().removeAllRanges()`
- No-op on non-WebView tabs (Settings, CodeEditor, etc.)
- Edit menu with Find entry in macOS menu bar
- `FindPrev`, `CloseFindBar` actions; find input with search icon prefix

### Embedded Test Server (feature-gated)
- `#[cfg(feature = "test-server")]` + `EPOCA_TEST=1` env var
- HTTP on `localhost:9223`: `GET /state`, `POST /action`, `GET /webview/eval?js=X`
- `TestCommand` enum + channel drain in `process_pending_nav`; `AppSnapshot`/`TabSnapshot` types
- JS eval uses correlation IDs via `epocaTestResult` WKScriptMessageHandler
- `tools/test_cursor.sh` smoke test; `tools/move_mouse.swift` CGEvent helper

---

---

## Changelog & Release Notes Process

`CHANGELOG.md` at the repo root is the living record of what shipped and when.

### When any work lands (features, fixes, improvements)
1. Verify it compiles and works (`cargo build` passes, manual smoke test if possible).
2. **Immediately** add a bullet to `CHANGELOG.md` under `## [Unreleased]` with the date.
   Use the appropriate subsection (`Added`, `Fixed`, `Changed`, `Removed`):
   ```
   - **Feature/fix name** — one-sentence description. Key types/files if non-obvious. (YYYY-MM-DD)
   ```
3. Add or update an entry in the **Implemented Features** section above so it's covered by
   the no-regression policy.
4. **This is not optional.** Every code change — whether a new feature, a bug fix, a UX
   improvement, or a refactor with user-visible effect — must be logged before moving on
   to the next task. Do not batch logging for later; log each change as it lands.

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

## Feature Development Process

When implementing a non-trivial feature (new module, multi-file change, user-facing behavior):

1. **Plan** — Design the implementation approach (plan mode or manual).
2. **Architect Review** — Before writing code, have the `software-architect` agent review the plan for correctness, security, and alignment with existing patterns.
3. **QA Test Strategy** — Have the `QA-expert` agent define the testing approach (unit tests, integration tests, manual verification steps). Tests may be written before or after implementation depending on the feature — QA decides.
4. **Implement** — Write the code following the reviewed plan.
5. **Tech Lead Code Review** — After implementation, have the `tech-lead` agent review the code for quality, security, and correctness.
6. **QA Code Check** — After implementation, have the `QA-expert` agent verify test coverage, edge cases, and that the implementation matches the plan.
7. **Fix** — Address any findings from steps 5-6 before considering the feature complete.

For small fixes (typos, single-line changes, obvious bugs), skip this process.

---

## Regression Prevention Policy
- **Before deleting code**: check the Implemented Features list above. If the code implements
  a listed feature, don't delete it.
- **After a context-window reset**: re-read this file before assuming what is or isn't
  implemented. The previous session's *summary* may describe planned work that wasn't yet
  committed. Always `grep` for the relevant function/const to confirm it exists.
- **New features**: add an entry to both CHANGELOG.md and the Implemented Features list
  above when the feature lands and compiles cleanly.
