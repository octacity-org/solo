# Menu-bar applications

Pass `menuBar` to `render()` to keep the application and its JavaScript work
alive after the last window closes. Without this option, Solo retains its
existing behavior and exits when the last window closes.

```tsx
import { render } from "@solo/solid"

const application = render(() => <App />, {
  title: "Newsprint",
  menuBar: {
    // Resolve this inside the packaged application. Relative paths are rejected.
    iconPath: "/Applications/Newsprint.app/Contents/Resources/menu-bar.png",
    tooltip: "Newsprint",
  },
})
```

The status-item menu contains `Open <title>` and `Quit <title>` actions. Open
activates the existing window or creates exactly one replacement; Quit
terminates the native event loop and JavaScript process.

The platform decides where the item lives:

| macOS | Linux |
|---|---|
| an `NSStatusItem` in the system menu bar | a StatusNotifierItem on the session bus |

On Linux nothing shows unless a status-notifier host is running: KDE Plasma
supports the item natively, XFCE through its status notifier panel plugin, and
GNOME through the AppIndicator and KStatusNotifierItem extensions (Fedora and
Arch package them). Without a host, `render()` throws, because a background
app whose only entry point is a missing icon could not be reached again — Solo
does not run hidden.

An icon is required on both platforms. macOS treats it as an AppKit template
image, so a monochrome asset adapts to light and dark menu bars; the
StatusNotifierItem specification has no template images, so on Linux the PNG
is shown as drawn. There is no dark-panel variant yet: use an icon that reads
on both, or set a different `iconPath` from `configureMenuBar` when your app
knows the panel's appearance.

Use `configureMenuBar` inside the mounted Solid owner to prepend reactive,
application-defined controls and update the icon or tooltip at runtime:

```tsx
import { configureMenuBar } from "@solo/solid"

function App() {
  configureMenuBar(() => ({
    iconPath: refreshing() ? refreshingIcon : idleIcon,
    tooltip: error() ?? "Newsprint",
    items: [
      {
        id: "refresh",
        label: refreshing() ? "Refreshing…" : "Refresh now",
        enabled: !refreshing(),
        run: refresh,
      },
      { type: "separator" },
      { type: "status", label: `Last refresh: ${lastRefresh()}` },
    ],
  }))
  return <Newsprint />
}
```

Action items support native disabled and checked states. Status items are
non-actionable text. Solo keeps its built-in Open and Quit entries after the
custom items. When the owning component unmounts, it drops the callbacks and
restores the original icon, tooltip, and default menu.

On Linux, `_` in a label is the menu's access-key marker, so Solo doubles it;
`Refresh_now` shows as `Refresh_now` and not as `Refreshnow`.

The render root exposes the same lifecycle controls for application UI:

```ts
application.closeWindow()
application.showWindow()
application.quitApplication()
```

The same root exposes the standard macOS zoomed state separately from
fullscreen:

```ts
application.maximizeWindow()
application.isWindowMaximized() // reads AppKit's live state
application.restoreWindow()
application.toggleWindowMaximized()
application.getWindowSize() // content size in logical pixels
```

These methods use `NSWindow`'s native zoom operation, so restoring returns to
the frame macOS saved before maximizing. A change made with the green window
button is visible through `isWindowMaximized()` as well. Fullscreen,
minimizing, the background and resizing are in
[multiple-windows.md](multiple-windows.md#window-state).

## State across reopening

Solo keeps the Solid owner, signals, timers, registered application commands,
retained element tree, and system-appearance subscription alive while no window
exists. Mutations continue updating that retained tree and the reopened window
paints its latest state.

A reopened window is a new native GPUI window. Window-local state therefore
resets: native focus, scroll handles, transient pointer state, and native child
views such as `WKWebView` instances are recreated. Applications that need to
restore focus or scroll position should keep those values in Solid state.

## Getting the window back

Open in the item's menu is enough for an app whose window needs no arguments.
Apps that want the platform's usual gestures — clicking the Dock icon on
macOS, clicking the status-item icon on Linux — can listen for the reopen
event instead:

```ts
import { onApplicationReopen } from "@solo/solid"

onApplicationReopen(() => root.showWindow())
```

This capability keeps the process alive after window close; it does not
survive explicit Quit and does not provide launch-at-login behavior.
