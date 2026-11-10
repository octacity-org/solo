# Native web views

`<webview>` embeds a real web view inside a Solo window: a WKWebView on
macOS, a WebKitGTK window on Linux/X11. Solo owns layout; the web engine
owns pixels and input. The view sits above GPUI's surface in the window's
own X child (or NSView subtree) and is positioned from GPUI's layout bounds
during each paint — window resizes and sibling-driven reflows move it
automatically.

```tsx
import { View } from "@solo/solid"

function Pane() {
  return (
    <webview
      url="https://example.com"
      userAgent="MyApp/1.0"
      onNavigation={(event) => setStatus(`navigating: ${event.value ?? ""}`)}
      onLoad={(event) => setStatus(`loaded: ${event.value ?? ""}`)}
      onLoadError={(event) => setStatus(`failed: ${event.value ?? ""}`)}
      style={{ flexGrow: 1, minHeight: 0 }}
    />
  )
}
```

## Props

- `url` or `html` (with optional `baseUrl`): the source. They are mutually
  exclusive; `baseUrl` requires `html`.
- `userAgent`: a custom user agent, applied immediately.

The view sends `navigation`, `load` and `loadError` events with the URL, as
on macOS. Solo's host installs a controller the app can also drive directly
(`ready()`, `evaluateJavaScript(script)`, `allowNavigation(id)` /
`cancelNavigation(id)`) — see `packages/solid/src/webview.ts`.

## Platform support

- macOS: WKWebView as an AppKit subview.
- Linux (X11): a WebKitGTK child window. GTK runs on its own thread, so a
  build that includes `<webview>` links the system libraries `gtk-3`,
  `libwebkit2gtk-4.1` and the JavaScriptCore runtime; the `webview-linux`
  Cargo feature is opt-in so other apps need none of them. Builds without
  it report a `loadError` (see [linux-packaging.md](linux-packaging.md)).
- Wayland: embedding another toolkit's surface is not possible, and WPE
  off-screen rendering is not implemented. The placeholder reports a clear
  error via `onLoadError` instead of painting nothing.

Two intentional differences from macOS on Linux:

- A WebKitGTK document that starts loading also emits the matching
  `navigation`/`load` for the initial blank page, which macOS does not.
- `evaluateJavaScript` results are delivered through a script message (the
  binding cannot unwrap returned Promises), and navigation decisions are
  fire-and-forget: `cancelNavigation` for an already-decided request is
  dropped instead of reported.
- `element.click()` on an anchor and a real in-page link click are both
  link-clicked activations, so both go through `onNavigationRequest`; the
  bindings do not expose the target frame, so `isNewWindow` is always
  `false` on Linux (new-window destinations never exist because Solo does
  not implement `create-web-view`).

## Lifecycle

Unmounted (or zero-area) views are destroyed or hidden rather than left
mounted: WebKit keeps compositing layers alive for an empty frame. Quitting
the app destroys every view. Reacting to the `url`/`html` props never
rebuilds the native view, so an in-place change does not lose scroll or
history.
