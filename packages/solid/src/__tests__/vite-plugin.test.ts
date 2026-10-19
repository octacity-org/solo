import { describe, expect, it } from "vitest"
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
    expect(boot.code).toContain(`new URL("./index.app.mjs", import.meta.url)`)
    const app = load(input["index.app"])!
    expect(app.indexOf("prepareAppWorker()")).toBeLessThan(app.indexOf(`await import("/app/src/index.tsx")`))
  })

  it("leaves dev servers and explicit inputs alone", () => {
    for (const [config, e] of [
      [{ build: { ssr: "src/index.tsx" } }, { command: "serve", mode: "development" }],
      [{ build: { ssr: "src/index.tsx", rollupOptions: { input: "src/index.tsx" } } }, env],
    ] as const) {
      const plugin = solidUniversal()
      expect((plugin.config as (config: object, e: object) => unknown)(config, e)).toBeUndefined()
    }
  })
})
