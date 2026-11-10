# Deep links and opened files

An app can claim a custom URL scheme such as `myapp://`. Opening a link in
that scheme starts the app, or reaches the copy already running, and
`onOpenUrls` receives the URL. Files opened with the app arrive the same way,
as `file://` URLs. macOS routes them through Launch Services; Linux uses
launch arguments, a Unix socket and a freedesktop `.desktop` entry (see
[Linux](#linux)).

```tsx
import { onOpenUrls, render } from "@solo/solid"

onOpenUrls((urls) => {
  for (const url of urls) {
    let parsed: URL
    try { parsed = new URL(url) } catch { continue }
    if (parsed.protocol === "myapp:" && parsed.hostname === "note") {
      openNote(parsed.searchParams.get("id"))
    }
  }
})

render(() => <App />)
```

## Declare the scheme in `Info.plist`

macOS routes a scheme to an app because the app's `Info.plist` declares it
under `CFBundleURLTypes`. This is the registration path Solo apps use:

```xml
<key>CFBundleURLTypes</key>
<array>
  <dict>
    <key>CFBundleURLName</key><string>com.example.myapp</string>
    <key>CFBundleURLSchemes</key><array><string>myapp</string></array>
  </dict>
</array>
```

Launch Services reads it when the app is installed or first opened. To
register a build by hand, run `lsregister -f` on it (the tool lives in
`/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/`).
macOS ignores apps inside temporary folders, so a bundle under
`/var/folders/…/T` never receives links.

## `registerUrlScheme`

```ts
await registerUrlScheme("myapp")
```

When several installed apps declare the same scheme, macOS picks one.
`registerUrlScheme` makes this app the default handler. It needs macOS 12 or
later and an installed bundle with a `CFBundleIdentifier`, and it does not
replace the `Info.plist` declaration. It rejects names that are not valid URL
schemes (a letter, then letters, digits, `+`, `-` or `.`; no `://`) and
system schemes such as `http`, `https`, `file` and `mailto`.

## Delivery

- Each call receives the URLs of one request, in the order macOS sent them.
- URLs that arrive before the first listener, including the one that launched
  the app, are kept and passed to that listener as soon as it subscribes,
  before `onOpenUrls` returns. Later URLs go to every current listener.
- `onOpenUrls` returns a function that stops listening. Inside a component,
  call it from `onCleanup`.
- A second `open myapp://…` reaches the running app. macOS does not start a
  second copy.
- `solo dev` starts a new app Worker on every reload, so listeners from the
  previous build are gone and the new build subscribes afresh.
- Windows does not deliver URLs: `onOpenUrls` never fires and
  `registerUrlScheme` rejects.

## Linux

Nothing on Linux declares a scheme for the whole desktop, so Solo does all
three steps itself.

**Launch.** URLs and files passed to the executable arrive in `onOpenUrls`,
queued until the first listener subscribes, so a cold launch by link works.
A URL argument is used as is. An absolute path to a file becomes a
`file://` URL. Flags and the JS entry that Bun or Node runs are ignored.

**Single instance.** The first launch listens on
`$XDG_RUNTIME_DIR/solo-<name>-<hash>.sock`. When `XDG_RUNTIME_DIR` is unset,
the socket goes into a per-user `solo-<uid>` directory under the temporary
directory, created with mode 0700; an existing directory owned by another
user, or accessible to group or others, is refused instead of used. A second
launch sends its URLs and paths over the socket and exits with status 0, so
there is never a second window. A socket left by a crashed instance is taken
over.

**Registration.**

```ts
await registerUrlScheme("myapp")
```

writes `$XDG_DATA_HOME/applications/solo-<name>-<hash>.desktop` with
`Exec=<this executable> %U` and `MimeType=x-scheme-handler/myapp;`, keeping
any keys and MIME types already in the file, then runs
`xdg-mime default` for the scheme. It needs `xdg-mime` (xdg-utils). The
desktop file's name derives from the executable, so two builds of different
apps do not collide; under Bun or Node, where the executable is the shared
runtime, it derives from the JS entry instead. That same identity names the
single-instance socket. The `Exec` line repeats the JS entry with its
absolute path.

A link opened with `xdg-open` (or `gio open`) reaches the running app once
`xdg-mime query default x-scheme-handler/myapp` names this entry. `xdg-open`
on a headless session can fall back to a browser because it cannot detect a
desktop environment; a normal desktop delegates to the desktop's opener.
The same limits as macOS apply: any app or page can send these URLs, so
validate them.

## Treat URLs as untrusted

Any app, web page or script can open a URL in your scheme, with any path and
query. Parse it with `new URL()`, accept only the routes you expect, and
validate every parameter before acting on it. Never pass a URL's contents to
a shell, `eval`, a file path, or an action that changes data without a
confirmation step. A `file://` URL names a file the user opened with the app;
check it is a kind of file you handle before reading it.
