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
`width`, `height`, …) except `menuBar`, plus `onClose`.

The returned `AppWindow` controls that window:

| member | behavior |
|---|---|
| `id` | the window's root element ID |
| `closed` | true once the window closed, by the user or through `close()` |
| `close()` | closes the native window and disposes its tree; a no-op once closed |
| `focus()` | brings the window to the front |
| `setTitle(title)` | sets the title |
| `maximize()`, `restore()`, `toggleMaximized()`, `isMaximized()` | maximized state |

Calls other than `close()` throw after the window has closed.

## Lifetime

- A window's tree is its own Solid root. It is not owned by the component
  that opened it, so it stays open when that component unmounts; close it
  explicitly when it should not.
- When the user closes the window, Solo disposes its tree (running its
  `onCleanup` handlers) and then calls `onClose`.
- `render()` again, or `unmount()` on the render root, closes every window
  opened with `openWindow`.

## Limits

- macOS only. On Linux and Windows `openWindow` throws.
- New windows open centered on the screen.
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
