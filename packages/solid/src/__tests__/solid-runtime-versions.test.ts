import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"

type PackageManifest = {
  dependencies?: Record<string, string>
}

const repositoryRoot = new URL("../../../../", import.meta.url)

function readJson<T>(relativePath: string): T {
  return JSON.parse(
    readFileSync(new URL(relativePath, repositoryRoot), "utf8"),
  ) as T
}

describe("Solid runtime dependency contract", () => {
  it("pins the runtime stack to the supported RC versions", () => {
    const packagePaths = [
      "packages/solid/package.json",
      "examples/solid-counter/package.json",
      "examples/tasks/package.json",
      "examples/webview-preview/package.json",
    ]

    for (const packagePath of packagePaths) {
      const manifest = readJson<PackageManifest>(packagePath)
      expect(manifest.dependencies?.["solid-js"], packagePath).toBe(
        "2.0.0-rc.15",
      )
    }

    const solidManifest = readJson<PackageManifest>("packages/solid/package.json")
    expect(solidManifest.dependencies?.["@solidjs/universal"]).toBe(
      "2.0.0-rc.15",
    )
    expect(solidManifest.dependencies?.["@solidjs/babel-plugin"]).toBe(
      "2.0.0-rc.15",
    )
    expect(solidManifest.dependencies?.["babel-preset-solid"]).toBeUndefined()

    // solid-js declares signals as a caret prerelease range, which a fresh
    // install resolves to the newest RC. Mismatched RCs fail to load.
    const rootManifest = readJson<{ overrides?: Record<string, string> }>("package.json")
    expect(rootManifest.overrides?.["@solidjs/signals"]).toBe("2.0.0-rc.15")
  })

  it("resolves one RC.15 runtime and signals package in bun.lock", () => {
    // bun.lock is JSON5-like (it contains trailing commas), so keep this
    // contract independent of a parser dependency and inspect package records.
    const lock = readFileSync(new URL("bun.lock", repositoryRoot), "utf8")
    const solidRuntimeEntries = lock.match(
      /"solid-js": \["solid-js@[^\"]+"/g,
    ) ?? []
    const signalsEntries = lock.match(
      /"@solidjs\/signals": \["@solidjs\/signals@[^\"]+"/g,
    ) ?? []

    expect(solidRuntimeEntries).toHaveLength(1)
    expect(signalsEntries).toHaveLength(1)
    expect(solidRuntimeEntries[0]).toBe(
      '"solid-js": ["solid-js@2.0.0-rc.15"',
    )
    expect(signalsEntries[0]).toBe(
      '"@solidjs/signals": ["@solidjs/signals@2.0.0-rc.15"',
    )
    expect(lock).toMatch(
      /"@solidjs\/babel-plugin": \["@solidjs\/babel-plugin@2\.0\.0-rc\.15"/,
    )
    expect(lock).not.toMatch(/"babel-preset-solid": \[/)
    expect(lock).not.toMatch(
      /"(?:solid-js|@solidjs\/signals)": \["(?:solid-js|@solidjs\/signals)@2\.0\.0-rc\.[123]"/,
    )
  })
})
