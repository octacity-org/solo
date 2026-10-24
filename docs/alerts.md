# Native alerts

`showAlert` shows a native alert, an `NSAlert` sheet on macOS, and resolves
to the index of the button the user clicked.

```ts
import { showAlert } from "@solo/solid"

const choice = await showAlert({
  message: "Delete 3 files?",
  detail: "They move to the Trash.",
  level: "warning",
  buttons: ["Delete", "Cancel"],
})
if (choice === 0) {
  // Delete
}
```

## Options

- `message` (required) and `detail` must be non-empty strings.
- `level` is `"info"` (the default), `"warning"` or `"critical"`. It picks
  the alert's icon and emphasis.
- `buttons` is a non-empty list of non-empty labels, shown in order, and
  defaults to `["OK"]`. The first button answers Return. A label reading
  "Cancel" (in any case) answers Escape. Labels are shown as written.
- `window` attaches the sheet to an `openWindow` window. The main window is
  the default. A closed window rejects with `Window <id> is not open`.

Invalid options throw a `TypeError` before anything reaches the native side.

## One alert at a time

Only one alert may be open in an application. While one is open, another
`showAlert` call rejects with `An alert is already open`, and the next call
succeeds once the first one is answered. Alerts are separate from
[file dialogs](file-dialogs.md): an open file dialog does not block an alert.

GPUI's `Window::prompt` panics only when it is called again while it is
still building a prompt, which a Solo app cannot do. The limit above is
Solo's own rule, so a second alert fails with a catchable error instead of
stacking behind the first.

If the window closes, or the app shuts down, before the alert is answered,
the promise rejects. It never resolves to a made-up index.

Alerts currently require macOS. Other platforms, and renderers without the
capability, reject explicitly.

## Verification

Unit tests cover option parsing, button mapping, the one-at-a-time rule and
a dropped alert. `TestSoloRenderer` replaces the `NSAlert` with a stand-in
prompt: GPU-backed tests read it with `pendingAlert()` and click a button
with `answerAlert(index)`, and they cover the index result, the rejected
second alert and attaching to an `openWindow` window.

For manual verification:

```sh
bun packages/solid/src/__tests__/fixtures/package-commands.ts file-dialogs
```

Open the emitted app and press "Show alert". The status reads "Second alert
rejected", because the fixture also requests a second alert. Check the
sheet's icon, text and three buttons. Click each button and confirm the
status shows its index (Delete 0, Keep 1, Cancel 2), and that Escape answers
Cancel.
