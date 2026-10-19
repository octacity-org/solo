/// Solo — render Solid apps into a native Solo window.
///
/// Creates the napi SoloRenderer, opens the window, wires native events into
/// Solid's handler registry, and batches mutations.

import { SoloRenderer } from "@solo/native"
import type { EventPayload, WindowOptions } from "@solo/native"
import { clearEventHandlers, handleSoloEvent } from "@solo/core"
import {
  InProcessBackend,
  liveRendererAsTest,
  serveAutomationStdio,
} from "@solo/core/automation"
import type { NativeRenderer } from "@solo/core"
import type { LiveAutomationRenderer } from "@solo/core/automation"
import type { Element as SolidElement } from "solid-js"
import { mount, resetIdCounter } from "./runtime.js"

export interface Root {
  unmount: () => void
  /** Reopen a closed background-app window, or activate the existing one. */
  showWindow: () => void
  /** Close the current window without necessarily quitting a background app. */
  closeWindow: () => void
  /** Enter the platform's standard zoomed/maximized window state. */
  maximizeWindow: () => void
  /** Restore the window frame saved by the platform's zoom operation. */
  restoreWindow: () => void
  /** Toggle the standard zoomed/maximized window state. */
  toggleWindowMaximized: () => void
  /** Read the current native zoomed/maximized state. */
  isWindowMaximized: () => boolean
  /** Explicitly terminate the native application. */
  quitApplication: () => void
}

export interface RenderOptions extends WindowOptions {
  onEvent?: (event: EventPayload) => void
  /** Inject a renderer (tests). Defaults to a real SoloRenderer window. */
  renderer?: NativeRenderer
}

const RENDER_HOST_KEY = "__soloSolidRenderHost"

type RenderSlot = {
  renderer?: NativeRenderer
  root?: Root
}

function renderSlot(): RenderSlot {
  const existing = Reflect.get(globalThis, RENDER_HOST_KEY)
  if (existing) return existing
  const created: RenderSlot = {}
  Reflect.set(globalThis, RENDER_HOST_KEY, created)
  return created
}

/** Serve the automation protocol over stdio when a controller owns stdin. */
export function enableAutomation(renderer: LiveAutomationRenderer): void {
  serveAutomationStdio(new InProcessBackend(liveRendererAsTest(renderer)))
}

function createNativeRenderer(
  onEvent?: (event: EventPayload) => void
): SoloRenderer {
  const renderer = new SoloRenderer((err, event) => {
    if (err) {
      console.error("[Solo] Native event error:", err)
      return
    }
    if (event) {
      // macOS: AppKit asked to quit. Exit through the Worker so JS `exit`
      // handlers run before the process ends.
      if (event.elementId === 0 && event.eventType === "applicationQuitRequested") {
        process.exit(0)
      }
      handleSoloEvent(event)
      onEvent?.(event)
    }
  })
  // A pipe means a controller owns stdin. A TTY is a human keyboard.
  if (!process.stdin.isTTY) {
    const init = renderer.init.bind(renderer)
    renderer.init = (options) => {
      init(options)
      enableAutomation(renderer)
    }
  }
  return renderer
}

/** Mount the app in a Solo window. Later calls remount on the same window. */
export function render(code: () => SolidElement, options: RenderOptions = {}): Root {
  const slot = renderSlot()
  if (options.renderer) {
    // Injected renderer (tests): always take the fresh one.
    slot.renderer = options.renderer
  } else if (!slot.renderer) {
    const renderer = createNativeRenderer(options.onEvent)
    renderer.init(options)
    slot.renderer = renderer
      console.log("[solo] created native window")
  }
  const host = slot.renderer as NativeRenderer

  if (slot.root) {
    console.log("[solo] remount: unmount previous tree")
    slot.root.unmount()
  }
  resetIdCounter()
  clearEventHandlers()

  let disposeTree: () => void = mount(code, host)
  slot.root = {
    unmount: () => {
      disposeTree()
      disposeTree = () => {}
      slot.root = undefined
    },
    showWindow: () => {
      if (!host.showWindow) throw new Error("This renderer does not support showing windows")
      host.showWindow()
    },
    closeWindow: () => {
      if (!host.closeWindow) throw new Error("This renderer does not support closing windows")
      host.closeWindow()
    },
    maximizeWindow: () => {
      if (!host.setWindowMaximized) throw new Error("This renderer does not support maximizing windows")
      host.setWindowMaximized(true)
    },
    restoreWindow: () => {
      if (!host.setWindowMaximized) throw new Error("This renderer does not support restoring windows")
      host.setWindowMaximized(false)
    },
    toggleWindowMaximized: () => {
      if (!host.setWindowMaximized || !host.isWindowMaximized) {
        throw new Error("This renderer does not support toggling maximized windows")
      }
      host.setWindowMaximized(!host.isWindowMaximized())
    },
    isWindowMaximized: () => {
      if (!host.isWindowMaximized) throw new Error("This renderer does not expose maximized window state")
      return host.isWindowMaximized()
    },
    quitApplication: () => {
      if (!host.quitApplication) throw new Error("This renderer does not support quitting applications")
      host.quitApplication()
    },
  }

  console.log("[solo] mount complete")
  return slot.root
}

export { View, Text, Button } from "./components.js"
export type { ViewProps, TextProps, ButtonProps } from "./components.js"
export { resetIdCounter, flushMutations } from "./runtime.js"
