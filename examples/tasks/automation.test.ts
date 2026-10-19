/// Automation regression: drive the REAL Solid tasks app in its real Solo
/// window over the stdio automation protocol — the same path application
/// controllers use for Solid apps.
///
/// Platform note: live input synthesis reaches the renderer on every platform
/// (macOS via update_window, elsewhere via the UiCommand channel), but only
/// lands in an element once a frame has painted, because an element exposes
/// its text input handler during paint. So this suite verifies launch/
/// handshake, tree lookup, scroll positioning and cleanup everywhere, and
/// pins down the paint-dependent behaviour per platform.

import { describe, expect, it } from "vitest"
import { launch } from "@solo/solid/automation"
import type { App } from "@solo/solid/automation"

const ENTRY = new URL("./dist/index.js", import.meta.url).pathname

// Vitest runs under Node, but on macOS a built Solo app runs under Bun.
async function launchTasksApp(): Promise<App> {
  const app = await launch({ command: "bun", args: [ENTRY] })
  return app
}

describe("tasks app automation", () => {
  it("boots a real window, serves the protocol, and cleans up", async () => {
    const app = await launchTasksApp()
    try {
      const capabilities = await app.call("initialize", {
        protocolVersion: 1,
        client: "tasks-app-regression",
      })
      expect(capabilities.protocolVersion).toBe(1)
      expect(capabilities.capabilities).toContain("tree")
    } finally {
      await app.close()
    }
  })

  it("exposes the task list through the retained tree", async () => {
    const app = await launchTasksApp()
    try {
      const list = await app.getByTestId("task-list").waitFor({ timeoutMs: 10_000 })
      expect(list.type).toBe("div")

      const { text } = await app.call("getAllText", {})
      expect(text).toContain("Finish renderer cleanup")
      expect(text).toContain("Decouple GPUI from Zed")
    } finally {
      await app.close()
    }
  }, 30_000)

  it("programmatic scroll moves the list's scroll offset", async () => {
    const app = await launchTasksApp()
    try {
      const list = await app.getByTestId("task-list").element()

      // Scroll offsets are geometry-derived; wait for the first painted frame.
      const painted = await waitForPaint(app)
      if (!painted) {
        // Headless environment (no display server): frames never draw, scroll
        // handles have no geometry, and every offset clamps to [0, 0]. Assert
        // the commands are at least accepted end-to-end.
        await expect(
          app.call("scrollTo", { elementId: list.id, x: 0, y: -200 })
        ).resolves.toEqual({ ok: true })
        const offset = await app.call("getScrollOffset", { elementId: list.id })
        expect(offset).toBeDefined()
        console.warn("[automation] headless: skipped paint-dependent assertions")
        return
      }

      const before = await app.call("getScrollOffset", { elementId: list.id })
      // Scroll up by 200px (offsets are negative going down).
      await app.call("scrollTo", { elementId: list.id, x: 0, y: -200 })
      let after = await app.call("getScrollOffset", { elementId: list.id })
      for (let i = 0; i < 20 && sameOffset(before.offset, after.offset); i++) {
        await new Promise((resolve) => setTimeout(resolve, 50))
        after = await app.call("getScrollOffset", { elementId: list.id })
      }
      expect(sameOffset(before.offset, after.offset)).toBe(false)

      function sameOffset(a: readonly number[] | null, b: readonly number[] | null): boolean {
        const [ax = 0, ay = 0] = a ?? []
        const [bx = 0, by = 0] = b ?? []
        return ax === bx && ay === by
      }
    } finally {
      await app.close()
    }
  }, 30_000)

  /** Poll until paint bounds exist (a frame has drawn), max ~5s. */
  async function waitForPaint(app: App): Promise<boolean> {
    for (let i = 0; i < 50; i++) {
      const tree = await app.call("getTree", {})
      if (hasBounds(tree.tree)) return true
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
    return false
  }

  function hasBounds(node: unknown): boolean {
    if (typeof node !== "object" || node === null) return false
    if ("bounds" in node && node.bounds != null) return true
    for (const child of (node as { children?: unknown[] }).children ?? []) {
      if (hasBounds(child)) return true
    }
    return false
  }

  it("injects wheel events through the GPUI event boundary", async () => {
    const app = await launchTasksApp()
    try {
      const list = await app.getByTestId("task-list").element()

      const before = await app.call("getScrollOffset", { elementId: list.id })
      await app.call("scrollWheel", { x: 240, y: 320, deltaX: 0, deltaY: -240 })

      let after = await app.call("getScrollOffset", { elementId: list.id })
      for (let i = 0; i < 20 && same(before.offset, after.offset); i++) {
        await new Promise((resolve) => setTimeout(resolve, 50))
        after = await app.call("getScrollOffset", { elementId: list.id })
      }

      // Scroll offsets are geometry-derived from the last painted frame.
      // Headless environments never paint, so offsets stay clamped at [0, 0]
      // there; on any session with a display the wheel must move the list.
      const { text } = await app.call("getPaintedText", {})
      if (text.length > 0) {
        expect(same(before.offset, after.offset), "wheel did not move the list").toBe(false)
      }
      expect(after.offset).toBeDefined()

      function same(a: readonly number[] | null, b: readonly number[] | null): boolean {
        const [ax = 0, ay = 0] = a ?? []
        const [bx = 0, by = 0] = b ?? []
        return ax === bx && ay === by
      }
    } finally {
      await app.close()
    }
  }, 30_000)

  it("types into the composer with fill() and press()", async () => {
    const app = await launchTasksApp()
    try {
      const input = app.getByTestId("new-task-input")

      // The issue's regression snippet. Every call must resolve: before this
      // change the live adapter threw "not live yet" for all four keyboard
      // methods, so Locator.fill and Locator.press were unusable here.
      await expect(input.fill("news")).resolves.toBeUndefined()
      await expect(input.press("cmd-a")).resolves.toBeUndefined()
      await expect(input.press("backspace")).resolves.toBeUndefined()
      await expect(input.fill("İstanbul 世界")).resolves.toBeUndefined()
      await expect(input.press("enter")).resolves.toBeUndefined()

      const { text } = await app.call("getPaintedText", {})
      if (text.length === 0) {
        // Headless (no display server): nothing paints, so no input handler is
        // installed and no keystroke can land in the composer. The commands
        // still round-tripped through the live renderer and resolved, which
        // is all that can be proven without a window.
        console.warn("[automation] headless: skipped paint-dependent assertions")
        return
      }

      // A display is present, so the full chain is exercised: keystrokes reach
      // the focused composer, onChange updates the Solid signal, and Enter
      // submits into the store — observable as a new row in the tree.
      const final = await waitForText(app, "İstanbul 世界")
      expect(final.some((entry) => entry.includes("İstanbul 世界"))).toBe(true)
    } finally {
      await app.close()
    }
  }, 30_000)

  it("rejects a malformed keystroke before it reaches the renderer", async () => {
    const app = await launchTasksApp()
    try {
      // Parsing happens on the calling thread, before the platform split, so
      // the error reaches the controller with the offending token instead of
      // being swallowed by the fire-and-forget UiCommand channel.
      const error = await app
        .call("keystrokes", { keys: "cmd-totally-bogus" })
        .then(() => null)
        .catch((e: unknown) => e)

      expect(error).toBeInstanceOf(Error)
      expect((error as Error).message).toContain("cmd-totally-bogus")
      // The error CODE is deliberately not pinned: on macOS the native method
      // is compiled in, so its throw is a genuine failure and the adapter
      // passes it through (reported as Protocol); off macOS it is re-tagged as
      // Unsupported. The token in the message is what the controller needs.
    } finally {
      await app.close()
    }
  }, 30_000)

  /** Poll `getAllText` until some entry contains `needle`. */
  async function waitForText(app: App, needle: string): Promise<string[]> {
    const deadline = Date.now() + 10_000
    let latest: string[] = []
    while (Date.now() < deadline) {
      const { text } = await app.call("getAllText", {})
      latest = text
      if (text.some((entry) => entry.includes(needle))) return text
      await new Promise((resolve) => setTimeout(resolve, 100))
    }
    return latest
  }
})
