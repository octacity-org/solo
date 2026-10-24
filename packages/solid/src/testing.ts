/// macOS-only GPU-backed test harness for @solo/solid.
///
/// Mirrors packages/react/src/testing.ts: mounts Solid trees through the
/// native TestSoloRenderer so tests exercise the same GPUI pipeline as
/// production. On non-macOS builds hasNativeTestRenderer is false and the
/// suites using this harness skip themselves; headless coverage comes from
/// MockNativeRenderer-based tests instead.

import type { EventPayload } from "@solo/native"
import { flush as flushSolid } from "solid-js"
import { clearEventHandlers, handleSoloEvent, wrapWithBatching } from "@solo/core"
import type { WindowOptions } from "@solo/native"
import { setWindowHost } from "./root.js"
import { mount as mountTree, resetIdCounter, flushMutations } from "./runtime.js"

/** Coordinates are window-local; `elementId` picks that element's window. */
interface NativeTestRendererApi {
  flush(): void
  drainEvents(): EventPayload[]
  simulateKeystrokes(keystrokes: string, elementId?: number): void
  focusElement(elementId: number): void
  simulateClick(x: number, y: number, button?: number, elementId?: number): void
  simulateKeyDown(keystroke: string, isHeld?: boolean, elementId?: number): void
  simulateKeyUp(keystroke: string, elementId?: number): void
  simulateMouseDown(x: number, y: number, button?: number, elementId?: number): void
  simulateMouseUp(x: number, y: number, button?: number, elementId?: number): void
  simulateMouseMove(x: number, y: number, pressedButton?: number, elementId?: number): void
  simulateScrollWheel(x: number, y: number, deltaX: number, deltaY: number, elementId?: number): void
  openWindow(rootId: number, options?: WindowOptions): void
  closeWindow(windowId?: number): void
  getWindowSize(windowId?: number): { width: number; height: number }
  getWindowAutomationTrees(): string
  scrollToItem(elementId: number, index: number): void
  scrollTo(elementId: number, x: number, y: number): void
  getScrollOffset(elementId: number): number[] | null
  clearSelection(): void
  getElementBounds(elementId: number): number[] | null
  getAllText(): string[]
  getPaintedText(): string[]
  getAutomationTree(): string
  getTreeJson(): string
  dragSelect(x1: number, y1: number, x2: number, y2: number): void
  getSelectedText(): string | null
  captureScreenshot(path: string): void
}

export interface NativeTestElement {
  id: number
  type: string
  style: Record<string, unknown>
  text: string | null
  events: Set<string>
  children: number[]
  parentId: number | null
  customProps?: Record<string, unknown>
}

interface SolidNativeTestRendererApi extends NativeTestRendererApi {
  findByType(type: string): NativeTestElement[]
  nativeSimulateKeystrokes(elementId: number, keystrokes: string): void
  nativeSimulateClick(x: number, y: number, elementId?: number): void
  nativeSimulateKeyDown(elementId: number, keystroke: string, isHeld?: boolean): void
  nativeSimulateKeyUp(elementId: number, keystroke: string): void
  nativeSimulateMouseDown(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseUp(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseMove(x: number, y: number, pressedButton?: number, elementId?: number): void
  nativeSimulateScrollWheel(x: number, y: number, deltaX: number, deltaY: number, elementId?: number): void
}

interface NativeTestRendererConstructor {
  new (): NativeTestRendererApi
}

let NativeTestRenderer: NativeTestRendererConstructor | null = null
try {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const native = require("@solo/native") as {
    TestSoloRenderer?: NativeTestRendererConstructor
  }
  if (native.TestSoloRenderer) {
    NativeTestRenderer = native.TestSoloRenderer
  }
} catch {
  // Native module unavailable — harness stays disabled.
}

export const hasNativeTestRenderer = NativeTestRenderer != null

/**
 * Mounted Solid tree against the native GPU-backed renderer. While mounted,
 * openWindow() opens offscreen windows on it. Simulation methods that take an
 * `elementId` act in the window that element is painted in.
 */
export interface SolidNativeTestRoot {
  renderer: SolidNativeTestRendererApi
  render: (code: () => unknown) => void
  unmount: () => void

  // ── inspection ──
  getAllText(): string[]
  getPaintedText(): string[]
  findByType(type: string): NativeTestElement[]
  getElement(elementId: number): NativeTestElement | undefined
  getElementBounds(elementId: number): number[] | null
  // ── simulation ──
  nativeSimulateKeystrokes(elementId: number, keystrokes: string): void
  simulateKeystrokes(keystrokes: string): void
  nativeSimulateClick(x: number, y: number, elementId?: number): void
  nativeSimulateKeyDown(elementId: number, keystroke: string, isHeld?: boolean): void
  nativeSimulateKeyUp(elementId: number, keystroke: string): void
  nativeSimulateMouseDown(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseUp(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseMove(x: number, y: number, pressedButton?: number, elementId?: number): void
  focusElement(elementId: number): void
  nativeSimulateScrollWheel(x: number, y: number, deltaX: number, deltaY: number, elementId?: number): void
  scrollToItem(elementId: number, index: number): void
  scrollTo(elementId: number, x: number, y: number): void
  getScrollOffset(elementId: number): number[] | null
  clearSelection(): void
  dragSelect(x1: number, y1: number, x2: number, y2: number): string | null
  captureScreenshot(path: string): void
}

export function createSolidNativeTestRoot(): SolidNativeTestRoot {
  if (!NativeTestRenderer) {
    throw new Error(
      "Native TestSoloRenderer not available. Build with test-support to run tests."
    )
  }
  const native = new NativeTestRenderer()
  const batched = wrapWithBatching(native as never)

  let dispose: (() => void) | null = null

  /** Returns whether there were any events. */
  function dispatchNativeEvents(): boolean {
    let dispatched = false
    for (;;) {
      const events = native.drainEvents()
      if (events.length === 0) return dispatched
      dispatched = true
      for (const event of events) handleSoloEvent(event)
      // Force the mutations queued by Solid's handlers into the native tree.
      flushMutations()
    }
  }

  /**
   * Draw until a draw raises no events. A data-driven virtual list reports
   * the rows it needs after layout, and mounting them takes another draw.
   */
  function settle(): void {
    for (let pass = 0; pass < 8; pass++) {
      flushSolid()
      flushMutations()
      native.flush()
      if (!dispatchNativeEvents()) return
    }
  }

  function buildElementMap(): Map<number, NativeTestElement> {
    const json = JSON.parse(native.getTreeJson()) as Record<string, unknown> | null
    const map = new Map<number, NativeTestElement>()
    const walk = (node: Record<string, unknown> | null, parentId: number | null): void => {
      if (!node) return
      const id = node.id as number
      const children = (node.children ?? []) as Array<Record<string, unknown>>
      map.set(id, {
        id,
        type: node.type as string,
        style: (node.style ?? {}) as Record<string, unknown>,
        text: (node.text ?? null) as string | null,
        events: new Set((node.events ?? []) as string[]),
        children: children.map((child) => child.id as number),
        parentId,
        ...(node.customProps
          ? { customProps: node.customProps as Record<string, unknown> }
          : {}),
      })
      for (const child of children) walk(child, id)
    }
    walk(json, null)
    // Windows from openWindow are parentless trees of their own.
    for (const window of JSON.parse(native.getWindowAutomationTrees()) as Array<Record<string, unknown>>) {
      walk(window, null)
    }
    return map
  }

  const root: SolidNativeTestRoot = {
    renderer: native as SolidNativeTestRendererApi,
    render(code) {
      if (dispose) dispose()
      // IDs restart below, so no window from the previous tree may survive.
      setWindowHost(native as never)
      resetIdCounter()
      clearEventHandlers()
      dispose = mountTree(code as never, batched as never)
      settle()
    },
    unmount() {
      setWindowHost(undefined)
      dispose?.()
      dispose = null
      flushMutations()
      native.flush()
    },

    getAllText: () => native.getAllText().filter((text) => text.length > 0),
    getPaintedText() {
      settle()
      return native.getPaintedText()
    },

    findByType(type) {
      return [...buildElementMap().values()].filter((element) => element.type === type)
    },

    getElement: (id) => buildElementMap().get(id),

    getElementBounds: (id) => native.getElementBounds(id),

    nativeSimulateKeystrokes(elementId, keystrokes) {
      native.flush()
      native.focusElement(elementId)
      native.simulateKeystrokes(keystrokes, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateClick(x, y, elementId) {
      native.flush()
      native.simulateClick(x, y, undefined, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateKeyDown(elementId, keystroke, isHeld) {
      native.flush()
      native.focusElement(elementId)
      native.simulateKeyDown(keystroke, isHeld, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateKeyUp(elementId, keystroke) {
      native.flush()
      native.focusElement(elementId)
      native.simulateKeyUp(keystroke, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateMouseDown(x, y, button, elementId) {
      native.flush()
      native.simulateMouseDown(x, y, button, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateMouseUp(x, y, button, elementId) {
      native.flush()
      native.simulateMouseUp(x, y, button, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateMouseMove(x, y, pressedButton, elementId) {
      native.flush()
      native.simulateMouseMove(x, y, pressedButton, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    focusElement(elementId) {
      native.flush()
      native.focusElement(elementId)
      dispatchNativeEvents()
    },

    dragSelect(x1: number, y1: number, x2: number, y2: number) {
      native.dragSelect(x1, y1, x2, y2)
      return native.getSelectedText()
    },

    simulateKeystrokes(keystrokes: string) {
      native.flush()
      native.simulateKeystrokes(keystrokes)
      dispatchNativeEvents()
      native.flush()
    },

    nativeSimulateScrollWheel(x: number, y: number, deltaX: number, deltaY: number, elementId?: number) {
      native.flush()
      native.simulateScrollWheel(x, y, deltaX, deltaY, elementId)
      dispatchNativeEvents()
      native.flush()
    },

    scrollToItem(elementId: number, index: number) {
      native.flush()
      native.scrollToItem(elementId, index)
      settle()
    },

    scrollTo(elementId: number, x: number, y: number) {
      native.flush()
      native.scrollTo(elementId, x, y)
      settle()
    },

    getScrollOffset(elementId: number) {
      native.flush()
      return native.getScrollOffset(elementId)
    },

    clearSelection() {
      native.clearSelection()
      native.flush()
    },

    captureScreenshot(path: string) {
      native.flush()
      native.captureScreenshot(path)
    },
  }

  root.renderer = new Proxy(native, {
    get(target, property, receiver) {
      switch (property) {
        case "findByType": return root.findByType
        case "getAllText": return root.getAllText
        case "getPaintedText": return root.getPaintedText
        case "scrollToItem": return root.scrollToItem
        case "scrollTo": return root.scrollTo
        case "captureScreenshot": return root.captureScreenshot
        case "dragSelect": return root.dragSelect
        case "simulateKeystrokes": return root.simulateKeystrokes
        case "focusElement": return root.focusElement
        case "nativeSimulateKeystrokes": return root.nativeSimulateKeystrokes
        case "nativeSimulateClick": return root.nativeSimulateClick
        case "nativeSimulateKeyDown": return root.nativeSimulateKeyDown
        case "nativeSimulateKeyUp": return root.nativeSimulateKeyUp
        case "nativeSimulateMouseDown": return root.nativeSimulateMouseDown
        case "nativeSimulateMouseUp": return root.nativeSimulateMouseUp
        case "nativeSimulateMouseMove": return root.nativeSimulateMouseMove
        case "nativeSimulateScrollWheel": return root.nativeSimulateScrollWheel
        default: {
          const value = Reflect.get(target, property, receiver) as unknown
          return typeof value === "function" ? value.bind(target) : value
        }
      }
    },
  }) as SolidNativeTestRendererApi

  return root
}
