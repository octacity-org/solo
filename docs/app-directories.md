# App directories and app info

```ts
import { appInfo, appPaths, ensureDir } from "@solo/solid"

const paths = appPaths()
const settings = `${ensureDir(paths.config)}/settings.json`
const icon = `${paths.resources}/icon.png`
console.log(appInfo()) // { name: "tasks", version: "1.0.0", identifier: "dev.solo.tasks" }
```

## Identify the app

`appPaths()` names the app's directories after its identifier, a reverse-DNS
name set in the app's `package.json`:

```json
{
  "name": "tasks",
  "version": "1.0.0",
  "solo": { "identifier": "dev.solo.tasks" }
}
```

`@solo/solid/vite` reads that `package.json` when it builds the app, in
`solo dev`, tests and production builds alike, so all three agree. A build
fails when `solo.identifier` is not made of letters, digits and hyphens
separated by dots, since it becomes a directory name. On macOS, use the same
value as the `.app`'s `CFBundleIdentifier`.

`appInfo()` returns `{ name, version, identifier }` from the same file. Each
field is `null` when `package.json` leaves it out.

## Directories

| Field | macOS | Linux |
|---|---|---|
| `data` | `~/Library/Application Support/<id>` | `$XDG_DATA_HOME/<id>` (`~/.local/share/<id>`) |
| `config` | same as `data` | `$XDG_CONFIG_HOME/<id>` (`~/.config/<id>`) |
| `cache` | `~/Library/Caches/<id>` | `$XDG_CACHE_HOME/<id>` (`~/.cache/<id>`) |
| `logs` | `~/Library/Logs/<id>` | `$XDG_STATE_HOME/<id>/logs` (`~/.local/state/<id>/logs`) |
| `home` | the user's home directory | the user's home directory |
| `temp` | the system temporary directory | the system temporary directory |
| `executable` | the running executable | the running executable |
| `resources` | see below | see below |

On Linux, an XDG variable that is unset, empty or relative falls back to its
default, as the XDG Base Directory specification requires.

`appPaths()` creates nothing. Call `ensureDir(path)` before writing: it creates
the directory and any missing parents, returns the path, and throws for a
relative path or when the directory cannot be created. It is native, so app
code that only needs directories doesn't pull `node:fs` into the executable.

`appPaths()` throws when `package.json` has no `solo.identifier`, and on
Windows, which is not supported yet.

## Resources

`resources` is where the files shipped with the app live:

- inside a packaged macOS app (`<App>.app/Contents/MacOS/<exe>`), it is
  `<App>.app/Contents/Resources`;
- for any other compiled binary, it is the binary's directory;
- under `bun` or `node` (`solo dev`, tests, `bun dist/index.js`), it is the
  project directory: the Vite root holding `package.json`.

So a file at `<project>/icon.png` during development ships as
`Contents/Resources/icon.png` in the `.app`, or next to a Linux binary.
