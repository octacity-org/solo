# Native context menus

`createContextMenu` from `@solo/solid` presents a macOS `NSMenu` with native
placement, keyboard navigation, disabled/checked items, and dismissal.

```tsx
import { createContextMenu, View, Text } from "@solo/solid"

function Article() {
  const menu = createContextMenu([
    { id: "read", label: "Mark read", checked: true },
    { type: "separator" },
    { id: "delete", label: "Delete", disabled: true },
  ], (id) => {
    if (id === "read") markArticleRead()
  })

  return <View onMouseDown={(event) => {
    if (event.button === 2) void menu.show(event).catch(reportError)
  }}><Text>Article title</Text></View>
}
```

Pass a Solo pointer event to position at its window-content coordinates, in
logical points. Pass `{ elementId }` to position below an already-painted
element instead. AppKit handles screen-edge adjustment. A missing or destroyed
element produces an error.

`show()` returns `Promise<string | null>`: the selected action ID, or `null`
for Escape, outside dismissal, replacement, `dismiss()`, or disposal. Errors
reject the promise. The optional callback runs once only for enabled actions.
Use either that callback or the returned ID to execute an action, not both.

Descriptors are copied when the controller is created. To change contents,
dispose it and create another controller. `dismiss()` cancels the current
session while keeping the controller reusable. `dispose()` also prevents
future opening and suppresses late selection callbacks. Controllers created
inside a Solid owner dispose automatically; otherwise dispose them explicitly.
Destroying the owning native element or closing its window cancels its menu.
Only one context menu tracks per app at a time; opening another cancels the
previous session. Unsupported platforms report a missing renderer capability.
Menus support up to 256 entries; IDs are limited to 1024 UTF-8 bytes and labels
to 4096 UTF-8 bytes. IDs must be unique and labels non-empty.

## Event loops and packaging

AppKit tracks menus synchronously. Solo runs the menu in a small native helper
process so JavaScript timers, I/O, mutations, and GPUI painting can continue.
All helper UI operations execute on that process's macOS main thread. The
helper reads descriptors and automation keys from a private pipe, and returns
only a selected ID. Closing the parent pipe dismisses the menu, including when
the application crashes or exits. Normal cancellation releases AppKit objects;
the parent has a bounded forced-cleanup fallback if the helper cannot exit.
Closing the window or quitting cancels every unfinished menu session. Solo
defers AppKit termination until workers have reaped the helpers and removed
their temporary copies; JavaScript keeps running during this drain. Updates
after window closure retain state without trying to repaint the closed window.

The existing `@solo/native` build commands also build the matching
`solo-context-menu.darwin-arm64` or `solo-context-menu.darwin-x64` executable.
Development keeps it beside the native `.node` file. A standalone compiled
executable keeps it beside the app executable. Packaged `.app` applications
must include the helper in `Contents/Resources` and sign it with the app.
Bun extracts embedded `.node` files into temporary storage, so including only
the `.node` binary is insufficient for packaged context menus.

Each menu session copies that helper into a private temporary directory before
launching it. This avoids an AppKit keyboard-tracking failure in a helper that
inherits the enclosing `.app` bundle's identity. The copy is removed after the child exits;
`TMPDIR` selects its disk. If the parent exits first, EOF dismisses the child,
which removes its own staged executable and empty directory before exiting.
An independent pipe-disconnection watchdog also exits the helper if AppKit
tracking does not respond. Cleanup handles macOS temporary-directory aliases
and never removes the shipped helper.

The helper opens a transient accessory application while its menu tracks; it
has no Dock icon. Menu labels contain application data, so the helper receives
no shell commands, paths chosen by menu descriptors, or network instructions.

The existing keyboard automation methods route to the menu while it is open.
Physical mouse and keyboard input are handled by AppKit.

An open menu is a system-wide modal: a click or key from the person at the
machine can dismiss or steer it. Once an automation client attaches (its
`initialize` request), menus are scripted instead. The helper builds the
`NSMenu` and chooses through the item's action, but never opens it or
activates itself. It applies arrow keys, Enter, Escape and typed prefixes with
AppKit's rules: arrows skip disabled items and stop at the ends, Up with nothing
highlighted starts at the bottom, and Enter with nothing highlighted cancels.
One difference: a typed prefix that matches no item leaves the highlight where
it was, while AppKit jumps to a nearby item. GPU-backed tests always script
their menus.

The tests that open real menus on screen are skipped unless
`SOLO_INTERACTIVE_MENUS=1` is set; run them on an idle machine. They cover
AppKit's keyboard tracking, including in the packaged app's staged helper.
The packaged test gets real menus by skipping `initialize`.
