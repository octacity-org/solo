/// Worker mode (macOS, Bun): the process main thread runs GPUI's ordinary
/// blocking loop, and the app runs in a Worker. Used only by the bootstrap
/// that `@solo/solid/vite` generates; applications never import it.
///
/// Bun's web `Worker` is used because its stdio is the process's own. Under
/// `node:worker_threads` (Bun or Node) stdio is relayed through the parent
/// thread's event loop, which never runs again once the main thread enters
/// `runMainLoop`, so console output and stdio automation would stop.

import { enableWorkerMainThread, requestProcessExit, runMainLoop } from "@solo/native"
import { markStdinAsTTY, stdinIsTTY } from "./automation-host.js"

declare const Bun: { readonly isMainThread: boolean }

// What the main thread tells an app Worker, as JSON in this environment
// variable: Bun's web Worker takes an `env`, and reading it needs no
// `node:worker_threads`, whose import alone adds about 11 MB of Node streams
// to a `bun build --compile` binary.
const WORKER_ENV = "SOLO_APP_WORKER"

interface WorkerData {
  stdinIsTTY?: boolean
  /** Set for app Workers that `solo dev` (dev.ts) supervises. */
  dev?: boolean
}

/**
 * Whether the app runs in a Worker: always on macOS, where GPUI needs the
 * process main thread. Elsewhere GPUI owns its own UI thread.
 */
export function usesWorkerMode(): boolean {
  if (process.platform !== "darwin") return false
  if (typeof process.versions.bun !== "string") {
    throw new Error("On macOS, Solo apps run under Bun: Node has no Worker that keeps stdio working.")
  }
  return true
}

/** Options for an app Worker. `dev` leaves ending the process to `solo dev`. */
export function appWorkerOptions(dev = false): WorkerOptions {
  const data: WorkerData = { stdinIsTTY: stdinIsTTY(), dev }
  return { env: { ...process.env, [WORKER_ENV]: JSON.stringify(data) } } as WorkerOptions
}

/** Read what the main thread passed, and drop it so child processes don't inherit it. */
function takeWorkerData(): WorkerData {
  const raw = process.env[WORKER_ENV]
  delete process.env[WORKER_ENV]
  return raw ? (JSON.parse(raw) as WorkerData) : {}
}

/** Start an app Worker. `dev` leaves ending the process to `solo dev`. */
export function createAppWorker(appUrl: URL, dev = false): Worker {
  return new Worker(appUrl.href, appWorkerOptions(dev))
}

/**
 * Main thread: start the app Worker with `start`, then run the native loop.
 * Never returns. The generated bootstrap passes a literal
 * `new Worker(new URL(…, import.meta.url))` so that
 * `bun build --compile --aot` can see the code the Worker runs.
 */
export function runAppWorker(start: () => Worker): void {
  enableWorkerMainThread()
  start()
  runMainLoop()
}

/**
 * App Worker: make every way the Worker can end also end the process, with the
 * same exit code. The main thread is inside `runMainLoop` and never returns to
 * JS, so nothing else would notice the Worker is gone. Under `solo dev` the
 * supervisor decides instead, so a crashed app can wait for the next build.
 */
export function prepareAppWorker(): void {
  if (Bun.isMainThread) return
  const data = takeWorkerData()
  // A Worker's process.stdin.isTTY is undefined, which automation reads as
  // "a controller owns stdin", and would start for a human-run app.
  if (data.stdinIsTTY) markStdinAsTTY()
  // The process ends right after the exit handler, so the text must be written
  // synchronously. Bun's console.error is, to a file or a pipe, and unlike
  // node:fs's writeSync it adds nothing to a `bun build --compile` binary.
  const fail = (error: unknown): void => {
    const text = error instanceof Error ? (error.stack ?? String(error)) : String(error)
    console.error(text)
    process.exit(1)
  }
  process.on("uncaughtException", fail)
  process.on("unhandledRejection", fail)
  if (!data.dev) process.on("exit", (code) => requestProcessExit(code))
}
