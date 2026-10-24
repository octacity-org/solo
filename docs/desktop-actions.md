# Browser, clipboard, file and Trash actions

```ts
import { openExternalUrl, writeClipboardText } from "@solo/solid"

try {
  openExternalUrl("https://example.com/article")
  writeClipboardText("https://example.com/article")
} catch (error) {
  // Show a failure message in your app.
  console.error(error)
}
```

Both functions are synchronous, return `void`, and throw catchable errors.
They are independent of a mounted renderer or Solid owner, so they work in
button callbacks, application commands, and startup code. Currently they support
macOS on the main JavaScript thread; other platforms and worker threads receive
explicit errors. Application code imports no internal Solo packages.

## Opening a URL

`openExternalUrl(url: string)` accepts absolute HTTP/HTTPS URLs with a host and
an explicit `://` separator. Relative paths, other schemes, embedded credentials,
raw whitespace/control characters, and backslashes are rejected before dispatch.
Encode spaces in paths as `%20`. Unicode hosts/paths are normalized using URL
parsing; query strings and fragments remain URL data.

The native bridge validates independently of TypeScript. It passes an `NSURL`
to `NSWorkspace.openURL`, never to a shell. macOS chooses the configured browser.
Success means the OS accepted the request, not that the browser loaded the page
or that the destination is trustworthy. Applications still decide which links
to open and when to ask their users.

## Writing text

`writeClipboardText(text: string)` replaces the system clipboard with plain
Unicode text, including empty strings and multiline text. It uses the same
AppKit pasteboard as GPUI's existing copy/paste implementation, but checks the
write result instead of discarding failures. Existing selection-copy shortcuts
are unchanged. Clipboard reading and rich clipboard formats are not exposed.

A rejected pasteboard write throws; macOS may already have cleared the previous
clipboard contents when that happens. Do not assume clipboard writes are
transactional or that other applications cannot replace the contents afterward.

## Showing and opening files

```ts
import { openPath, revealPath } from "@solo/solid"

revealPath("/Users/me/Downloads/report.pdf") // Finder window with the file selected
openPath("/Users/me/Downloads/report.pdf")   // opens in the default PDF viewer
```

Both take an absolute path to a file or folder, are synchronous, and throw
catchable errors like the actions above. A relative path, or a path that does
not exist, throws before anything reaches Finder; the message includes the
operating system's reason, such as `No such file or directory`. The path is
data, never shell input.

`revealPath` uses `NSWorkspace selectFile:inFileViewerRootedAtPath:`.
`openPath` uses `NSWorkspace openURL:` with a file URL, so it behaves like a
double-click in Finder: an application bundle launches, a folder opens in
Finder, and a `.command` or `.tool` script runs in Terminal. Applications
decide which paths to open. Choosing a specific application ("Open With…") is
not supported.

On Linux and Windows both throw after the path checks.

## Moving to the Trash

```ts
import { moveToTrash } from "@solo/solid"

const results = await moveToTrash(["/Users/me/Downloads/old.zip", "/Users/me/tmp"])
for (const { path, ok, error } of results) if (!ok) console.warn(path, error)
```

`moveToTrash(paths)` moves files and folders to the Trash, where the user can
put them back, and resolves with one `{ path, ok, error? }` per input path, in
input order. A path that cannot be moved (missing, no permission, a volume
without a Trash) gets `ok: false` and macOS's reason; the other paths still
move. The promise rejects only when a path is not absolute or on platforms
other than macOS.

Unlike the actions above it is asynchronous: it runs `NSFileManager`
`trashItemAtURL` on a background thread, because a large folder can take a
while, and it works from the app Worker. Paths are data, never shell input.

## Verification

Rust tests cover URL parsing, absolute and existing path checks, a real
private-pasteboard Unicode round trip without touching the user's clipboard,
and trashing a real file and folder (removed from the Trash afterwards). Public
API tests check argument types,
unchanged text delivery, and error propagation. The packaged regression checks
that an invalid URL reaches a catchable error and leaves the app responsive.

After building native and core, build the standalone macOS fixture:

```sh
bun packages/solid/src/__tests__/fixtures/package-commands.ts desktop
```

Set `TMPDIR` to an existing external-disk directory to put its temporary bundle
and compilation output there. The script prints the executable path.

In the fixture's **Desktop** menu, choose **Open test URL** and confirm the
configured browser opens `https://example.com/#solo-desktop-check`. Choose
**Copy Unicode URL**, then paste into a new document in another application;
expect `https://example.com/世界?q=İstanbul#solo-desktop-check`. This manual copy
replaces the system clipboard. **Reject invalid URL** must show an error without
opening another application. **Show test file in Finder** must open a Finder
window with `Solo Path Check.txt` selected, and **Open test file** must open it
in the default text editor. **Move test file to Trash** must report success,
and Finder's Trash must show `Solo Trash Check.txt` with **Put Back** available.
