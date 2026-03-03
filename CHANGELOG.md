# Changelog

All notable changes to Epoca are recorded here.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [Unreleased] — ongoing

### Added
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

---

## [0.0.1] — 2026-02-12

### Added
- **Workbench shell** — GPUI-based browser chrome with pinned and overlay sidebar modes.
  Sidebar animates in/out; traffic lights positioned inside the sidebar panel.
  Fullscreen-safe: sidebar hides cleanly, mini toolbar with pin button shown in fullscreen.

- **WebView tabs** — WKWebView-backed browser tabs via wry. CALayer mask clips rendering
  to sidebar-free region without changing the page viewport (no content reflow).
  `EpocaSidebarBlocker` NSView intercepts hit-testing in the sidebar overlay region.

- **Content blocking (epoca-shield)** — Six-layer pipeline: DNS CNAME uncloaking,
  WKContentRuleList (EasyList + EasyPrivacy, 45k-rule bucket splitting), NavigationDelegate,
  document_start JS (fingerprint countermeasures), document_end JS (overlay sweeper, cookie
  consent auto-dismiss), GPUI shield UI. Per-site exception toggle (Eye/EyeOff in URL bar
  popover). Shield badge (green/red) on globe icon inside URL bar. Blocked count via
  `epocaShield` WKScriptMessageHandler. 6-hour background list refresh.

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
