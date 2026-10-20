# Native file dialogs

`@solo/solid` exposes macOS file-open, folder-open and save-destination
dialogs as typed, asynchronous functions. Applications receive paths only; Solo never reads,
writes, validates, or creates the selected files.

```ts
import { selectFiles, selectSavePath } from "@solo/solid"

const imports = await selectFiles({
  multiple: true,
  prompt: "Import",
})
if (imports === null) {
  // The user cancelled.
}

const destination = await selectSavePath({
  suggestedName: "Newsprint Notes.md",
  initialDirectory: "/Users/me/Documents",
})
if (destination !== null) {
  // The application decides whether and how to write destination.
}
```

## Results and errors

- `selectFiles()` resolves to one path by default, or multiple paths when
  `multiple: true`. It resolves to `null` on cancellation. With
  `directories: true` the panel selects folders instead of files.
- `selectSavePath()` resolves to the chosen destination or `null` on
  cancellation. Showing or accepting this dialog does not create the file.
- Operational failures reject the promise. Channel closure during shutdown is
  an error, never cancellation.
- Only one file dialog may be active for a renderer. A concurrent open or save
  request rejects with an `already open` error. The next request is allowed
  after selection, cancellation, or failure settles the first request.

`prompt` is an optional non-empty native confirmation-button label.
`suggestedName` must be a non-empty filename without directory components.
`initialDirectory` must be absolute and defaults to the process working
directory. File-extension filtering is not supported by the pinned GPUI API.

Dialogs currently require macOS. Unsupported platforms, a renderer without the
capability, and an uninitialized or stopped renderer reject explicitly. Paths
are returned as Unicode strings; non-Unicode native paths reject because they
cannot be represented faithfully in JavaScript.

GPUI creates the native panels on its foreground executor. Solo waits for their
oneshot result on a napi worker task, so neither JavaScript nor the macOS event
pump synchronously waits for user input.

## Verification

Unit tests cover options, Unicode and space-containing paths, cancellation,
errors, unavailable capabilities, and concurrency. GPU-backed tests exercise
the real napi promise boundary. The packaged fixture opens an actual macOS
panel and uses an external heartbeat plus automation query to prove JavaScript
and the event pump remain responsive while it is open.

For manual verification:

```sh
bun packages/solid/src/__tests__/fixtures/package-commands.ts file-dialogs
```

Open the emitted app. Test open selection and cancellation, then test save with
the suggested Unicode filename and `/tmp` initial directory. Confirm accepting
the save destination does not create a file, and repeat both dialogs to verify
the prior request was cleaned up.
