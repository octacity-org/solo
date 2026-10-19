/// Worker mode (macOS, Bun): the process main thread runs GPUI's ordinary
/// blocking loop, and the app runs in a Worker. Used only by the bootstrap
/// that `@solo/solid/vite` generates; applications never import it.
///
/// Bun's web `Worker` is used because its stdio is the process's own. Under
/// `node:worker_threads` (Bun or Node) stdio is relayed through the parent
/// thread's event loop, which never runs again once the main thread enters
/// `runMainLoop`, so console output and stdio automation would stop.

import { isMainThread, workerData } from "node:worker_threads"
import { writeSync } from "node:fs"
import { enableWorkerMainThread, requestProcessExit, runMainLoop } from "@solo/native"

const STDIN_IS_TTY = "soloStdinIsTTY"

/** Whether this process can run the app in a Worker. */
export function supportsWorkerMode(): boolean {
  return process.platform === "darwin" && typeof process.versions.bun === "string"
}

/** Main thread: start the app Worker, then run the native loop. Never returns. */
export function runAppWorker(appUrl: URL): void {
  enableWorkerMainThread()
  new Worker(appUrl.href, {
    // Bun extension: readable through node:worker_threads in the Worker.
    workerData: { [STDIN_IS_TTY]: Boolean(process.stdin.isTTY) },
  } as WorkerOptions)
  runMainLoop()
}

/**
 * App Worker: make every way the Worker can end also end the process, with the
 * same exit code. The main thread is inside `runMainLoop` and never returns to
 * JS, so nothing else would notice the Worker is gone.
 */
export function prepareAppWorker(): void {
  if (isMainThread) return
  // A Worker's process.stdin.isTTY is undefined, which root.ts would read as
  // "a controller owns stdin" and start automation for a human-run app.
  if (workerData?.[STDIN_IS_TTY]) {
    Object.defineProperty(process.stdin, "isTTY", { value: true, configurable: true })
  }
  // Written synchronously: the process ends right after the exit handler.
  const fail = (error: unknown): void => {
    const text = error instanceof Error ? (error.stack ?? String(error)) : String(error)
    writeSync(2, `${text}\n`)
    process.exit(1)
  }
  process.on("uncaughtException", fail)
  process.on("unhandledRejection", fail)
  process.on("exit", (code) => requestProcessExit(code))
}
