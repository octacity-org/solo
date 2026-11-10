# Application controls

`@solo/solid` exposes app-wide controls and system state that are not tied to
one window.

```tsx
import {
  createKeyboardLayout,
  createThermalState,
  hideApplication,
  restartApplication,
  shouldAutoHideScrollbars,
  Text,
  View,
} from "@solo/solid"

function Status() {
  const thermal = createThermalState()
  const layout = createKeyboardLayout()
  return <View>
    <Text>{thermal() === "nominal" ? "Cool" : `Thermal: ${thermal()}`}</Text>
    <Text>{`Keyboard: ${layout().name}`}</Text>
  </View>
}
```

| function | behavior | platforms |
|---|---|---|
| `hideApplication()` | hides the app, as its Hide menu command (Cmd+H) does | macOS |
| `hideOtherApplications()` | hides every other app, as Hide Others does | macOS |
| `showAllApplications()` | shows the apps that Hide Others hid, as Show All does | macOS |
| `createThermalState()` | accessor for `"nominal"`, `"fair"`, `"serious"` or `"critical"` | macOS |
| `createKeyboardLayout()` | accessor for the selected input source, `{ id, name }`, such as `{ id: "com.apple.keylayout.US", name: "U.S." }` | macOS, Linux |
| `shouldAutoHideScrollbars()` | true when the desktop hides scroll bars until scrolling | macOS, Linux |
| `restartApplication()` | quits, then opens the app again; see [Restart](#restart) | macOS, Linux |

## Thermal state and keyboard layout

`createThermalState()` and `createKeyboardLayout()` work like
`createSystemAppearance()`: call them inside a Solid component or owner. The
first value is read at once; later values follow the system's notifications,
without polling. An accessor stops updating when its owner is disposed.

Under `solo dev`, a reload starts the app again, and its accessors read the
current values afresh.

Thermal state reports how hard the system works to stay cool. At `"serious"`
or `"critical"`, consider pausing background work such as indexing or
animations.

`shouldAutoHideScrollbars()` has no change notification, because GPUI offers
none. Read it again when it matters, for example when a view mounts.

## Restart

`restartApplication()` quits the app the same way as Cmd+Q, then opens the
app bundle again once the old process has exited. JS `exit` handlers run
before the old process ends, as on any quit.

Only a packaged `.app` can restart. Run any other way (`solo dev`, or
`bun app.js`), the executable is `bun` itself, so there is nothing to reopen:
`restartApplication()` throws and the app keeps running.

On Linux the same rule is applied to the running executable: a compiled
`bun build --compile` binary restarts into a new PID of itself, while an app
started with `bun` or `node` throws instead of reopening the runtime.
GPUI's Linux restart waits for this process to exit before re-executing it.
Until the Linux quit path that ends the process lands, the old process can
stay alive with no window after `restartApplication()`; this follows in the
same stack.

## Platforms

macOS has a native feed for every control above. On Linux:

- `createKeyboardLayout()` follows the X11/Wayland keyboard map.
- `shouldAutoHideScrollbars()` reports the desktop's scroll-bar preference.
- `restartApplication()` works only in a compiled app binary, as described
  above.
- `createThermalState()` throws: GPUI always reports `Nominal` on Linux, and a
  value that never changes would look like a reading.
- `hideApplication()`, `hideOtherApplications()` and `showAllApplications()`
  throw: GPUI only logs them on Linux, and silently doing nothing is worse
  than a clear error.

Windows builds carry the same calls but are untested.
## Limits

- Changing the keyboard layout or thermal state is up to the user and the
  system; Solo only reports them.
