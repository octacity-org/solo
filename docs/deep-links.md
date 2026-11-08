# Deep links and opened files

A packaged macOS app can claim a custom URL scheme such as `myapp://`.
Opening a link in that scheme starts the app, or reaches the copy already
running, and `onOpenUrls` receives the URL. Files opened with the app
arrive the same way, as `file://` URLs.

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
- macOS only. Elsewhere `onOpenUrls` never fires and `registerUrlScheme`
  rejects.

## Treat URLs as untrusted

Any app, web page or script can open a URL in your scheme, with any path and
query. Parse it with `new URL()`, accept only the routes you expect, and
validate every parameter before acting on it. Never pass a URL's contents to
a shell, `eval`, a file path, or an action that changes data without a
confirmation step. A `file://` URL names a file the user opened with the app;
check it is a kind of file you handle before reading it.
