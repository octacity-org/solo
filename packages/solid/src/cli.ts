#!/usr/bin/env bun
/// `solo dev [root]`: rebuild the app on every change. On macOS only the app
/// Worker is replaced, keeping GPUI and the window alive; on Linux a child
/// process runs the app and is replaced under the same supervisor.

import { resolve } from "node:path"

const [command, root = "."] = process.argv.slice(2)
if (command !== "dev") {
  console.error("Usage: solo dev [root]")
  process.exit(1)
}

if (process.platform === "darwin") {
  const { enableAppReload, enableWorkerMainThread, runMainLoop } = await import("@solo/native")
  const { appWorkerOptions, usesWorkerMode } = await import("./worker.js")
  if (!usesWorkerMode()) {
    console.error("solo dev supports macOS only for now")
    process.exit(1)
  }
  enableWorkerMainThread()
  enableAppReload()
  new Worker(new URL("./dev.js", import.meta.url).href, {
    ...appWorkerOptions(),
    // Bun extension: readable through node:worker_threads in the Worker.
    workerData: { soloDevRoot: resolve(root) },
  } as WorkerOptions)
  runMainLoop()
} else if (process.platform === "linux") {
  const { runLinuxDev } = await import("./dev-linux.js")
  await runLinuxDev(root)
} else {
  console.error("solo dev supports macOS and Linux only for now")
  process.exit(1)
}
