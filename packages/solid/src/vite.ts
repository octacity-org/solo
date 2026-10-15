import { transformSync } from "@babel/core"
import type { ParserPlugin } from "@babel/parser"
import { createRequire } from "node:module"
import { dirname } from "node:path"
import type { Plugin } from "vite"

// Solid's package exports select the server build under Node. Native Solo apps
// need the reactive client builds even though Vite produces an SSR bundle.
const req = createRequire(import.meta.url)
const solidJsDist = dirname(req.resolve("solid-js"))
const nativeBridge = "@solo/solid/_native"
// Resolved from @solo/solid, which declares the compiler plugin as a
// dependency. Babel resolves a bare plugin name against the file being
// compiled, so passing the string would make every consumer depend on
// hoisting.
const babelPluginSolid = req.resolve("@solidjs/babel-plugin")
// Anchor on package.json: the signals entry lives under dist/prod/, but the
// dev build is dist/dev.js.
const signalsDist = `${dirname(
  createRequire(req.resolve("solid-js")).resolve("@solidjs/signals/package.json"),
)}/dist`
const universalDist = dirname(req.resolve("@solidjs/universal"))

const PINNED: Record<string, string> = {
  "solid-js": `${solidJsDist}/solid.dev.js`,
  "@solidjs/signals": `${signalsDist}/dev.js`,
  "@solidjs/universal": `${universalDist}/universal.dev.js`,
}

/** Compile Solid JSX for Solo and keep native implementation details external. */
export function solidUniversal(): Plugin {
  let useNativeBridge = false

  return {
    name: "solo:solid-universal",
    enforce: "pre",
    config(_config, env) {
      useNativeBridge = env.command === "build"
    },
    async resolveId(id) {
      if (useNativeBridge && id === "@solo/native") {
        return { id: nativeBridge, external: true }
      }
      return PINNED[id] ?? null
    },
    transform(code, id) {
      const [path] = id.split("?")
      if (!/\.[jt]sx$/.test(path) || path.includes("node_modules")) return

      const parserPlugins: ParserPlugin[] = ["jsx"]
      if (/\.tsx$/.test(path)) parserPlugins.push("typescript")
      const out = transformSync(code, {
        filename: path,
        parserOpts: { plugins: parserPlugins },
        plugins: [
          [
            babelPluginSolid,
            {
              generate: "universal",
              moduleName: "@solo/solid/runtime",
            },
          ],
        ],
        sourceMaps: true,
      })
      if (out == null || out.code == null) return null
      return { code: out.code, map: out.map }
    },
  }
}
