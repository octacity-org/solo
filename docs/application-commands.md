# Application commands

Register application commands through `@solo/solid`, inside a Solid component.
Commands belong to that component's owner and are removed when it unmounts.
They work on macOS and Linux. Windows builds use the same UI-thread path but
are untested.

```tsx
import { createSignal } from "solid-js"
import { registerApplicationCommand, Text } from "@solo/solid"

function Articles() {
  const [refreshing, setRefreshing] = createSignal(false)

  registerApplicationCommand({
    id: "articles.refresh",
    label: "Refresh",
    shortcut: "cmd-r",
    menu: "Article",
    enabled: () => !refreshing(),
    run: () => {
      setRefreshing(true)
      void refreshArticles().catch(console.error).finally(() => setRefreshing(false))
    },
  })

  return <Text>Articles</Text>
}
```

`refreshArticles` is application code. Handle its errors in the application,
just as for a button callback.

## Lifetime and state

- `id` must be unique among registered commands. Duplicate registration throws;
  it never replaces someone else's handler. An ID can be reused after disposal.
- `label` is the native menu label. `menu` is an optional top-level menu name.
  Linux has no global menu bar, so `menu` only groups commands for macOS; the
  shortcut remains the way to reach a command there.
- `shortcut` is optional. This is an application command, not a system-wide
  hotkey that runs while another application is active.
- `enabled` defaults to `true`; pass a boolean accessor to update it reactively.
  It controls both shortcut dispatch and the menu item's enabled state.
- All other options are fixed for the registration. Dispose and register again
  to change them.
- The returned function disposes early and is safe to call more than once.
  Solid owner cleanup handles ordinary component unmounts and app remounts.
- Native events carry a registration-specific token. A queued event from a
  disposed registration cannot invoke a new command with the same public ID.
- Registration failures are synchronous and leave existing commands intact.

Application code imports neither `@solo/core` nor `@solo/native`. The internal
bridge replaces a complete descriptor list; native menu and keyboard input
both emit the same application-command event to the Solid callback.

## Shortcuts and focus

- Use one chord: `space`, `s`, `h`, `o`, `cmd-r`, or `cmd-option-p`.
  Modifiers can be absent or combined in any order: `cmd`, `ctrl`, `alt`
  (also spelled `option`), and `shift`. `cmd` is the primary command modifier
  of the platform — Command on macOS, Ctrl on Linux — so `cmd-r` keeps its
  meaning in an app written for macOS. `super` and `win` name the Super key
  explicitly; Linux desktops reserve most Super chords for the shell.
  `secondary` is accepted as an explicit spelling of the same modifier.
  Use lowercase letters; uppercase `S` means `shift-s`, not plain `s`.
  Named keys include `space`, `enter`, `escape`, `up`, `down`, `left`, `right`,
  `home`, `end`, `pageup`, `pagedown`, `backspace`, `delete`, and `f1`–`f12`.
  Multi-chord sequences, Tab shortcuts, and `fn` modifiers are rejected.
- `scopeElementId` optionally limits keyboard dispatch to a retained element
  and its focused descendants. Obtain its `id` from a native element's `ref`,
  then register under a Solid owner once that ref is available. Detached or
  destroyed scopes do not match. Use reactive `enabled` for application state
  such as the active reader mode. Scope does not restrict an explicit menu click.
- Scoped shortcuts never intercept native inputs, textareas, or WebView focus.
  Unscoped shortcuts without the primary command modifier (`cmd`, i.e. Command
  on macOS and Ctrl on Linux) also require non-editable native focus.
  Space/Enter remain available to a focused clickable control. Use
  `<Button tabIndex={0}>` for Tab focus and GPUI's Space/Enter release activation.
- `allowRepeat` defaults to `false`: holding a matching shortcut consumes repeats
  without repeatedly invoking the callback. Set it to `true` for repeatable actions.
- Duplicate shortcuts are rejected even if one command is disabled. Existing
  GPUI editing/navigation bindings and the quit shortcut (`cmd-q`: Cmd+Q on
  macOS, Ctrl+Q on Linux) are reserved; registration cannot override them. Invalid replacements leave the previous bindings intact.
- Enabled commands take precedence over an element's raw `onKeyDown` handler
  in GPUI controls. Disabled/disposed commands no longer consume that shortcut.
  Ordinary typing, editing bindings, and Tab navigation remain native.
- On Linux, shortcuts run through GPUI's key bindings and the same focus
  rules as above. A command whose only reach is a `menu` item cannot be
  invoked there, because nothing draws the menu.
- WKWebView is an AppKit child view, not part of GPUI's keyboard dispatch tree.
  Give an **unscoped Command-modified** command a `menu` to make its shortcut
  available through the macOS menu key-equivalent path while web content has
  focus. Shortcut-only commands have
  no such guarantee in web content. Scoped and non-Command shortcuts have no
  native menu accelerator, so they cannot steal web typing or composition.
  Solo does not intercept web-page key events.

## Platforms

macOS installs the commands as menu items and key bindings; a menu click and a
shortcut emit the same event. Linux installs the commands and their shortcuts
as GPUI key bindings only, so `enabled` and focus scoping behave the same;
application commands do not appear in the status-item menu. Windows shares
that UI-thread path but is untested.

## Verification

Headless tests cover registration, disposal/remounts, duplicate IDs, failed
registration, reactive enabled state, and stale native events. The packaged
regression exercises commands across native controls and checks that text
editing continues to work.
`application-shortcuts-live.test.ts` additionally covers scoped and unscoped
shortcuts, first-focus dispatch, modifier ordering, repeats, focused buttons,
and input/contenteditable WebView first responders. Native parser/scope tests
also cover detached scopes and reserved bindings. IME composition and physical
AppKit menu accelerators still need manual macOS verification.

To build the standalone macOS verification app after building the native addon
and core package:

```sh
bun packages/solid/src/__tests__/fixtures/package-commands.ts
```

The script prints the executable path inside a temporary `Solo Commands.app`.
Use its Article menu, native input/textarea, enable/dispose buttons, and embedded
web input to check actual macOS event routing. Automated GPUI keyboard injection
does not by itself prove AppKit menu routing or WKWebView first-responder behavior.

Manual macOS verification also checks that typing in the web input still works,
Cmd+R invokes Refresh once with that input focused, and Article → Refresh invokes
the same callback once. Toggle enabled to inspect the disabled native menu item;
toggle registration to remove it and register it again without duplicate calls.
