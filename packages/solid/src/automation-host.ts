/// Stdio automation for a running app: a controller that owns stdin drives the
/// app over the automation protocol. Production builds leave it out:
/// `@solo/solid/vite` swaps this module for automation-off.ts, so a release
/// binary carries neither the server nor the Node streams that stdio needs.
/// Keep the two modules' exports identical.

import type { SoloRenderer } from "@solo/native"
import { InProcessBackend, liveRendererAsTest, serveAutomationStdio } from "@solo/core/automation"

/** Main thread: whether stdin is a TTY, for an app Worker, whose own stdin has no TTY flag. */
export function stdinIsTTY(): boolean {
  return Boolean(process.stdin.isTTY)
}

/** App Worker: report stdin as a TTY because the main thread's stdin is one. */
export function markStdinAsTTY(): void {
  Object.defineProperty(process.stdin, "isTTY", { value: true, configurable: true })
}

/**
 * Serve the protocol over stdio once `renderer` is initialized, if a
 * controller owns stdin: a pipe means a controller, a TTY a human keyboard.
 */
export function automateWhenPiped(renderer: SoloRenderer): void {
  if (process.stdin.isTTY) return
  const init = renderer.init.bind(renderer)
  renderer.init = (options) => {
    init(options)
    serveAutomationStdio(new InProcessBackend(liveRendererAsTest(renderer)))
  }
}
