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
///
/// Drives `dist/`, built with `bun run build:automation`: a plain production
/// build leaves stdio automation out.

import { mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { inflateSync } from "node:zlib"
import { describe, expect, it } from "vitest"
import { launch } from "@solo/solid/automation"
import type { App } from "@solo/solid/automation"

// fileURLToPath, not URL.pathname, which is "/D:/..." on Windows.
const ENTRY = fileURLToPath(new URL("./dist/index.js", import.meta.url))

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

  it("clicks through painted bounds and captures a screenshot", async () => {
    const app = await launchTasksApp()
    try {
      const firstTask = app.getByTestId("task-1")
      await firstTask.getByTestId("checkbox-off").waitFor({ timeoutMs: 15_000 })

      // click() hit-tests the element's painted bounds, which are recorded
      // during paint on the UI thread.
      await firstTask.getByTestId("checkbox-off").click()
      await firstTask.getByTestId("checkbox-on").waitFor({ timeoutMs: 5_000 })

      // CI keeps screenshots for failed runs by pointing this at an artifact
      // directory; a local run uses and removes a temp directory.
      const retained = process.env.SOLO_SCREENSHOT_DIR
      const dir = retained ?? mkdtempSync(join(tmpdir(), "tasks-automation-"))
      if (retained) mkdirSync(retained, { recursive: true })
      try {
        const path = await app.screenshot({
          path: join(dir, `window-${process.pid}.png`),
        })
        const bytes = readFileSync(path)
        // A PNG starts with this 8-byte signature.
        expect(bytes.subarray(0, 8)).toEqual(
          Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
        )
        const png = parsePng(bytes)
        expect(png.width).toBeGreaterThan(0)
        expect(png.height).toBeGreaterThan(0)
        // Ensure the screenshot is not a uniform or uninitialized all-black frame
        const uniqueBytes = new Set(png.pixels)
        expect(uniqueBytes.size).toBeGreaterThan(1)
        // Verify pixels match known app colors (#1e1e2e -> 0x1e, 0x2e)
        expect(uniqueBytes.has(0x1e)).toBe(true)
        expect(uniqueBytes.has(0x2e)).toBe(true)
      } finally {
        if (!retained) rmSync(dir, { recursive: true, force: true })
      }
    } finally {
      await app.close()
    }
  }, 30_000)

  function parsePng(buffer: Buffer): { width: number; height: number; pixels: Buffer } {
    let offset = 8
    const idatParts: Buffer[] = []
    let width = 0
    let height = 0
    while (offset < buffer.length) {
      const length = buffer.readUInt32BE(offset)
      const type = buffer.subarray(offset + 4, offset + 8).toString("ascii")
      if (type === "IHDR") {
        width = buffer.readUInt32BE(offset + 8)
        height = buffer.readUInt32BE(offset + 12)
      } else if (type === "IDAT") {
        idatParts.push(buffer.subarray(offset + 8, offset + 8 + length))
      }
      offset += 12 + length
    }
    const decompressed = inflateSync(Buffer.concat(idatParts))
    return { width, height, pixels: decompressed }
  }

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

  it("drives the SQL editor through its controller methods", async () => {
    const app = await launchTasksApp()
    try {
      const editor = app.getByTestId("query-editor")
      await editor.waitFor({ timeoutMs: 10_000 })
      expect(await editor.callMethod("getText")).toBe(
        "SELECT title FROM tasks WHERE completed = false"
      )

      if (!(await waitForPaint(app))) {
        // Headless: no keystroke can land, but the controller still answers.
        expect(typeof (await editor.callMethod("revision"))).toBe("number")
        console.warn("[automation] headless: skipped paint-dependent assertions")
        return
      }

      // Type, then edit from "JS" against the revision before the last key.
      await editor.fill("SELECT 1")
      const revision = (await editor.callMethod("revision")) as number
      await editor.press("2")
      expect(await editor.callMethod("getText")).toBe("SELECT 12")

      const result = await editor.callMethod("applyEdits", {
        revision,
        edits: [{ start: 0, end: 6, text: "select" }],
      })
      expect(result).toEqual({ applied: true, revision: revision + 2 })
      expect(await editor.callMethod("getText")).toBe("select 12")

      // An edit into a range changed since is refused.
      const stale = await editor.callMethod("applyEdits", {
        revision,
        edits: [{ start: 0, end: 3, text: "sel" }],
      })
      expect(stale).toEqual({ applied: false, reason: "conflict", revision: revision + 2 })

      // A selection set from code reaches onSelectionChange.
      await editor.callMethod("setSelections", { selections: [{ anchor: 0, head: 6 }] })
      expect(await editor.callMethod("getSelections")).toEqual([{ anchor: 0, head: 6 }])
      const text = await waitForText(app, "Selection 0-6")
      expect(text).toContain("Selection 0-6")
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

  it.skipIf(process.platform === "darwin")("delivers deep-link URLs from a second launch into the running instance", async () => {
    const app = await launchTasksApp()
    try {
      const { execFile } = await import("node:child_process")
      const { promisify } = await import("node:util")
      const exec = promisify(execFile)

      await exec("bun", [ENTRY, "tasks://open-deep-link-target"])

      const found = await waitForText(app, "Link: tasks://open-deep-link-target")
      expect(found.some((entry) => entry.includes("Link: tasks://open-deep-link-target"))).toBe(true)
    } finally {
      await app.close()
    }
  }, 30_000)

  it("opens a context menu on Windows, verifies items and layout, navigates with keyboard, and dismisses on Escape or outside click", async () => {
    const app = await launchTasksApp()
    try {
      const firstTask = app.getByTestId("task-1")
      const menuBtn = firstTask.getByTestId("task-menu-btn")
      await menuBtn.waitFor({ timeoutMs: 15_000 })

      // 1. Open context menu via right click on task row
      const firstTaskBounds = await firstTask.bounds()
      const firstTaskNode = await firstTask.element()
      await app.call("mouseDown", {
        x: firstTaskBounds.x + firstTaskBounds.width / 2,
        y: firstTaskBounds.y + firstTaskBounds.height / 2,
        button: 2,
        elementId: firstTaskNode.id,
      })
      await waitForStatus(app, "open")

      // 2. Context menu items are drawn via chrome_text in GPUI overlay on Linux and Windows
      if (process.platform !== "darwin") {
        const painted = await app.call("getPaintedText", {})
        expect(painted.text.length).toBeGreaterThan(0)
        expect(painted.text).toContain("Toggle complete")
        expect(painted.text).toContain("Inspect Task")
        expect(painted.text).toContain("Set Priority")
        expect(painted.text).toContain("Delete task")
      }

      // 3. Dismiss via Escape
      await app.call("keystrokes", { keys: "escape" })
      await waitForStatus(app, "cancelled")
      expect(await getStatus(app)).toBe("cancelled")

      // 4. Re-open menu via menu button, navigate with Down arrows (skipping separator and disabled item), select via Enter
      await menuBtn.click()
      await waitForStatus(app, "open")

      // First 'down' highlights item 0 ('toggle')
      // Second 'down' skips separator and disabled 'inspect' item, landing on item 4 ('priority')
      await app.call("keystrokes", { keys: "down down enter" })
      await waitForStatus(app, "priority")
      expect(await getStatus(app)).toBe("priority")

      // 5. Re-open menu, dismiss by clicking outside (backdrop takes click on Linux and Windows)
      if (process.platform !== "darwin") {
        await menuBtn.click()
        await waitForStatus(app, "open")

        await app.call("click", { x: 5, y: 5 })
        await waitForStatus(app, "cancelled")
        expect(await getStatus(app)).toBe("cancelled")
      }

      // 6. Test menu action store effect: toggle task completed
      await menuBtn.click()
      await waitForStatus(app, "open")
      await app.call("keystrokes", { keys: "down enter" })
      await waitForStatus(app, "toggle")
      await firstTask.getByTestId("checkbox-on").waitFor({ timeoutMs: 5_000 })
    } finally {
      await app.close()
    }
  }, 30_000)

  async function getStatus(app: App): Promise<string> {
    return await app.getByTestId("menu-status").textContent()
  }

  async function waitForStatus(app: App, expected: string): Promise<void> {
    const deadline = Date.now() + 10_000
    while (Date.now() < deadline) {
      try {
        const text = await app.getByTestId("menu-status").textContent()
        if (text === expected) return
      } catch {}
      await new Promise((resolve) => setTimeout(resolve, 50))
    }
    const current = await app.getByTestId("menu-status").textContent()
    expect(current).toBe(expected)
  }

  it("opens a secondary window, clicks into it, toggles window state, and handles guarded close", async () => {
    const app = await launchTasksApp()
    try {
      // 1. Initial tree only has main window
      const initialTree = await app.call("getTree", {})
      expect(initialTree.windows ?? []).toHaveLength(0)

      // 2. Click the open-secondary-window button in the main window
      const openBtn = app.getByTestId("open-secondary-window")
      await openBtn.waitFor({ timeoutMs: 15_000 })
      await openBtn.click()

      // 3. Wait for the secondary window to appear in the tree
      const secondaryRoot = app.getByTestId("secondary-window-root")
      await secondaryRoot.waitFor({ timeoutMs: 10_000 })

      // 4. Verify getTree() now includes the secondary window
      const treeWithSecondary = await app.call("getTree", {})
      expect(treeWithSecondary.windows ?? []).toHaveLength(1)

      // 5. Verify the secondary window sees the shared state (total tasks)
      const tasksElem = app.getByTestId("secondary-total-tasks")
      await tasksElem.waitFor({ timeoutMs: 10_000 })
      expect(await tasksElem.textContent()).toContain("Total tasks: 25")

      // 6. Click an element inside the secondary window (verifies automation routing across windows)
      const counterElem = app.getByTestId("secondary-counter")
      await counterElem.waitFor({ timeoutMs: 10_000 })
      expect(await counterElem.textContent()).toContain("Counter: 0")

      const incBtn = app.getByTestId("secondary-increment-btn")
      await incBtn.click()

      let counterText = await counterElem.textContent()
      for (let i = 0; i < 20 && !counterText.includes("Counter: 1"); i++) {
        await new Promise((r) => setTimeout(r, 100))
        counterText = await counterElem.textContent()
      }
      expect(counterText).toContain("Counter: 1")

      // 7. Test window state manipulation from the secondary window
      const testStateBtn = app.getByTestId("secondary-test-state-btn")
      await testStateBtn.click()

      const stateLogElem = app.getByTestId("secondary-state-log")
      let stateLogText = await stateLogElem.textContent()
      for (let i = 0; i < 20 && !stateLogText.includes("resized:420x320"); i++) {
        await new Promise((r) => setTimeout(r, 100))
        stateLogText = await stateLogElem.textContent()
      }
      expect(stateLogText).toMatch(/initial:360x30\d/)
      expect(stateLogText).toContain("resized:420x320")
      expect(stateLogText).toContain("wasMax:false")
      if (process.platform === "linux") {
        // Headless X11 (xvfb without a window manager) does not process EWMH maximize/fullscreen messages
        expect(stateLogText).toContain("isMax:false")
        expect(stateLogText).toContain("fs:false")
      } else {
        expect(stateLogText).toContain("isMax:true")
        expect(stateLogText).toContain("fs:true")
      }
      expect(stateLogText).toContain("restored:true")

      // 8. Test guarded close request: requestClose leaves window open
      const requestCloseBtn = app.getByTestId("secondary-request-close-btn")
      await requestCloseBtn.click()

      // The guarded close request fired, revealing the confirm close button
      const confirmCloseBtn = app.getByTestId("secondary-confirm-close-btn")
      await confirmCloseBtn.waitFor({ timeoutMs: 5_000 })

      // Window is still open!
      const treeStillOpen = await app.call("getTree", {})
      expect(treeStillOpen.windows ?? []).toHaveLength(1)

      // 9. Confirm and close the window
      await confirmCloseBtn.click()

      // 10. Wait for the secondary window to disappear from getTree
      let finalTree = await app.call("getTree", {})
      for (let i = 0; i < 20 && (finalTree.windows ?? []).length > 0; i++) {
        await new Promise((r) => setTimeout(r, 100))
        finalTree = await app.call("getTree", {})
      }
      expect(finalTree.windows ?? []).toHaveLength(0)
    } finally {
      await app.close()
    }
  }, 45_000)

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
