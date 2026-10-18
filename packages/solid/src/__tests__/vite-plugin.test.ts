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
