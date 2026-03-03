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

### Content Blocking (epoca-shield)
- EasyList / EasyPrivacy rules compiled to WKContentRuleList JSON and installed on each WebView
- Per-site exceptions: Eye/EyeOff toggle in URL bar popover calls `toggle_site_exception()`
- Shield blocked-count displayed in URL bar via `epocaShield` WKScriptMessageHandler
- Cosmetic hiding (document_end_script): overlay sweeper, cookie consent auto-dismiss

### URL Bar & Navigation
- Globe icon with shield badge lives inside the Input border as a prefix (via `Input::prefix()`)
- Shield badge color: green (#44bb66) = blocking on; red (#cc4444) = site excepted
- URL bar shows current tab's URL; updates on tab switch and navigation
- Hand cursor (pointer) on all `<a href>` elements via `a[href]{cursor:pointer!important}` in SCROLLBAR_CSS_SCRIPT

### Sidebar
- Pinned mode: sidebar in flex flow, fixed width
- Overlay mode: sidebar slides in over content; hides on mouse-out
- Fullscreen: mini toolbar with pin button shown when sidebar hidden (no traffic-light trap)
- WKWebView masked via CALayer (no page reflow) when sidebar overlaps content

---

## Regression Prevention Policy
- **Before deleting code**: check this list. If the code implements a listed feature, don't delete it.
- **After a context-window reset**: re-read this file before assuming what is or isn't implemented.
  The previous session's *summary* may describe planned work that wasn't yet committed.
  Always `grep` for the relevant function/const to confirm it exists before treating it as done.
- **New features**: add an entry here when the feature lands and compiles cleanly.
