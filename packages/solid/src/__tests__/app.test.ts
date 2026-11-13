import { beforeEach, describe, expect, it, vi } from "vitest"
import * as solo from "@solo/solid"
import { resolveAppPaths, type AppPathsInput } from "../app.js"

const platform = vi.hoisted(() => ({ createDirectory: vi.fn() }))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

beforeEach(() => { vi.resetAllMocks() })

const macos: AppPathsInput = {
  platform: "darwin",
  identifier: "dev.solo.tasks",
  home: "/Users/ada",
  temp: "/var/folders/xy/T",
  env: {},
  executable: "/Users/ada/.bun/bin/bun",
  root: "/Users/ada/tasks",
}

const linux: AppPathsInput = {
  ...macos,
  platform: "linux",
  home: "/home/ada",
  temp: "/tmp",
  executable: "/home/ada/.bun/bin/bun",
  root: "/home/ada/tasks",
}

describe("app paths on macOS", () => {
  it("follows the Library conventions", () => {
    expect(resolveAppPaths(macos)).toEqual({
      home: "/Users/ada",
      data: "/Users/ada/Library/Application Support/dev.solo.tasks",
      config: "/Users/ada/Library/Application Support/dev.solo.tasks",
      cache: "/Users/ada/Library/Caches/dev.solo.tasks",
      logs: "/Users/ada/Library/Logs/dev.solo.tasks",
      temp: "/var/folders/xy/T",
      resources: "/Users/ada/tasks",
      executable: "/Users/ada/.bun/bin/bun",
    })
  })

  it("ignores XDG variables", () => {
    const paths = resolveAppPaths({ ...macos, env: { XDG_DATA_HOME: "/xdg/data" } })
    expect(paths.data).toBe("/Users/ada/Library/Application Support/dev.solo.tasks")
  })

  it("finds resources inside a packaged .app", () => {
    const executable = "/Applications/Tasks.app/Contents/MacOS/Tasks"
    expect(resolveAppPaths({ ...macos, executable }).resources)
      .toBe("/Applications/Tasks.app/Contents/Resources")
  })

  it("puts resources beside a compiled binary outside an .app", () => {
    const executable = "/Users/ada/tasks/dist/Tasks"
    expect(resolveAppPaths({ ...macos, executable }).resources).toBe("/Users/ada/tasks/dist")
  })
})

describe("app paths on Linux", () => {
  it("falls back to the XDG defaults under the home directory", () => {
    expect(resolveAppPaths(linux)).toEqual({
      home: "/home/ada",
      data: "/home/ada/.local/share/dev.solo.tasks",
      config: "/home/ada/.config/dev.solo.tasks",
      cache: "/home/ada/.cache/dev.solo.tasks",
      logs: "/home/ada/.local/state/dev.solo.tasks/logs",
      temp: "/tmp",
      resources: "/home/ada/tasks",
      executable: "/home/ada/.bun/bin/bun",
    })
  })

  it("uses absolute XDG overrides", () => {
    const env = {
      XDG_DATA_HOME: "/xdg/data",
      XDG_CONFIG_HOME: "/xdg/config",
      XDG_CACHE_HOME: "/xdg/cache",
      XDG_STATE_HOME: "/xdg/state",
    }
    expect(resolveAppPaths({ ...linux, env })).toMatchObject({
      data: "/xdg/data/dev.solo.tasks",
      config: "/xdg/config/dev.solo.tasks",
      cache: "/xdg/cache/dev.solo.tasks",
      logs: "/xdg/state/dev.solo.tasks/logs",
    })
  })

  it("ignores empty and relative XDG values, as the spec requires", () => {
    const env = { XDG_DATA_HOME: "", XDG_CONFIG_HOME: "relative/config" }
    expect(resolveAppPaths({ ...linux, env })).toMatchObject({
      data: "/home/ada/.local/share/dev.solo.tasks",
      config: "/home/ada/.config/dev.solo.tasks",
    })
  })

  it("puts resources beside a compiled binary", () => {
    expect(resolveAppPaths({ ...linux, executable: "/opt/tasks/tasks" }).resources).toBe("/opt/tasks")
  })

  it("uses the project directory under bun or node", () => {
    for (const executable of ["/usr/bin/node", "/usr/bin/nodejs", "/home/ada/.bun/bin/bun"])
      expect(resolveAppPaths({ ...linux, executable }).resources).toBe("/home/ada/tasks")
  })
})

describe("app paths elsewhere", () => {
  it("throws on Windows until it is implemented", () => {
    expect(() => resolveAppPaths({ ...macos, platform: "win32" })).toThrow("not supported on Windows yet")
  })

  it("explains a missing identifier", () => {
    expect(() => resolveAppPaths({ ...macos, identifier: null }))
      .toThrow('Set "solo": { "identifier": "com.example.app" } in package.json')
  })

  it("explains a missing project directory under bun", () => {
    expect(() => resolveAppPaths({ ...macos, root: null })).toThrow("@solo/solid/vite")
  })
})

describe("appInfo", () => {
  it("reports what the Vite plugin injected from package.json", () => {
    // vitest runs packages/solid through solidUniversal(), so this is
    // @solo/solid's own manifest, which has no Solo identifier.
    expect(solo.appInfo()).toEqual({ name: "@solo/solid", version: "0.4.0", identifier: null })
  })
})

describe("ensureDir", () => {
  it("creates the directory natively and returns it", () => {
    expect(solo.ensureDir("/Users/ada/Library/Caches/dev.solo.tasks"))
      .toBe("/Users/ada/Library/Caches/dev.solo.tasks")
    expect(platform.createDirectory)
      .toHaveBeenCalledExactlyOnceWith("/Users/ada/Library/Caches/dev.solo.tasks")
  })

  it("rejects relative paths before dispatch", () => {
    for (const value of ["cache", "", undefined, 42])
      expect(() => solo.ensureDir(value as never)).toThrow(TypeError)
    expect(platform.createDirectory).not.toHaveBeenCalled()
  })

  it("passes native failures through", () => {
    const error = new Error('"/root/x": Permission denied (os error 13)')
    platform.createDirectory.mockImplementation(() => { throw error })
    expect(() => solo.ensureDir("/root/x")).toThrow(error)
  })
})
