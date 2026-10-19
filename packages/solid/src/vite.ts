import { transformSync } from "@babel/core"
import type { ParserPlugin } from "@babel/parser"
import { createRequire } from "node:module"
import { basename, dirname, extname, posix, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import type { Plugin, UserConfig } from "vite"

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

// Tests and development keep Solid's dev builds for their diagnostics.
const DEV_BUILDS: Record<string, string> = {
  "solid-js": `${solidJsDist}/solid.dev.js`,
  "@solidjs/signals": `${signalsDist}/dev.js`,
  "@solidjs/universal": `${universalDist}/universal.dev.js`,
}

// Production app builds ship the smaller, faster client builds.
const PRODUCTION_BUILDS: Record<string, string> = {
  "solid-js": `${solidJsDist}/solid.js`,
  "@solidjs/signals": `${signalsDist}/prod/index.js`,
  "@solidjs/universal": `${universalDist}/universal.js`,
}

// Worker mode (macOS): the built entry becomes a small bootstrap that runs
// GPUI on the main thread and the app in a Worker; see worker.ts.
const workerModule = fileURLToPath(new URL("./worker.js", import.meta.url))
const BOOT_ID = "\0solo:boot"
const APP_ID = "\0solo:app"
// Replaced with the app entry's emitted path, whose extension (.js or .mjs)
// Vite picks from the project's package type.
const APP_FILE = "__SOLO_APP_FILE__"

/** On macOS, split an SSR app build into `<name>.js` (bootstrap) and
 *  `<name>.app.js` (the app, run in a Worker). Returns the app entry. */
function workerBuild(config: UserConfig, command: string): string | undefined {
  const ssr = config.build?.ssr
  if (command !== "build" || process.platform !== "darwin" || typeof ssr !== "string") return
  if (config.build?.rollupOptions?.input) return
  return resolve(config.root ?? process.cwd(), ssr)
}

/** Compile Solid JSX for Solo and keep native implementation details external. */
export function solidUniversal(): Plugin {
  let useNativeBridge = false
  let pinned = DEV_BUILDS
  let split: string | undefined

  return {
    name: "solo:solid-universal",
    enforce: "pre",
    config(config, env) {
      useNativeBridge = env.command === "build"
      pinned = env.command === "build" && env.mode === "production"
        ? PRODUCTION_BUILDS
        : DEV_BUILDS
      split = workerBuild(config, env.command)
      if (!split) return
      const name = basename(split, extname(split))
      return {
        build: {
          ssr: true,
          rollupOptions: { input: { [name]: BOOT_ID, [`${name}.app`]: APP_ID } },
        },
      }
    },
    async resolveId(id) {
      if (useNativeBridge && id === "@solo/native") {
        return { id: nativeBridge, external: true }
      }
      if (split && (id === BOOT_ID || id === APP_ID)) return id
      return pinned[id] ?? null
    },
    load(id) {
      if (!split) return
      // Node has no stdio-preserving Worker, so it keeps the embedded loop.
      if (id === BOOT_ID) {
        return [
          `import { runAppWorker, supportsWorkerMode } from ${JSON.stringify(workerModule)}`,
          `const app = new URL(${JSON.stringify(APP_FILE)}, import.meta.url)`,
          `if (supportsWorkerMode()) runAppWorker(app)`,
          `else await import(app.href)`,
        ].join("\n")
      }
      if (id === APP_ID) {
        return [
          `import { prepareAppWorker } from ${JSON.stringify(workerModule)}`,
          `prepareAppWorker()`,
          `await import(${JSON.stringify(split)})`,
        ].join("\n")
      }
    },
    generateBundle(_options, bundle) {
      if (!split) return
      const chunks = Object.values(bundle).filter((output) => output.type === "chunk")
      const boot = chunks.find((chunk) => chunk.facadeModuleId === BOOT_ID)
      const app = chunks.find((chunk) => chunk.facadeModuleId === APP_ID)
      if (!boot || !app) return this.error("Solo worker-mode entries are missing from the bundle")
      const path = posix.relative(posix.dirname(boot.fileName), app.fileName)
      boot.code = boot.code.replace(APP_FILE, `./${path}`)
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
