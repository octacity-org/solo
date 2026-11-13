import { homedir, tmpdir } from "node:os"
import { basename, dirname, isAbsolute, join } from "node:path"
import * as native from "@solo/native"

/** Replaced at build time by `@solo/solid/vite` (see `appInfoDefine`). */
declare const __SOLO_APP_INFO__: InjectedAppInfo | undefined

interface InjectedAppInfo {
  name: string | null
  version: string | null
  identifier: string | null
  /** The Vite project root: resources while running under bun or node. */
  root: string
}

/** The app's identity, read from its `package.json` when it was built. */
export interface AppInfo {
  /** `name` in package.json. */
  name: string | null
  /** `version` in package.json. */
  version: string | null
  /** `solo.identifier` in package.json: a reverse-DNS name such as `dev.solo.tasks`. */
  identifier: string | null
}

/** Absolute directories the app reads and writes. None are created for you. */
export interface AppPaths {
  home: string
  /** The app's own data. macOS: ~/Library/Application Support/<id>; Linux: $XDG_DATA_HOME/<id>. */
  data: string
  /** Settings. macOS: same as `data`; Linux: $XDG_CONFIG_HOME/<id>. */
  config: string
  /** Data the app can rebuild. macOS: ~/Library/Caches/<id>; Linux: $XDG_CACHE_HOME/<id>. */
  cache: string
  /** macOS: ~/Library/Logs/<id>; Linux: $XDG_STATE_HOME/<id>/logs. */
  logs: string
  temp: string
  /**
   * Files shipped with the app: `Contents/Resources` inside a macOS `.app`,
   * the executable's directory for any other compiled binary, and the project
   * directory when the app runs under `bun` or `node` (`solo dev`, tests).
   */
  resources: string
  /** The running executable: the compiled app, or `bun`/`node`. */
  executable: string
}

/** @internal Everything `resolveAppPaths` reads from the environment. */
export interface AppPathsInput {
  platform: NodeJS.Platform
  identifier: string | null
  home: string
  temp: string
  env: Record<string, string | undefined>
  executable: string
  root: string | null
}

const injected = typeof __SOLO_APP_INFO__ === "undefined" ? undefined : __SOLO_APP_INFO__

/** The app's name, version and identifier, from the package.json it was built from. */
export function appInfo(): AppInfo {
  return {
    name: injected?.name ?? null,
    version: injected?.version ?? null,
    identifier: injected?.identifier ?? null,
  }
}

/**
 * The app's standard directories on this platform. Throws when package.json
 * sets no `solo.identifier`, and on Windows.
 */
export function appPaths(): AppPaths {
  return resolveAppPaths({
    platform: process.platform,
    identifier: injected?.identifier ?? null,
    home: homedir(),
    temp: tmpdir(),
    env: process.env,
    executable: process.execPath,
    root: injected?.root ?? null,
  })
}

/** Create `path` and any missing parents, and return it. Throws for a relative path. */
export function ensureDir(path: string): string {
  if (typeof path !== "string" || !isAbsolute(path)) {
    throw new TypeError(`Expected an absolute path, got ${JSON.stringify(path)}`)
  }
  native.createDirectory(path)
  return path
}

/** @internal */
export function resolveAppPaths(input: AppPathsInput): AppPaths {
  const { platform, identifier, home, env } = input
  if (platform === "win32") throw new Error("appPaths() is not supported on Windows yet")
  if (identifier === null) {
    throw new Error('appPaths() needs the app identifier. Set "solo": { "identifier": "com.example.app" } in package.json')
  }
  const resources = resourcesDirectory(input)
  if (platform === "darwin") {
    const library = join(home, "Library")
    const data = join(library, "Application Support", identifier)
    return {
      home,
      data,
      config: data,
      cache: join(library, "Caches", identifier),
      logs: join(library, "Logs", identifier),
      temp: input.temp,
      resources,
      executable: input.executable,
    }
  }
  // XDG Base Directory: an unset, empty or relative variable means the default.
  const xdg = (name: string, fallback: string) => {
    const value = env[name]
    return value && isAbsolute(value) ? value : join(home, fallback)
  }
  return {
    home,
    data: join(xdg("XDG_DATA_HOME", ".local/share"), identifier),
    config: join(xdg("XDG_CONFIG_HOME", ".config"), identifier),
    cache: join(xdg("XDG_CACHE_HOME", ".cache"), identifier),
    logs: join(xdg("XDG_STATE_HOME", ".local/state"), identifier, "logs"),
    temp: input.temp,
    resources,
    executable: input.executable,
  }
}

function resourcesDirectory({ platform, executable, root }: AppPathsInput): string {
  const directory = dirname(executable)
  if (platform === "darwin" && basename(directory) === "MacOS" && dirname(directory).endsWith(".app/Contents")) {
    return join(dirname(directory), "Resources")
  }
  // The same rule restartApplication uses for a compiled app binary.
  const stem = basename(executable).replace(/\.exe$/, "")
  if (!["bun", "node", "nodejs"].includes(stem)) return directory
  if (root === null) {
    throw new Error("appPaths() could not find the project directory; build the app with @solo/solid/vite")
  }
  return root
}
