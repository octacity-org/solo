import { handleSoloEvent, MockNativeRenderer } from "@solo/core"
import type { MockElement } from "@solo/core"
import type { WindowOptions } from "@solo/native"
import { createSignal } from "solid-js"
import { describe, expect, it } from "vitest"
import { openWindow, render, Text } from "@solo/solid"

class LifecycleRenderer extends MockNativeRenderer {
  calls: string[] = []
  maximized = false

  showWindow(): void {
    this.calls.push("show")
  }

  closeWindow(): void {
    this.calls.push("close")
  }

  quitApplication(): void {
    this.calls.push("quit")
  }

  setWindowMaximized(maximized: boolean): void {
    this.maximized = maximized
    this.calls.push(maximized ? "maximize" : "restore")
  }

  isWindowMaximized(): boolean {
    return this.maximized
  }
}

describe("application lifecycle", () => {
  it("exposes window and application controls on the render root", () => {
    const renderer = new LifecycleRenderer()
    const root = render(() => <Text>background</Text>, { renderer })

    root.closeWindow()
    root.showWindow()
    root.maximizeWindow()
    expect(root.isWindowMaximized()).toBe(true)
    root.maximizeWindow()
    root.restoreWindow()
    renderer.maximized = true // Native zoom button changed the window state.
    expect(root.isWindowMaximized()).toBe(true)
    root.toggleWindowMaximized()
    expect(root.isWindowMaximized()).toBe(false)
    root.quitApplication()

    expect(renderer.calls).toEqual([
      "close", "show", "maximize", "maximize", "restore", "restore", "quit",
    ])
    root.unmount()
  })
})

/** Records window calls; `windows` holds the roots native would paint. */
class WindowsRenderer extends MockNativeRenderer {
  calls: unknown[][] = []
  windows = new Set<number>()

  openWindow(rootId: number, options?: WindowOptions): void {
    this.calls.push(["open", rootId, options?.title])
    this.windows.add(rootId)
  }

  closeWindow(windowId?: number): void {
    this.calls.push(["close", windowId])
    if (windowId != null && !this.windows.delete(windowId)) throw new Error(`Window ${windowId} is not open`)
  }

  setWindowTitle(title: string, windowId?: number): void {
    this.calls.push(["title", title, windowId])
  }

  text(id: number): string {
    const flatten = (element: MockElement | undefined): string =>
      (element?.text ?? "") + (element?.children ?? []).map((child) => flatten(this.getElement(child))).join("")
    return flatten(this.getElement(id))
  }
}

describe("openWindow", () => {
  it("renders into a parentless root in the same tree, sharing the app's signals", () => {
    const renderer = new WindowsRenderer()
    const [count, setCount] = createSignal(0)
    const root = render(() => <Text>main {count()}</Text>, { renderer })

    const window = openWindow(() => <Text>inspector {count()}</Text>, { title: "Inspector" })
    expect(renderer.calls).toEqual([["open", window.id, "Inspector"]])
    // Committed before openWindow, so native has the subtree to paint.
    expect(renderer.getElement(window.id)?.parentId).toBe(null)
    expect(renderer.text(window.id)).toBe("inspector 0")
    expect(window.id).not.toBe(renderer.rootId)

    setCount(1)
    return Promise.resolve().then(() => {
      expect(renderer.text(window.id)).toBe("inspector 1")
      expect(renderer.text(renderer.rootId!)).toBe("main 1")

      window.setTitle("Renamed")
      expect(renderer.calls.at(-1)).toEqual(["title", "Renamed", window.id])
      root.unmount()
    })
  })

  it("disposes the window's tree when native reports the window closed", () => {
    const renderer = new WindowsRenderer()
    const root = render(() => <Text>main</Text>, { renderer })
    let closes = 0
    const window = openWindow(() => <Text>inspector</Text>, { onClose: () => closes++ })

    // The user closed it: native removed the window, then emitted the event.
    renderer.windows.delete(window.id)
    handleSoloEvent({ elementId: window.id, eventType: "windowClosed" } as never)
    expect(window.closed).toBe(true)
    expect(closes).toBe(1)
    expect(renderer.getElement(window.id)).toBeUndefined()
    expect(renderer.text(renderer.rootId!)).toBe("main")

    // Later calls are no-ops or explicit errors, never native calls.
    window.close()
    expect(() => window.setTitle("x")).toThrow("closed window")
    expect(renderer.calls.filter(([op]) => op === "close")).toEqual([])
    root.unmount()
  })

  it("close() closes the native window first, and remounting closes every window", () => {
    const renderer = new WindowsRenderer()
    render(() => <Text>main</Text>, { renderer })
    const first = openWindow(() => <Text>first</Text>)
    const second = openWindow(() => <Text>second</Text>)

    first.close()
    expect(renderer.calls.at(-1)).toEqual(["close", first.id])
    expect(renderer.getElement(first.id)).toBeUndefined()
    // The windowClosed event that follows a close() is ignored.
    handleSoloEvent({ elementId: first.id, eventType: "windowClosed" } as never)

    const root = render(() => <Text>remounted</Text>, { renderer })
    expect(second.closed).toBe(true)
    expect(renderer.calls.at(-1)).toEqual(["close", second.id])
    expect(renderer.windows.size).toBe(0)
    root.unmount()
  })

  it("requires a running app", () => {
    Reflect.deleteProperty(globalThis, "__soloSolidRenderHost")
    expect(() => openWindow(() => <Text>x</Text>)).toThrow("Call render() before openWindow()")
  })
})
