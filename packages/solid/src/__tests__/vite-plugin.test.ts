import { describe, expect, it } from "vitest"
import { mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { basename, dirname, join, resolve } from "node:path"
import { solidUniversal } from "../vite.js"

type ConfigHook = (config: object, env: { command: "build" | "serve"; mode: string }) => void
type ResolveHook = (id: string) => Promise<string | { id: string } | null>

async function resolveFor(env: { command: "build" | "serve"; mode: string }, id: string) {
  const plugin = solidUniversal()
  ;(plugin.config as ConfigHook)({}, env)
  const resolved = await (plugin.resolveId as ResolveHook)(id)
  return typeof resolved === "string" ? resolved : resolved?.id
}

describe("solidUniversal Solid build selection", () => {
  it("bundles Solid's production client builds for production app builds", async () => {
    const env = { command: "build", mode: "production" } as const
    expect(await resolveFor(env, "solid-js")).toMatch(/\/solid-js\/dist\/solid\.js$/)
    expect(await resolveFor(env, "@solidjs/signals")).toMatch(/\/@solidjs\/signals\/dist\/prod\/index\.js$/)
    expect(await resolveFor(env, "@solidjs/universal")).toMatch(/\/@solidjs\/universal\/dist\/universal\.js$/)
  })

  it("keeps Solid's dev builds for tests and development-mode builds", async () => {
    for (const env of [
      { command: "serve", mode: "test" },
      { command: "build", mode: "development" },
    ] as const) {
      expect(await resolveFor(env, "solid-js")).toMatch(/\/solid-js\/dist\/solid\.dev\.js$/)
      expect(await resolveFor(env, "@solidjs/signals")).toMatch(/\/@solidjs\/signals\/dist\/dev\.js$/)
      expect(await resolveFor(env, "@solidjs/universal")).toMatch(/\/@solidjs\/universal\/dist\/universal\.dev\.js$/)
    }
  })
})

type LoadHook = (id: string) => string | undefined

describe("solidUniversal worker-mode split", () => {
  const env = { command: "build", mode: "production" } as const

  it.skipIf(process.platform !== "darwin")("splits a macOS SSR build into a bootstrap and an app Worker entry", () => {
    const plugin = solidUniversal()
    const result = (plugin.config as (config: object, e: typeof env) => any)(
      { root: "/app", build: { ssr: "src/index.tsx" } },
      env,
    )
    const input = result.build.rollupOptions.input as Record<string, string>
    expect(Object.keys(input)).toEqual(["index", "index.app"])
    const load = plugin.load as LoadHook
    const boot = { type: "chunk", facadeModuleId: input.index, fileName: "index.js", code: load(input.index)! }
    const bundle = { boot, app: { type: "chunk", facadeModuleId: input["index.app"], fileName: "index.app.mjs" } }
    ;(plugin.generateBundle as unknown as (this: object, o: object, b: object) => void).call({}, {}, bundle)
    // A literal Worker path lets `bun build --compile --aot --trim` see the
    // code the Worker runs, and no import() of the app means the bootstrap
    // does not bundle a second copy of it.
    expect(boot.code).toContain(`new Worker(new URL("./index.app.mjs", import.meta.url), appWorkerOptions())`)
    expect(boot.code).not.toContain("import(")
    expect(boot.code).not.toContain("__SOLO_APP_FILE__")
    const app = load(input["index.app"])!
    expect(app.indexOf("prepareAppWorker()")).toBeLessThan(app.indexOf(`await import("/app/src/index.tsx")`))
  })

  it("leaves dev servers and explicit inputs alone", () => {
    for (const [config, e] of [
      [{ build: { ssr: "src/index.tsx" } }, { command: "serve", mode: "development" }],
      [{ build: { ssr: "src/index.tsx", rollupOptions: { input: "src/index.tsx" } } }, env],
    ] as const) {
      const plugin = solidUniversal()
      const result = (plugin.config as (config: object, e: object) => { build?: unknown })(config, e)
      expect(result.build).toBeUndefined()
    }
  })
})

describe("solidUniversal app info", () => {
  type Define = { define: Record<string, string> }
  const injected = (root: string, env = { command: "build", mode: "production" }) => {
    const result = (solidUniversal().config as (config: object, e: object) => Define)({ root }, env)
    return JSON.parse(result.define.__SOLO_APP_INFO__)
  }
  const project = (manifest?: object) => {
    const root = mkdtempSync(join(tmpdir(), "solo-app-info-"))
    if (manifest) writeFileSync(join(root, "package.json"), JSON.stringify(manifest))
    return root
  }

  it("injects the app's name, version, identifier and project directory", () => {
    const root = project({ name: "tasks", version: "1.2.3", solo: { identifier: "dev.solo.tasks" } })
    try {
      for (const env of [{ command: "build", mode: "production" }, { command: "serve", mode: "test" }])
        expect(injected(root, env)).toEqual({ name: "tasks", version: "1.2.3", identifier: "dev.solo.tasks", root })
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  it("leaves missing fields null", () => {
    for (const manifest of [undefined, { name: "tasks" }]) {
      const root = project(manifest)
      try {
        expect(injected(root)).toEqual({ name: manifest ? "tasks" : null, version: null, identifier: null, root })
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }
  })

  it("fails the build for an identifier that cannot name a directory", () => {
    for (const identifier of ["", "tasks", "dev/solo.tasks", "../escape", "dev..tasks", 42]) {
      const root = project({ name: "tasks", solo: { identifier } })
      try {
        expect(() => injected(root)).toThrow("solo.identifier")
      } finally {
        rmSync(root, { recursive: true, force: true })
      }
    }
  })
})

describe("solidUniversal automation", () => {
  const pluginDir = new URL("..", import.meta.url).pathname
  const importer = `${pluginDir}root.ts`
  type Context = { resolve: (id: string, importer: string) => Promise<{ id: string }> }
  const context: Context = { resolve: async (id) => ({ id: `resolved:${id}` }) }

  async function automationModule(env: { command: "build" | "serve"; mode: string }, options = {}) {
    const plugin = solidUniversal(options)
    ;(plugin.config as ConfigHook)({}, env)
    const resolveId = plugin.resolveId as unknown as (this: Context, id: string, importer?: string) => Promise<unknown>
    return resolveId.call(context, "./automation-host.js", importer)
  }

  it("leaves stdio automation out of production builds", async () => {
    expect(await automationModule({ command: "build", mode: "production" }))
      .toEqual({ id: "resolved:./automation-off.js" })
  })

  it("keeps it for development, tests, and builds that ask for it", async () => {
    expect(await automationModule({ command: "serve", mode: "test" })).toBeNull()
    expect(await automationModule({ command: "build", mode: "development" })).toBeNull()
    expect(await automationModule({ command: "build", mode: "production" }, { automation: true })).toBeNull()
    process.env.SOLO_AUTOMATION = "1"
    try {
      expect(await automationModule({ command: "build", mode: "production" })).toBeNull()
      expect(await automationModule({ command: "build", mode: "production" }, { automation: false }))
        .toEqual({ id: "resolved:./automation-off.js" })
    } finally {
      delete process.env.SOLO_AUTOMATION
    }
  })

  it("swaps only Solo's own import of the automation host", async () => {
    const plugin = solidUniversal()
    ;(plugin.config as ConfigHook)({}, { command: "build", mode: "production" })
    const resolveId = plugin.resolveId as unknown as (this: Context, id: string, importer?: string) => Promise<unknown>
    expect(await resolveId.call(context, "./automation-host.js", "/app/src/root.ts")).toBeNull()
  })

  it("gives the stand-in the same exports as the automation host", async () => {
    const host = await import("../automation-host.js")
    const off = await import("../automation-off.js")
    expect(Object.keys(off).sort()).toEqual(Object.keys(host).sort())
  })
})

describe.skipIf(process.platform !== "darwin")("solidUniversal native binary", () => {
  const ssrConfig = { root: "/app", build: { ssr: "src/index.tsx" } }
  type Resolve = (id: string, importer?: string) => Promise<unknown>

  function plugin(env: { command: "build" | "serve"; mode: string }) {
    const p = solidUniversal()
    ;(p.config as ConfigHook)(ssrConfig, env)
    return { resolve: p.resolveId as unknown as Resolve, load: p.load as LoadHook }
  }

  it("loads the addon binary directly in production worker-mode builds", async () => {
    const { resolve, load } = plugin({ command: "build", mode: "production" })
    const id = (await resolve("@solo/native")) as string
    const code = load(id)!
    const binary = /^const native = require\("([^"]+)"\)$/m.exec(code)?.[1]
    expect(binary).toMatch(/\/solo-native\.darwin-(universal|arm64|x64)\.node$/)
    // Without napi-rs's loader there is no node:fs in a compiled app.
    expect(code).not.toContain("@solo/native")
  })

  it("refers to the binary relative to each output chunk", async () => {
    const p = solidUniversal()
    ;(p.config as ConfigHook)(ssrConfig, { command: "build", mode: "production" })
    const id = (await (p.resolveId as unknown as Resolve)("@solo/native")) as string
    const binary = /^const native = require\("([^"]+)"\)$/m.exec((p.load as LoadHook)(id)!)![1]
    type Render = (code: string, chunk: { fileName: string }, output: { dir: string }) => { code: string } | null
    const render = p.renderChunk as unknown as Render
    const dir = resolve(dirname(binary), "../../examples/app/dist")
    const code = `const native = require(${JSON.stringify(binary)});\nexport const { a } = native;`
    const name = basename(binary)
    expect(render(code, { fileName: "index.js" }, { dir })?.code)
      .toBe(`const native = require("../../../packages/native/${name}");\nexport const { a } = native;`)
    expect(render(code, { fileName: "assets/index-abc.js" }, { dir })?.code)
      .toContain(`"../../../../packages/native/${name}"`)
    expect(render("export {}", { fileName: "index.js" }, { dir })).toBeNull()
    // Bun resolves from the real path; a symlinked output must not shift it.
    const temp = mkdtempSync(join(tmpdir(), "solo-native-"))
    try {
      const linked = join(temp, "link")
      symlinkSync(dirname(dirname(dir)), linked)
      expect(render(code, { fileName: "index.js" }, { dir: join(linked, "app", "dist") })?.code)
        .toContain(`"../../../packages/native/${name}"`)
    } finally {
      rmSync(temp, { recursive: true, force: true })
    }
  })

  it("exports everything the native bridge does", async () => {
    const { resolve, load } = plugin({ command: "build", mode: "production" })
    const code = load((await resolve("@solo/native")) as string)!
    const names = /export const \{ ([^}]+) \} = native/.exec(code)![1].split(", ")
    const bridge = await import("../_native.js")
    expect(names.sort()).toEqual(Object.keys(bridge).sort())
  })

  it("keeps the native bridge for development builds", async () => {
    const { resolve } = plugin({ command: "build", mode: "development" })
    expect(await resolve("@solo/native")).toEqual({ id: "@solo/solid/_native", external: true })
  })
})
