# Multiple windows (macOS)

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
| `size()` | the content size in logical pixels, read from the live window |

Calls other than `close()` throw after the window has closed. The render root
reads the main window's size with `getWindowSize()`.

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
| `tabbingIdentifier` | none | windows sharing an identifier can merge into native tabs |
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
| `id` | the display's UUID, stable across reconnects and restarts; store it to reopen a window on the same screen |
| `bounds` | the whole screen in points, starting at (0, 0) |
| `visibleBounds` | the part windows can use, without the menu bar and Dock, in the same coordinates |
| `primary` | true for the screen holding the menu bar |

Rectangles are measured from each screen's own top-left corner, as GPUI
reports them, so they do not show how screens are arranged relative to one
another.

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


- A window's tree is its own Solid root. It is not owned by the component
  that opened it, so it stays open when that component unmounts; close it
  explicitly when it should not.
- When the user closes the window, Solo disposes its tree (running its
  `onCleanup` handlers) and then calls `onClose`.
- `render()` again, or `unmount()` on the render root, closes every window
  opened with `openWindow`.

## Limits

- macOS only. On Linux and Windows `openWindow` throws.
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
