#!/usr/bin/env bun
/// `solo dev [root]`: rebuild the app on every change and restart only its
/// Worker, keeping the process, GPUI and the window alive. macOS only.

import { resolve } from "node:path"
import { enableAppReload, enableWorkerMainThread, runMainLoop } from "@solo/native"
import { usesWorkerMode } from "./worker.js"

const [command, root = "."] = process.argv.slice(2)
if (command !== "dev") {
  console.error("Usage: solo dev [root]")
  process.exit(1)
}
if (!usesWorkerMode()) {
  console.error("solo dev supports macOS only for now")
  process.exit(1)
}

enableWorkerMainThread()
enableAppReload()
new Worker(new URL("./dev.js", import.meta.url).href, {
  // Bun extension: readable through node:worker_threads in the Worker.
  workerData: { soloStdinIsTTY: Boolean(process.stdin.isTTY), soloDevRoot: resolve(root) },
} as WorkerOptions)
runMainLoop()
