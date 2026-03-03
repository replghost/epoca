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
