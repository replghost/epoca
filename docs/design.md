# Epoca Design System

Locked-in visual constraints for the workbench UI.
**Do not change these values without explicit user approval.**

## Chrome & Window

| Token | Value | Location |
|-------|-------|----------|
| Window chrome bg | `rgb(0x2b2b2b)` | `workbench.rs` `chrome_bg` |
| Chrome inset (all sides) | `10.0 px` | `workbench.rs` `CHROME` |
| Chrome border radius | `10.0 px` | `workbench.rs` `RADIUS` |
| macOS traffic light position | `x=18, y=12` | `main.rs` `traffic_light_position` |

The content viewport is inset `CHROME` px on all sides inside `chrome_bg`, with `rounded(RADIUS)` and `overflow_hidden`.

## Sidebar Panel

| Token | Value | Location |
|-------|-------|----------|
| Sidebar width | `260.0 px` | `workbench.rs` `SIDEBAR_W` |
| Hover trigger zone | `8.0 px` | `workbench.rs` `EDGE_ZONE` |
| Animation ease factor | `0.22` per frame | `workbench.rs` `ANIM_EASE` |

### Pinned mode
- Sidebar is in the flex flow, pushes content right
- No top/bottom margin — full window height
- `CHROME` padding only on top/right/bottom of content (left side provided by sidebar)
- Traffic lights always visible

### Overlay mode
- Sidebar slides in as an absolute floating panel
- **Top margin: `4 px`**, bottom margin: `8 px` — creates the floating/modal look
- Panel bg: `rgb(0x424242)` (noticeably lighter than chrome to read as a distinct surface)
- Panel border: `border_1` + `rgba(0xffffff1e)`
- Panel rounded corners: `rounded(RADIUS)` on all 4 sides
- Traffic lights hidden when sidebar is fully hidden; revealed as sidebar animates in
- `OverlayLeftInset` global published so `WebViewTab` shifts its native WKWebView right

### Overlay mode — No Content Shift Rule (ENFORCED)

**Design intent**: When the overlay sidebar appears, the web content MUST NOT
shift right. The sidebar slides over the content as a pure modal, like a drawer.

**How it works (CALayer mask approach)**:
`WebViewTab` applies a `CALayer.mask` to the WKWebView's backing layer whenever
`OverlayLeftInset` changes. The mask clips the WKWebView's rendering to
`x = OverlayLeftInset..width` in the WKWebView's own coordinate space.

- WKWebView **frame is unchanged** → page viewport is unchanged → no reflow ✓
- WKWebView is **visually absent** from `x = 0..OverlayLeftInset` → GPUI's
  Metal layer (which renders the sidebar) is visible in that region ✓
- No content shift ✓

**`OverlayLeftInset` formula**: `(SIDEBAR_W × anim − CHROME).max(0)` — accounts
for the chrome inset so the mask aligns with the sidebar's right edge in
WKWebView-local coordinates.

**Do NOT** convert `OverlayLeftInset` back into a padding or frame shift.
The mask approach is the correct mechanism; shifting the frame causes content
reflow (viewport change) and is the original regression. The no-shift rule is
authoritative.

### Fullscreen mode

- Traffic lights are managed by macOS in fullscreen — **never call
  `set_traffic_lights_hidden(true)` when `is_window_fullscreen()` returns true**.
  Hiding them in fullscreen leaves the user with no way to exit.
- In fullscreen + overlay mode (sidebar hidden), a mini toolbar is rendered at
  `top: SIDEBAR_TOP, left: 0` containing the traffic-light spacer (68 px) and the
  sidebar pin button, so the user can toggle the sidebar from the fullscreen hover
  zone without moving the mouse to the left edge.
- The macOS system menubar appears in fullscreen when the user hovers at the top of
  the screen; this requires the app to not block the system's titlebar hover zone.

## Sidebar Top Row (toolbar)

Traffic light alignment rule:
- macOS traffic light center ≈ `y = traffic_light_position.y (12) + radius (6) = 18 px`
- **Pinned mode**: row `h=38 px`, panel flush at `y=0` → icon center `= 0 + 19 = 19 px` ✓
- **Overlay mode**: row `h=28 px`, panel top at `y=4` → icon center `= 4 + 14 = 18 px` ✓

Layout within the row:
- `68 px` spacer reserves the traffic-light zone
- Sidebar pin/unpin button (PanelLeft) follows immediately
- **Pinned**: nav buttons follow pin button with `gap(2px)` — all left-aligned
- **Overlay**: `flex_1` spacer between pin and nav buttons — nav buttons pushed to right edge

## Sidebar Content Spacing

| Element | Value |
|---------|-------|
| Tab item height | `28 px` |
| Tab item border radius | `5 px` |
| Tab item horizontal padding | `pl=10, pr=2` |
| Gap between tab items | `2 px` |
| Tabs area top padding | `4 px` |
| URL bar horizontal margin | `mx=8` |
| URL bar top margin | `mt=4` |
| URL bar bottom margin | `mb=10` |
| URL bar border radius | `8 px` |
| Section divider vertical margin | `my=4` |
| Pinned section spacer below items | `8 px` |
| Bottom toolbar vertical padding | `py=6` |

## Sidebar Colors (dark theme)

| Role | Value |
|------|-------|
| URL bar background | `rgba(0xffffff14)` |
| Item active background | `rgba(0xffffff1c)` |
| Item hover background | `rgba(0xffffff0f)` |
| Text active | `rgba(0xffffffff)` |
| Text normal | `rgba(0xffffffcc)` |
| Text muted | `rgba(0xffffff66)` |
| Icon active | `rgba(0xffffffcc)` |
| Icon muted | `rgba(0xffffff66)` |
| Divider | `rgba(0xffffff14)` |

## Scrollbar (WebKit CSS injection)

Injected via `SCROLLBAR_CSS_SCRIPT` in `tabs.rs` at webview initialization.

| State | Dark mode | Light mode |
|-------|-----------|------------|
| Track | `rgba(15,15,15,0.6)` | `rgba(200,200,200,0.4)` |
| Thumb | `rgba(130,130,130,0.75)` | `rgba(80,80,80,0.45)` |
| Thumb hover | `rgba(180,180,180,0.9)` | `rgba(50,50,50,0.65)` |
| Width/height | `8 px` | `8 px` |
| Border radius | `4 px` | `4 px` |

## WKWebView Corner Radius (macOS)

Applied via `apply_webview_corner_radius` (objc2 CALayer):
- Radius: `10.0` — matches `RADIUS` so the native view clips to the same rounded rect
- `setWantsLayer: YES` + `setMasksToBounds: YES` required

## Navigation / URL Heuristic

- `http://` or `https://` prefix → direct `open_webview`
- Path that exists on disk → open as file (`.toml`/`.zml` → declarative, `.polkavm` → sandbox)
- No spaces + contains `.` or `:` → prepend `https://` → `open_webview`
- Otherwise (spaces or no TLD) → DuckDuckGo search: `https://duckduckgo.com/?q=<encoded>`
