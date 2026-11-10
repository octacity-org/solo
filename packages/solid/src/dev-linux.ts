/// `solo dev` supervisor (Linux). Vite rebuilds the app on every change and
/// each successful build replaces the child process that runs it. The
/// supervisor, its watcher and the exit policy stay up; app state is not
/// preserved across a reload. A clean exit, such as closing the last window,
/// ends the session; a crash waits for the next good build.

import { spawn, type ChildProcess } from "node:child_process"
import { createRequire } from "node:module"
import { join, resolve } from "node:path"
import { pathToFileURL } from "node:url"
import type { Rollup } from "vite"

const log = (message: string): void => console.error(`[solo] ${message}`)

export async function runLinuxDev(projectRoot: string): Promise<void> {
  const root = resolve(projectRoot)
  // Dev output stays apart from `dist`, so a running session never overwrites
  // a production build.
  const outDir = join(root, "node_modules", ".solo", "dev")

  // The project's own Vite, which loads its vite.config with solidUniversal().
  const vite = (await import(
    pathToFileURL(createRequire(join(root, "package.json")).resolve("vite")).href
  )) as typeof import("vite")

  let appFile: string | undefined
  let failed = false
  let buildStart = 0
  let started = false

  let app: ChildProcess | undefined
  let stopping: ChildProcess | undefined
  let restarting = false
  let restartAgain = false

  const startApp = (file: string): void => {
    const child = spawn(process.execPath, [file], {
      cwd: root,
      // The app owns this stdio, including the automation protocol when a
      // controller launches the supervisor with pipes.
      stdio: "inherit",
      env: process.env,
    })
    child.on("error", (error) => log(`app error: ${error.message}`))
    child.on("exit", (code, signal) => {
      if (child === stopping) return
      if (app !== child) return
      app = undefined
      if (code === 0) process.exit(0)
      log(`app exited (${code ?? signal}); waiting for changes`)
    })
    app = child
  }

  const stopApp = (): Promise<void> => {
    const child = app
    if (!child) return Promise.resolve()
    app = undefined
    stopping = child
    return new Promise((done) => {
      child.once("exit", () => done())
      child.kill()
    })
  }

  /** Replace the app process. Builds that land mid-restart coalesce into one. */
  const restart = async (): Promise<void> => {
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
      name: "solo:dev-linux",
      writeBundle(options, bundle) {
        // Linux has no worker split: the app is the single entry chunk.
        const chunk = Object.values(bundle).find(
          (output) => output.type === "chunk" && output.isEntry,
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

  let shuttingDown = false
  const shutdown = (): void => {
    if (shuttingDown) return
    shuttingDown = true
    void watcher.close().finally(() => {
      void stopApp().finally(() => process.exit(0))
    })
  }
  process.on("SIGINT", shutdown)
  process.on("SIGTERM", shutdown)

  // The watcher keeps the process alive; exits go through `shutdown`.
  await new Promise<void>(() => {})
}
