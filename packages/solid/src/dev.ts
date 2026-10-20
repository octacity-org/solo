/// `solo dev` supervisor (macOS). Runs in a Worker started by cli.ts, beside
/// the GPUI loop on the main thread. Vite rebuilds the app on every change, and
/// each successful build replaces the app Worker. The process, GPUI and the
/// window stay up; the new app's renderer takes over the window (native
/// `enableAppReload`). App state is not preserved across a reload.

import { createRequire } from "node:module"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import { workerData } from "node:worker_threads"
import type { Rollup } from "vite"
import { createAppWorker, prepareAppWorker } from "./worker.js"

// Errors here end the process; leaving the supervisor would end it too.
prepareAppWorker()

const root = resolve(workerData.soloDevRoot as string)
// Dev output stays apart from `dist`, so a running dev session never
// overwrites a production build.
const outDir = join(root, "node_modules", ".solo", "dev")
const log = (message: string): void => console.error(`[solo] ${message}`)

// The project's own Vite, which loads its vite.config with solidUniversal().
const vite = (await import(
  pathToFileURL(createRequire(join(root, "package.json")).resolve("vite")).href
)) as typeof import("vite")

let appFile: string | undefined
let failed = false
let buildStart = 0
let started = false

let app: Worker | undefined
let stopping: Worker | undefined
let restarting = false
let restartAgain = false

function startApp(file: string): void {
  const worker = createAppWorker(pathToFileURL(file), true)
  worker.addEventListener("error", (event) => {
    log(`app error: ${(event as ErrorEvent).message}`)
  })
  worker.addEventListener("close", (event) => {
    if (worker === stopping) return
    app = undefined
    const code = (event as Event & { code?: number }).code ?? 0
    // A clean exit, such as Cmd-Q, ends the session. A crash keeps the window
    // and waits for a fix.
    if (code === 0) process.exit(0)
    log(`app exited with code ${code}; waiting for changes`)
  })
  app = worker
}

function stopApp(): Promise<void> {
  const worker = app
  if (!worker) return Promise.resolve()
  app = undefined
  stopping = worker
  return new Promise((done) => {
    worker.addEventListener("close", () => done())
    worker.terminate()
  })
}

/** Replace the app Worker. Builds that land mid-restart coalesce into one. */
async function restart(): Promise<void> {
  restartAgain = true
  if (restarting) return
  restarting = true
  while (restartAgain) {
    restartAgain = false
    await stopApp()
    startApp(appFile!)
  }
  restarting = false
}

const watcher = (await vite.build({
  root,
  mode: "development",
  logLevel: "warn",
  build: { watch: {}, outDir, emptyOutDir: true },
  plugins: [{
    name: "solo:dev",
    writeBundle(options, bundle) {
      // The app half of solidUniversal()'s macOS worker split.
      const chunk = Object.values(bundle).find(
        (output) => output.type === "chunk" && output.isEntry && output.name.endsWith(".app"),
      )
      appFile = chunk && join(options.dir ?? outDir, chunk.fileName)
    },
  }],
})) as Rollup.RollupWatcher

watcher.on("event", (event) => {
  switch (event.code) {
    case "START":
      failed = false
      buildStart = performance.now()
      break
    case "BUNDLE_END":
      event.result.close()
      break
    case "ERROR":
      failed = true
      event.result?.close()
      // Vite has already printed the error.
      log("build failed; the app keeps running")
      break
    case "END":
      if (failed) break
      if (!appFile) {
        log("no app entry: set build.ssr in vite.config to the app's entry file")
        process.exit(1)
      }
      log(`${started ? "reloaded" : "built"} in ${Math.round(performance.now() - buildStart)} ms`)
      started = true
      void restart()
      break
  }
})
