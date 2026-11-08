# System appearance

`createSystemAppearance()` from `@solo/solid` returns a reactive accessor with
the system's current appearance: `"light"` or `"dark"`. Call it inside a mounted
Solid component or owner. The initial value is available synchronously;
subsequent values come from native GPUI appearance notifications, without polling.

Keep the user's theme preference separate from this OS state:

```tsx
import { createSignal } from "solid-js"
import { createSystemAppearance, Text, View } from "@solo/solid"
import type { SystemAppearance } from "@solo/solid"

function Settings() {
  const system = createSystemAppearance()
  const [preference, setPreference] = createSignal<"system" | SystemAppearance>("system")
  const effective = (): SystemAppearance => {
    const choice = preference()
    return choice === "system" ? system() : choice
  }

  return <View style={{ backgroundColor: effective() === "dark" ? "#202433" : "#f7f8fa" }}>
    <Text>{`System: ${system()}`}</Text>
    <input theme={{ appearance: effective() }} />
  </View>
}
```

Bind your theme controls to `setPreference`. Solo does not persist or replace
that preference. Selecting explicit Light or Dark remains effective when macOS
changes; selecting System follows the current OS value. Existing style and native
`theme` props update reactively without rebuilding the element tree.

All consumers on a renderer share one native observer. Solid owner cleanup removes
each consumer; disposing the last consumer removes the native observer. Remounting
reads a fresh snapshot, and events queued for an earlier observer are ignored.
Light/vibrant-light and dark/vibrant-dark normalize to light and dark respectively.

On Linux, GPUI reads the desktop's `color-scheme` from xdg-desktop-portal
through its Wayland backend and notifies subscribers when it changes. The X11
backend has no appearance feed yet, so under X11 the accessor reports `light`
and never changes; use an explicit preference there. Windows builds reject the
call.

A renderer without the native capability throws an explicit error. The accessor
never changes the desktop's settings.

## Verification

Solid tests cover initial values, native transitions, explicit overrides, shared
cleanup, remounts, invalid events, and mutation-only updates. Native tests exercise
appearance normalization and observer registration/disposal. The packaged fixture
checks the real native snapshot, preference switching, and continued input handling.

For a manual macOS check, package and open the fixture:

```sh
bun packages/solid/src/__tests__/fixtures/package-commands.ts system-appearance
```

Open the emitted `.app`, then switch macOS Appearance between Light and Dark in
System Settings. In Follow System, the labels, background, and native input should
update while Mount remains 1. In Explicit light/dark (Theme menu), only the System
label should follow the OS. Toggle appearance component twice to dispose/remount,
then repeat; the Mount label increments once and updates remain single.
Restore your original macOS appearance afterward.
