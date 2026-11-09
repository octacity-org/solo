# Multiple windows

`render()` starts the application and opens its main window. `openWindow()`
opens further windows in the same application, after `render()` has run:

```tsx
import { createSignal } from "solid-js"
import { Button, openWindow, render, Text, View } from "@solo/solid"
import type { AppWindow } from "@solo/solid"

const [selection, setSelection] = createSignal("none")
let inspector: AppWindow | undefined

function showInspector() {
  if (inspector && !inspector.closed) return inspector.focus()
  inspector = openWindow(() => <Text>Selected: {selection()}</Text>, {
    title: "Inspector",
    width: 320,
    height: 240,
    onClose: () => { inspector = undefined },
  })
}

render(() => <Button onClick={showInspector}>Inspect</Button>, { title: "Editor" })
```

Every window belongs to one renderer, so signals, stores and context-free
modules are shared: a signal write repaints each window that reads it, in the
same batch. `openWindow` takes the window options of `render()` (`title`,
`width`, `height`, …) except `menuBar`, plus `onClose` and `onCloseRequest`.

The returned `AppWindow` controls that window:

| member | behavior |
|---|---|
| `id` | the window's root element ID |
| `closed` | true once the window closed, by the user or through `close()` |
| `close()` | closes the native window and disposes its tree; a no-op once closed |
| `requestClose()` | asks to close, as the close button does; see [Close requests](#close-requests) |
| `focus()` | brings the window to the front |
| `setTitle(title)` | sets the title |
| `maximize()`, `restore()`, `toggleMaximized()`, `isMaximized()` | maximized state |
| `enterFullscreen()`, `exitFullscreen()`, `toggleFullscreen()`, `isFullscreen()` | native fullscreen; see [Window state](#window-state) |
| `minimize()` | minimizes the window into the Dock |
| `setBackground(background)` | `"opaque"`, `"transparent"` or `"blurred"` |
| `size()` | the content size in logical pixels, read from the live window |
| `resize({ width, height })` | sets the content size `size()` reports |
| `setTabbingIdentifier(identifier)`, `toggleTabOverview()` | window tabs; see [Window tabs](#window-tabs) |
| `setDocumentEdited(edited)`, `setDocumentPath(path)` | unsaved-changes marker and title-bar file icon; see [Document windows](#document-windows) |

Calls other than `close()` throw after the window has closed. The render root
has the same controls for the main window; see [Window state](#window-state).

## Window options

Besides `title`, `width`, `height`, the minimum size, `resizable` and the
titlebar options, `render()` and `openWindow()` accept:

| option | default | behavior |
|---|---|---|
| `focus` | `true` | make the window key and activate the application |
| `show` | `true` | order the window on screen when it opens |
| `kind` | `"normal"` | `"normal"`, `"floating"` (stays above normal windows) or `"popup"` (a non-activating panel) |
| `movable` | `true` | let the user drag the window |
| `minimizable` | `true` | enable the minimize button |
| `tabbingIdentifier` | none | windows sharing an identifier can merge into native tabs; see [Window tabs](#window-tabs) |
| `useDisplayP3` | `false` | render in the Display P3 color space |
| `screen` | the main screen | open on this screen, an `id` from `getScreens()` |

With `focus: false` or `show: false` Solo neither activates the application
nor brings the window forward, so the window opens without taking focus from
the frontmost app. `showWindow()` and `focus()` still activate.

## Screens

`getScreens()` lists the connected screens. It works before `render()`, so
the main window can choose its screen too:

```tsx
import { getScreens, render } from "@solo/solid"

const saved = loadSavedScreenId() // your app's storage
const screens = getScreens()
const screen = screens.find((s) => s.id === saved) ?? screens.find((s) => s.primary)

render(() => <App />, { title: "Editor", screen: screen?.id })
```

Each screen has:

| field | meaning |
|---|---|
| `id` | the display's UUID, or `x11-<root index>` on X11 (gpui has no per-screen UUID there); stable across reconnects and restarts, so store it to reopen a window on the same screen |
| `bounds` | the whole screen in points, starting at (0, 0) |
| `visibleBounds` | the part windows can use, without the menu bar and Dock, in the same coordinates |
| `primary` | true for the screen holding the menu bar |

Rectangles are measured from each screen's own top-left corner, as GPUI
reports them, so they do not show how screens are arranged relative to one
another.

On Wayland the id is the output's UUID. On X11 it follows the X server's root
order, which is stable for that server and survives app restarts.

A window opens centered in the visible area of its screen. An `id` that no
connected screen has makes `render()` or `openWindow()` throw, so check a
stored id against `getScreens()` first, as above. A main window reopened later
(a menu-bar app's Open, after its screen was disconnected) falls back to the
main screen.

`onScreensChanged(listener)` calls `listener` when a screen is connected or
disconnected, or changes size, and returns a function that stops listening.
One change can call it more than once, and it carries no details: read
`getScreens()` again.

```ts
import { getScreens, onScreensChanged } from "@solo/solid"

const stop = onScreensChanged(() => {
  const ids = new Set(getScreens().map((screen) => screen.id))
  if (!ids.has(savedScreenId)) savedScreenId = undefined
})
```

## Window state

Fullscreen, minimizing, the background and the size can change while a
window is open. The render root controls the main window; an `AppWindow`
controls its own:

| render root | `AppWindow` |
|---|---|
| `enterFullscreen()`, `exitFullscreen()`, `toggleFullscreen()`, `isFullscreen()` | the same names |
| `minimizeWindow()` | `minimize()` |
| `setWindowBackground(background)` | `setBackground(background)` |
| `getWindowSize()`, `resizeWindow({ width, height })` | `size()`, `resize({ width, height })` |
| `toggleWindowMaximized()`, `isWindowMaximized()` | `toggleMaximized()`, `maximize()`, `restore()`, `isMaximized()` |

```ts
const root = render(() => <App />, { title: "Editor", width: 800, height: 600 })

root.resizeWindow({ width: 1024, height: 700 })
root.getWindowSize() // { width: 1024, height: 700 }
root.setWindowBackground("blurred")
root.toggleFullscreen()
```

- Sizes are the window's content in logical pixels, without the titlebar. A
  resize applies at once, so `getWindowSize()` or `size()` reads it back
  straight away. Both numbers must be positive.
- Fullscreen is macOS native fullscreen: the window moves to its own Space
  in an animation, and `isFullscreen()` reads AppKit's live state. Entering when already fullscreen, or exiting when not, does nothing.
  The `fullscreen` window option opens a window fullscreen.
- `setBackground` takes the values of the `windowBackground` window option
  and applies them to the open window. Any other value throws.
- On Linux all of these reach GPUI's platform window instead of AppKit:
  maximize/minimize go to the window manager, fullscreen tiles the window
  rather than opening a new Space, and `resizeWindow` sets the content area.
  A Wayland compositor applies maximize, minimize and fullscreen
  asynchronously, so `isWindowMaximized()` and `isFullscreen()` can lag the
  request by a frame. A window manager also ignores these requests until the
  window is mapped, so calls made on the first frame do nothing; call them
  once the window is on screen. `setBackground` sets the window's background
  appearance, but GPUI paints the app opaque, so `transparent` and `blurred`
  look like `opaque` there.

## Window tabs

Windows that share a tabbing identifier can merge into one window with native
tabs. Set it when the window opens with the `tabbingIdentifier` option, or
later:

```ts
const root = render(() => <App />, { title: "Untitled", tabbingIdentifier: "documents" })
const second = openWindow(() => <Document />, { title: "Notes" })

second.setTabbingIdentifier("documents")
root.toggleTabOverview() // Show All Tabs
second.setTabbingIdentifier(undefined) // clear it
```

| render root | `AppWindow` |
|---|---|
| `setTabbingIdentifier(identifier)` | the same |
| `toggleTabOverview()` | the same |

- Sharing an identifier makes windows *able* to merge. On macOS, whether a
  new window opens as a tab follows the user's System Settings → Desktop &
  Dock → "Prefer tabs when opening documents": always, only in full screen
  (the default), or never. Windows that do not merge on their own stay
  separate until the user merges them.
- AppKit's automatic window tabbing is one setting for the whole
  application. Setting an identifier turns it on and clearing one turns it
  off, for every window; opening a window with or without
  `tabbingIdentifier` sets it the same way. Mixing tabbed and untabbed
  windows therefore follows whichever call came last.
- `toggleTabOverview()` shows or hides the overview of the tabs the window
  belongs to. Linux draws the tabs in the window itself and has no overview:
  it throws.
- Events for tabs (moved to a new window, all windows merged) are not
  exposed.

On Linux nothing groups windows natively, so Solo draws the tabs. Windows
opened with the same identifier join the first one as tabs of its native
window, "always" with no setting to consult:

```tsx
const editor = openWindow(() => <Editor />, { title: "notes.txt", tabbingIdentifier: "documents" })
openWindow(() => <Preview />, { title: "preview", tabbingIdentifier: "documents" }) // a tab of editor's window
openWindow(() => <Settings />, { title: "Settings" }) // its own window
```

- The first window with an identifier keeps its native window; later ones
  become tabs in it and are selected on open. A window without an identifier
  gets a native window of its own.
- The strip shows each tab's title (with the document marker, see
  [Document windows](#document-windows)) and a close button. Selecting a tab
  paints its tree; the window title follows the selected tab.
- Each tab keeps the window API it would have had alone: `setTitle`,
  `setDocumentEdited`, `close()`, `requestClose()`, `onCloseRequest`, and its
  `windowClosed` event. Closing a tab leaves the window and its other tabs;
  closing the last tab closes the native window.
- The native window's own close button asks the selected tab's
  `onCloseRequest`, then closes the window with all of its tabs.
- `setTabbingIdentifier` decides which windows join later; clearing it keeps
  the tabs the window already has, because moving a tab out into its own
  window is not implemented yet.
- Automation sees the tabs that exist but only the selected tab has painted
  bounds, so `getByTestId()` finds elements of the tab you can see.

## Document windows

A window that edits a file can show it the macOS way:

```ts
const editor = openWindow(() => <Editor />, { title: "notes.txt" })

editor.setDocumentPath("/Users/me/notes.txt") // file icon and path menu in the title bar
editor.setDocumentEdited(true)  // dot in the close button: unsaved changes
editor.setDocumentEdited(false) // saved
editor.setDocumentPath(null)    // clear the file
```

The render root has the same two methods for the main window.

- `setDocumentEdited(true)` marks unsaved changes. macOS puts a dot in the
  close button; Linux has no window property or protocol field for it, so the
  window's title gains a `*` prefix (`*notes.txt`) instead, in the window
  manager's titlebar and in Solo's own. The marker follows `setTitle`, so the
  app keeps setting its plain title. It only shows the state: closing still
  follows `onCloseRequest`, so pair it with a [close request](#close-requests)
  that asks before discarding changes.
- `setDocumentPath(path)` shows the file's icon beside the title. Cmd-clicking
  the title opens a menu of the folders that contain it. The path must be
  absolute, or it throws; `null` clears it. The title stays whatever `title`
  says. Off macOS no window protocol carries the path, so it is only kept for
  `getWindowDocumentPath()`.
- A custom `trafficLightX`/`trafficLightY` position is kept when either
  changes.

## Close requests

By default the close button closes a window. Pass `onCloseRequest` to decide
instead, for example to confirm unsaved changes. The window stays open and
the handler runs; close the window yourself to let it go:

```tsx
import { openWindow, render, showAlert } from "@solo/solid"

const editor = openWindow(() => <Editor />, {
  title: "Draft",
  onCloseRequest: async () => {
    if (!dirty()) return editor.close()
    const choice = await showAlert({
      message: "Discard your changes?",
      buttons: ["Discard", "Cancel"],
      window: editor,
    })
    if (choice === 0) editor.close()
  },
})

const root = render(() => <App />, {
  onCloseRequest: () => { if (!dirty()) root.closeWindow() },
})
```

While a promise returned by `onCloseRequest` is pending, further requests
for that window are ignored, so repeated clicks on the close button do not
stack confirmations.

| path | with `onCloseRequest` |
|---|---|
| the window's close button | calls the handler; the window stays open |
| `requestClose()`, `root.requestCloseWindow()` | the same path as the close button (AppKit's `performClose:`) |
| `close()`, `root.closeWindow()` | closes without asking |
| Cmd+W | not bound by Solo; bind it with `registerApplicationCommand` and call `requestClose()` |
| Quit (Cmd+Q), `quitApplication()` | not covered: quitting does not ask each window |

Without `onCloseRequest`, `requestClose()` simply closes the window.

## Custom titlebars

`titlebarTransparent: true` hides the titlebar and lets the app draw under
the traffic lights. Mark the app-drawn titlebar with `windowDragRegion`, so it
behaves like the system one:

```tsx
render(
  () => (
    <View style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <View style={{ display: "flex", flexDirection: "row", height: 38 }}>
        <View windowDragRegion style={{ flexGrow: 1, paddingLeft: 80 }}>
          <Text>Draft</Text>
        </View>
        <Button onClick={share}>Share</Button>
      </View>
      <Editor />
    </View>
  ),
  { title: "Draft", titlebarTransparent: true, appOwnsTitlebarDrag: true },
)
```

- Dragging the region moves the window. The move starts on the first mouse
  move after the press, so a click without movement still reaches the
  region's own handlers.
- A double-click does what the user chose under System Settings → Desktop &
  Dock → "Double-click a window's title bar to": zoom, fill, minimize or
  nothing. Zooming needs a resizable window, minimizing a minimizable one.
- A press on a child is a press on the region, so pressing a control inside
  it and moving the mouse drags the window. Mark the empty part of the
  titlebar and put controls beside it, as above, rather than inside it.
- `appOwnsTitlebarDrag: true` tells AppKit that the app moves the window.
  Without it AppKit also drags from the hidden titlebar strip at the top of
  the window, and may hold back clicks there for a moment while it waits for
  a possible double-click. Use it with `windowDragRegion`; on its own
  nothing drags the window.

On Linux a drag region works the same way, and there it is the only way to
move a window from app chrome when Solo owns the frame.

## Decorations

Who draws the titlebar and the frame is the window manager's call:

- **Server decorations** (XFCE, KDE, most X11 sessions, a Wayland compositor
  that supports `xdg-decoration`): the window manager draws both, and Solo
  paints exactly the app's own view, sized to the content area. Nothing
  changes for these sessions.
- **Client decorations** (GNOME on Wayland, which has no `xdg-decoration`, and
  any window that asked for them): gpui reports `Decorations::Client` and
  draws nothing itself, so Solo paints a 34px titlebar — window buttons in the
  desktop's own order (`App::button_layout()`, right-hand side by default),
  the window title in a drag region between them, and double-click and
  right-click doing what a titlebar does: zoom the resizable window, or open
  the window menu. Resize strips live on the sides the window manager does not
  own, so a maximized or tiled window is not offered an edge that would do
  nothing.

The close button asks `onCloseRequest` first, so a client-decorated window
closes exactly like a window-manager close.

`titlebarTransparent` asks for client decorations on Linux, since an app that
draws under the titlebar needs the window manager to stop drawing one too. It
also hides Solo's titlebar, leaving the resize strips, so the app's own
chrome is the only titlebar on screen. On a compositor that hands out client
decorations anyway, the app gets Solo's titlebar unless it passes
`titlebarTransparent`.

## Lifetime

- A window's tree is its own Solid root. It is not owned by the component
  that opened it, so it stays open when that component unmounts; close it
  explicitly when it should not.
- When the user closes the window, Solo disposes its tree (running its
  `onCleanup` handlers) and then calls `onClose`.
- `render()` again, or `unmount()` on the render root, closes every window
  opened with `openWindow`.

## Platforms

On Linux every window is a real top-level window, and the surface above works
as it does on macOS, with these differences:

- The window manager owns the close button, or Solo draws one when the
  window is client-decorated (see [Decorations](#decorations)). Either way a
  close request reaches the window as `on_should_close`, so with
  `onCloseRequest` the window stays open and the handler runs, exactly as on
  macOS.
- Closing the last window ends the process, because Linux has no menu-bar app
  lifecycle yet. `showWindow()` still calls `activate` on an open window, and
  reopens the main window while another window keeps the app running.
- `setBackground("transparent" | "blurred")` sets the appearance, but GPUI
  paints the app opaque, so both look like `opaque`.
- `setDocumentEdited` marks the title with a `*` (see
  [Document windows](#document-windows)), and `setDocumentPath` validates and
  stores the path without drawing anything.
- Window tabs are drawn by Solo (see [Window tabs](#window-tabs)); the menu
  bar is still macOS-only.

## Limits

- Windows are macOS and Linux. On Windows `openWindow` throws, and so do the
  render root's `setDocumentEdited` and `setDocumentPath`.
- New windows open centered on their screen; there is no option for an
  exact position yet.
- File dialogs, the debug overlay, screenshots and clock control in the
  automation protocol use the main window.

In tests, `createSolidNativeTestRoot()` from `@solo/solid/testing` hosts
`openWindow` too: each window is an offscreen GPU-backed window, `findByType`
and `getElement` include their elements, and simulation methods that take an
`elementId` act in that element's window. The test renderer cannot close its
main window. See `packages/solid/src/__tests__/native-windows.test.tsx`.

Automation sees every window: `getByTestId` and the other locators search
the main tree and the `openWindow` trees, and clicks and key presses go to the
window that contains the element.
