# Notifications

An app can post notifications to the macOS Notification Center, replace or
remove them by tag, and hear when the user clicks one or one of its buttons.

```tsx
import { dismissNotification, onNotificationResponse, showNotification } from "@solo/solid"

onNotificationResponse(({ tag, actionId }) => {
  if (tag === "export" && actionId === "reveal") revealExport()
  else if (tag === "export") showExportWindow()
})

showNotification({
  tag: "export",
  title: "Export finished",
  body: "report.pdf is ready",
  actions: [{ id: "reveal", label: "Show in Finder" }],
})

// Later, once it no longer applies:
dismissNotification("export")
```

| function | behavior |
|---|---|
| `showNotification({ tag, title, body?, actions? })` | posts a notification; a tag still shown is replaced |
| `dismissNotification(tag)` | removes the notification with that tag, shown or still pending |
| `onNotificationResponse(listener)` | receives `{ tag, actionId }` for each click; returns a function that stops listening |

`tag` and `title` must be non-empty. Each action needs a non-empty `id` and
`label`, and ids must be unique within one notification; anything else
throws before the notification is posted.

## A packaged app

macOS delivers notifications only to an app bundle with a
`CFBundleIdentifier`. Under `bun` or `solo dev`, and in a binary run outside
its `.app`, both `showNotification` and `dismissNotification` throw:

```text
showNotification needs a packaged .app with a CFBundleIdentifier: macOS delivers notifications only to app bundles
```

To try notifications, package the app as a `.app` whose `Info.plist` sets
`CFBundleIdentifier`, the same value as `solo.identifier` (see
[app-directories.md](app-directories.md)).

## Permission

The first `showNotification` asks the user to allow notifications for the
app, with alerts and sounds. Until they answer, and after they deny it, macOS
shows nothing and `showNotification` still returns normally: Solo does not
report the user's choice. They can change it later in System Settings >
Notifications.

## Responses

- A click on the notification itself arrives with `actionId: null` and brings
  the app to the front.
- A click on one of its buttons arrives with that button's `id`. It does not
  bring the app to the front; open a window yourself if the action needs one.
- A notification is shown as a banner even while the app is frontmost.
- Responses that arrive before the first listener, including a click that
  launched the app, are kept and passed to that listener before
  `onNotificationResponse` returns. Later responses go to every current
  listener.
- `solo dev` starts a new app Worker on every reload, so listeners from the
  previous build are gone and the new build subscribes afresh.

## Platforms

Notifications work on macOS only for now. On Linux and Windows,
`showNotification` and `dismissNotification` throw, and
`onNotificationResponse` never fires.
