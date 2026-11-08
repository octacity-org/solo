# Application controls (macOS)

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

| function | behavior |
|---|---|
| `hideApplication()` | hides the app, as its Hide menu command (Cmd+H) does |
| `hideOtherApplications()` | hides every other app, as Hide Others does |
| `showAllApplications()` | shows the apps that Hide Others hid, as Show All does |
| `createThermalState()` | accessor for `"nominal"`, `"fair"`, `"serious"` or `"critical"` |
| `createKeyboardLayout()` | accessor for the selected input source, `{ id, name }`, such as `{ id: "com.apple.keylayout.US", name: "U.S." }` |
| `shouldAutoHideScrollbars()` | true when System Settings → Appearance → "Show scroll bars" hides them until scrolling |
| `restartApplication()` | quits, then opens the app again; see [Restart](#restart) |

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

## Limits

- macOS only. On Linux and Windows every function throws.
- Changing the keyboard layout or thermal state is up to the user and the
  system; Solo only reports them.
