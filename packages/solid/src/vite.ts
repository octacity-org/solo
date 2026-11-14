import { transformSync } from "@babel/core"
import type { ParserPlugin } from "@babel/parser"
import { existsSync, readFileSync, realpathSync } from "node:fs"
import { createRequire } from "node:module"
import { basename, dirname, extname, posix, relative, resolve, sep } from "node:path"
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
// Stdio automation (automation-host.ts) and its no-op stand-in, swapped in by
// builds without automation.
const pluginDir = dirname(fileURLToPath(import.meta.url))
const AUTOMATION_HOST = "./automation-host.js"
const AUTOMATION_OFF = "./automation-off.js"
// Production worker-mode builds load the addon binary directly instead of
// through napi-rs's generated loader (@solo/native's index.js), whose
// `node:fs` keeps about 0.5 MB of Bun's fs in a `bun build --compile` binary.
// Worker mode only: it runs under Bun, whose ES modules have `require`.
const NATIVE_ID = "\0solo:native"
// What _native.ts re-exports; a test keeps the two equal.
const NATIVE_EXPORTS = [
  "SoloRenderer",
  "addRecentDocument",
  "beep",
  "createDirectory",
  "enableWorkerMainThread",
  "getKeyboardLayout",
  "getScreens",
  "getThermalState",
  "hideApplication",
  "hideOtherApplications",
  "moveToTrash",
  "openExternalUrl",
  "openPath",
  "readClipboardText",
  "registerUrlScheme",
  "requestProcessExit",
  "restartApplication",
  "revealPath",
  "runMainLoop",
  "setDockMenu",
  "shouldAutoHideScrollbars",
  "showAllApplications",
  "writeClipboardText",
]
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

/**
 * The macOS addon binary that @solo/native's loader would pick on this
 * machine, in the loader's order: universal, then this architecture, each
 * as a file beside the loader and then as its platform package.
 */
function macosNativeBinary(): string | undefined {
  const nativeDir = dirname(req.resolve("@solo/native/package.json"))
  const nativeReq = createRequire(`${nativeDir}/`)
  for (const target of ["darwin-universal", `darwin-${process.arch}`]) {
    const local = `${nativeDir}/solo-native.${target}.node`
    if (existsSync(local)) return local
    try {
      return nativeReq.resolve(`@solo/native-${target}`)
    } catch {
      // Not installed; try the next target.
    }
  }
}

/**
 * `dir` with symlinks resolved, as Bun resolves a require() from the file's
 * real path (macOS temp dirs live under the /var -> /private/var link). The
 * output may not exist yet, so resolve its nearest existing ancestor.
 */
function realDir(dir: string): string {
  if (existsSync(dir)) return realpathSync(dir)
  const parent = dirname(dir)
  return parent === dir ? dir : resolve(realDir(parent), basename(dir))
}

// A reverse-DNS app identifier, safe to use as one directory name.
const IDENTIFIER = /^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$/

/**
 * `define` entry that gives app.ts the app's package.json identity and the
 * project directory (its resources while running under bun or node).
 */
function appInfoDefine(root: string): Record<string, string> {
  const file = resolve(root, "package.json")
  const manifest = existsSync(file) ? JSON.parse(readFileSync(file, "utf8")) : {}
  const identifier = manifest.solo?.identifier
  if (identifier !== undefined && (typeof identifier !== "string" || !IDENTIFIER.test(identifier))) {
    throw new Error(
      `${file}: solo.identifier must be a reverse-DNS name such as "com.example.app", got ${JSON.stringify(identifier)}`,
    )
  }
  const text = (value: unknown) => (typeof value === "string" ? value : null)
  const info = { name: text(manifest.name), version: text(manifest.version), identifier: identifier ?? null, root }
  return { __SOLO_APP_INFO__: JSON.stringify(info) }
}

export interface SolidUniversalOptions {
  /**
   * Serve the automation protocol over stdio when a controller owns stdin, so
   * tests can drive the app (`@solo/solid/automation`). Production builds
   * leave it out unless the `SOLO_AUTOMATION=1` environment variable is set:
   * it keeps Node's stdio streams in the app, several megabytes of a
   * `bun build --compile` binary. Default: on, except in production builds.
   */
  automation?: boolean
}

/** Compile Solid JSX for Solo and keep native implementation details external. */
export function solidUniversal(options: SolidUniversalOptions = {}): Plugin {
  let useNativeBridge = false
  let pinned = DEV_BUILDS
  let split: string | undefined
  let automation = true
  let nativeBinary: string | undefined

  return {
    name: "solo:solid-universal",
    enforce: "pre",
    config(config, env) {
      useNativeBridge = env.command === "build"
      const production = env.command === "build" && env.mode === "production"
      pinned = production ? PRODUCTION_BUILDS : DEV_BUILDS
      automation = options.automation ?? (!production || process.env.SOLO_AUTOMATION === "1")
      split = workerBuild(config, env.command)
      // Without a binary found here, the loader's other sources still apply.
      nativeBinary = production && split ? macosNativeBinary() : undefined
      const define = appInfoDefine(resolve(config.root ?? process.cwd()))
      if (!split) return { define }
      const name = basename(split, extname(split))
      return {
        define,
        build: {
          ssr: true,
          rollupOptions: { input: { [name]: BOOT_ID, [`${name}.app`]: APP_ID } },
        },
      }
    },
    async resolveId(id, importer) {
      if (useNativeBridge && id === "@solo/native") {
        if (nativeBinary) return NATIVE_ID
        return { id: nativeBridge, external: true }
      }
      if (!automation && id === AUTOMATION_HOST && importer && dirname(importer) === pluginDir) {
        return this.resolve(AUTOMATION_OFF, importer, { skipSelf: true })
      }
      if (split && (id === BOOT_ID || id === APP_ID)) return id
      return pinned[id] ?? null
    },
    load(id) {
      if (nativeBinary && id === NATIVE_ID) {
        // Bun loads a .node file only through require(). A literal path lets
        // `bun build --compile` embed the binary.
        return [
          `const native = require(${JSON.stringify(nativeBinary)})`,
          `export const { ${NATIVE_EXPORTS.join(", ")} } = native`,
        ].join("\n")
      }
      if (!split) return
      if (id === BOOT_ID) {
        // The Worker's path stays a literal: `bun build --compile --aot --trim`
        // keeps the interpreter for a Worker it cannot resolve. Only macOS
        // builds are split, so there is no in-process fallback to import: it
        // would also bundle the whole app a second time into the bootstrap.
        const wrongPlatform = "This Solo app was built on macOS; build it on this platform to run it here."
        return [
          `import { appWorkerOptions, runAppWorker, usesWorkerMode } from ${JSON.stringify(workerModule)}`,
          `if (!usesWorkerMode()) throw new Error(${JSON.stringify(wrongPlatform)})`,
          `runAppWorker(() => new Worker(new URL(${JSON.stringify(APP_FILE)}, import.meta.url), appWorkerOptions()))`,
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
    renderChunk(code, chunk, output) {
      // An absolute path would tie the build to this checkout; require() the
      // binary relative to the chunk instead. Only that string changes length,
      // so line mappings still hold.
      if (!nativeBinary || !output.dir) return null
      const absolute = JSON.stringify(nativeBinary)
      if (!code.includes(absolute)) return null
      const path = relative(realDir(dirname(resolve(output.dir, chunk.fileName))), nativeBinary).split(sep).join("/")
      return { code: code.replaceAll(absolute, JSON.stringify(path.startsWith(".") ? path : `./${path}`)), map: null }
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
