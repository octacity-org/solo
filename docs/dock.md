# Dock integration

Three macOS Dock behaviors: reopening the window when the Dock icon is
clicked, app-defined items in the Dock icon's menu, and recent documents.

```tsx
import {
  addRecentDocument, configureDockMenu, onApplicationReopen, onOpenUrls, render,
} from "@solo/solid"

function App() {
  configureDockMenu(() => [
    { id: "new-note", label: "New Note", run: createNote },
    { type: "separator" },
    { id: "sync", label: "Sync Now", enabled: online(), run: sync },
  ])
  return <Notes />
}

const root = render(() => <App />, { title: "Notes", menuBar: { iconPath } })
onApplicationReopen(() => root.showWindow())
onOpenUrls((urls) => urls.filter((url) => url.startsWith("file://")).forEach(openFile))
addRecentDocument("/Users/me/Notes/today.txt")
```

## Reopen on Dock click

`onApplicationReopen(listener)` runs when the user clicks the Dock icon, or
opens the app again from Finder or with `open`, while the app shows no
window. It returns a function that stops listening.

Only a background app (one with `menuBar` in `render`) keeps running after its
window closes; an ordinary app quits with its last window. Solo does not
reopen the window by itself: call `root.showWindow()` from the listener.

## Dock menu

`configureDockMenu(source)` sets the items above the ones macOS adds itself.
Items have the same shape as `configureMenuBar` items:

- `{ id, label, run, enabled?, checked? }` — an action. A disabled action is
  greyed out and never runs.
- `{ type: "separator" }`
- `{ type: "status", label }` — a label nobody can choose.

`source` is reactive: the menu is rebuilt when a signal it reads changes.
Call it inside a component or another Solid owner; disposing the owner
removes the items. One Dock menu configuration can be active at a time. A
`solo dev` reload also removes the previous build's items.

## Recent documents

`addRecentDocument(path)` adds an existing file, given by absolute path, to
the app's recent documents, which macOS lists in the Dock menu. Declare the
file types the app opens under `CFBundleDocumentTypes` in its `Info.plist`:
the recent-document list belongs to AppKit's document controller, which may
ignore files of types the app does not declare.

Choosing a recent document does not call a Dock callback. macOS asks the app
to open the file, and it arrives through `onOpenUrls` as a `file://` URL, the
same way as a file opened from Finder. Decode it with
`fileURLToPath(new URL(url))` from `node:url`, and treat it as untrusted input
(see [deep-links.md](deep-links.md)).

On Linux there is no Dock. `configureDockMenu` and `onApplicationReopen`
throw, but `addRecentDocument(path)` appends a bookmark to the freedesktop
`recently-used.xbel`, so the file appears in GTK file choosers and desktop
recent lists; it is not shown in a Solo-drawn menu. See
[linux-packaging.md](linux-packaging.md).

Windows: every one of these throws.
