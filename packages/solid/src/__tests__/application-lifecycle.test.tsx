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

  getWindowSize(windowId?: number): { width: number; height: number } {
    this.calls.push(windowId === undefined ? "size" : `size ${windowId}`)
    return { width: 1024, height: 700 }
  }
}

describe("application lifecycle", () => {
  it("reads the main window's size from the renderer", () => {
    const renderer = new LifecycleRenderer()
    const root = render(() => <Text>sized</Text>, { renderer })
    expect(root.getWindowSize()).toEqual({ width: 1024, height: 700 })
    expect(renderer.calls).toEqual(["size"])
    root.unmount()
  })

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

  it("routes window-state calls to the main window and to openWindow windows", () => {
    class StateRenderer extends WindowsRenderer {
      fullscreen = new Set<number | undefined>()
      setWindowFullscreen(fullscreen: boolean, windowId?: number): void {
        this.calls.push(["fullscreen", fullscreen, windowId])
        if (fullscreen) this.fullscreen.add(windowId)
        else this.fullscreen.delete(windowId)
      }
      isWindowFullscreen(windowId?: number): boolean {
        return this.fullscreen.has(windowId)
      }
      minimizeWindow(windowId?: number): void {
        this.calls.push(["minimize", windowId])
      }
      setWindowBackground(background: string, windowId?: number): void {
        this.calls.push(["background", background, windowId])
      }
      resizeWindow(width: number, height: number, windowId?: number): void {
        this.calls.push(["resize", width, height, windowId])
      }
      setWindowTabbingIdentifier(identifier: string | null, windowId?: number): void {
        this.calls.push(["tabbing", identifier, windowId])
      }
      toggleWindowTabOverview(windowId?: number): void {
        this.calls.push(["tab overview", windowId])
      }
      setWindowDocumentEdited(edited: boolean, windowId?: number): void {
        this.calls.push(["edited", edited, windowId])
      }
      setWindowDocumentPath(path: string | null, windowId?: number): void {
        this.calls.push(["document", path, windowId])
      }
    }
    const renderer = new StateRenderer()
    const root = render(() => <Text>main</Text>, { renderer })
    root.enterFullscreen()
    expect(root.isFullscreen()).toBe(true)
    root.toggleFullscreen()
    root.minimizeWindow()
    root.setWindowBackground("blurred")
    root.resizeWindow({ width: 640, height: 480 })
    root.setTabbingIdentifier("documents")
    root.setTabbingIdentifier(undefined)
    root.toggleTabOverview()
    root.setDocumentEdited(true)
    root.setDocumentPath("/Users/me/notes.txt")
    root.setDocumentPath(null)
    expect(() => root.setDocumentPath("notes.txt")).toThrow(TypeError)
    expect(() => root.setDocumentPath(undefined as never)).toThrow(TypeError)
    expect(() => root.setDocumentEdited("yes" as never)).toThrow(TypeError)
    expect(renderer.calls).toEqual([
      ["fullscreen", true, undefined],
      ["fullscreen", false, undefined],
      ["minimize", undefined],
      ["background", "blurred", undefined],
      ["resize", 640, 480, undefined],
      ["tabbing", "documents", undefined],
      ["tabbing", null, undefined],
      ["tab overview", undefined],
      ["edited", true, undefined],
      ["document", "/Users/me/notes.txt", undefined],
      ["document", null, undefined],
    ])

    renderer.calls = []
    const window = openWindow(() => <Text>inspector</Text>)
    window.toggleFullscreen()
    expect(window.isFullscreen()).toBe(true)
    window.exitFullscreen()
    window.minimize()
    window.setBackground("transparent")
    window.resize({ width: 300, height: 200 })
    window.setTabbingIdentifier("documents")
    window.toggleTabOverview()
    window.setDocumentEdited(false)
    window.setDocumentPath("/tmp/draft.md")
    expect(renderer.calls.slice(1)).toEqual([
      ["fullscreen", true, window.id],
      ["fullscreen", false, window.id],
      ["minimize", window.id],
      ["background", "transparent", window.id],
      ["resize", 300, 200, window.id],
      ["tabbing", "documents", window.id],
      ["tab overview", window.id],
      ["edited", false, window.id],
      ["document", "/tmp/draft.md", window.id],
    ])

    window.close()
    expect(() => window.resize({ width: 1, height: 1 })).toThrow("closed window")
    expect(() => window.isFullscreen()).toThrow("closed window")
    expect(() => window.toggleTabOverview()).toThrow("closed window")
    expect(() => window.setDocumentEdited(true)).toThrow("closed window")
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

  getWindowSize(windowId?: number): { width: number; height: number } {
    this.calls.push(["size", windowId])
    return { width: 320, height: 200 }
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
      expect(window.size()).toEqual({ width: 320, height: 200 })
      expect(renderer.calls.at(-1)).toEqual(["size", window.id])
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
    expect(() => window.size()).toThrow("closed window")
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

  it("routes close requests to onCloseRequest instead of closing", async () => {
    class GuardRenderer extends WindowsRenderer {
      setWindowCloseGuard(guarded: boolean, windowId?: number): void {
        this.calls.push(["guard", guarded, windowId])
      }
      requestWindowClose(windowId?: number): void {
        this.calls.push(["request", windowId])
      }
    }
    const renderer = new GuardRenderer()
    let mainRequests = 0
    const root = render(() => <Text>main</Text>, { renderer, onCloseRequest: () => void mainRequests++ })
    expect(renderer.calls).toEqual([["guard", true, undefined]])

    let confirm!: () => void
    const requests: number[] = []
    const window = openWindow(() => <Text>inspector</Text>, {
      onCloseRequest: () => {
        requests.push(window.id)
        return new Promise<void>((resolve) => (confirm = resolve))
      },
    })
    expect(renderer.calls.at(-1)).toEqual(["guard", true, window.id])

    window.requestClose()
    root.requestCloseWindow()
    expect(renderer.calls.slice(-2)).toEqual([["request", window.id], ["request", undefined]])

    // Native answers a guarded request with windowCloseRequested.
    handleSoloEvent({ elementId: 0, eventType: "windowCloseRequested" } as never)
    expect(mainRequests).toBe(1)
    handleSoloEvent({ elementId: window.id, eventType: "windowCloseRequested" } as never)
    // Ignored while the first request's promise is pending.
    handleSoloEvent({ elementId: window.id, eventType: "windowCloseRequested" } as never)
    expect(requests).toEqual([window.id])
    expect(window.closed).toBe(false)
    confirm()
    await Promise.resolve()
    await Promise.resolve()
    handleSoloEvent({ elementId: window.id, eventType: "windowCloseRequested" } as never)
    expect(requests).toEqual([window.id, window.id])

    // close() skips the guard.
    window.close()
    expect(window.closed).toBe(true)
    expect(() => window.requestClose()).toThrow("closed window")

    // Remounting without a handler lifts the main window's guard.
    const plain = render(() => <Text>plain</Text>, { renderer })
    expect(renderer.calls.at(-1)).toEqual(["guard", false, undefined])
    handleSoloEvent({ elementId: 0, eventType: "windowCloseRequested" } as never)
    expect(mainRequests).toBe(1)
    plain.unmount()
    expect(renderer.calls.at(-1)).toEqual(["guard", false, undefined])
  })

  it("tells native when a window's onCloseRequest keeps it open", async () => {
    class DeclineRenderer extends WindowsRenderer {
      setWindowCloseGuard(): void {}
      declineWindowClose(windowId?: number): void {
        this.calls.push(["decline", windowId])
      }
    }
    const renderer = new DeclineRenderer()
    const root = render(() => <Text>main</Text>, { renderer })
    const declines = () => renderer.calls.filter((call) => call[0] === "decline")

    // Returning without closing declines at once.
    let approve = false
    const kept = openWindow(() => <Text>kept</Text>, {
      onCloseRequest: () => {
        if (approve) kept.close()
      },
    })
    handleSoloEvent({ elementId: kept.id, eventType: "windowCloseRequested" } as never)
    expect(declines()).toEqual([["decline", kept.id]])

    // Closing from the handler approves; nothing to decline.
    approve = true
    handleSoloEvent({ elementId: kept.id, eventType: "windowCloseRequested" } as never)
    expect(kept.closed).toBe(true)
    expect(declines()).toEqual([["decline", kept.id]])

    // An async handler declines once its promise settles with the window open.
    let settle!: () => void
    const asked = openWindow(() => <Text>asked</Text>, {
      onCloseRequest: () => new Promise<void>((resolve) => (settle = resolve)),
    })
    handleSoloEvent({ elementId: asked.id, eventType: "windowCloseRequested" } as never)
    expect(declines()).toHaveLength(1)
    settle()
    await Promise.resolve()
    await Promise.resolve()
    expect(declines()).toEqual([["decline", kept.id], ["decline", asked.id]])
    root.unmount()
  })

  it("requires a running app", () => {
    Reflect.deleteProperty(globalThis, "__soloSolidRenderHost")
    expect(() => openWindow(() => <Text>x</Text>)).toThrow("Call render() before openWindow()")
  })
})
