/// Solo — render Solid apps into a native Solo window.
///
/// Creates the napi SoloRenderer, opens the window, wires native events into
/// Solid's handler registry, and batches mutations.

import { SoloRenderer } from "@solo/native"
import type { EventPayload, WindowOptions } from "@solo/native"
import {
  clearEventHandlers,
  handleSoloEvent,
  registerEventHandler,
  unregisterEventHandler,
} from "@solo/core"
import type { NativeRenderer } from "@solo/core"
import type { Element as SolidElement } from "solid-js"
import { automateWhenPiped } from "./automation-host.js"
import { mount, mountWindowRoot, resetIdCounter } from "./runtime.js"
import { requireCapability } from "./capability.js"
import { receiveApplicationReopen, receiveDockMenuAction } from "./dock.js"
import { receiveScreensChanged } from "./screens.js"
import { receiveOpenUrls } from "./open-urls.js"

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
  /** The window's content size in logical pixels. */
  getWindowSize: () => { width: number; height: number }
  /** Explicitly terminate the native application. */
  quitApplication: () => void
}

/** A window opened with openWindow. */
export interface AppWindow {
  /** The window's root element ID; stable while the window is open. */
  readonly id: number
  /** True once the window closed, by the user or through close(). */
  readonly closed: boolean
  /** Close the window and dispose its tree. A no-op once closed. */
  close: () => void
  /** Bring the window to the front. */
  focus: () => void
  setTitle: (title: string) => void
  maximize: () => void
  restore: () => void
  toggleMaximized: () => void
  isMaximized: () => boolean
  /** The window's content size in logical pixels. */
  size: () => { width: number; height: number }
}

export interface OpenWindowOptions extends Omit<WindowOptions, "menuBar"> {
  /** Called once after the window closes, however it closed. */
  onClose?: () => void
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
  /** Open windows from openWindow: their close functions, by root element ID. */
  windows?: Map<number, () => void>
}

function renderSlot(): RenderSlot {
  const existing = Reflect.get(globalThis, RENDER_HOST_KEY)
  if (existing) return existing
  const created: RenderSlot = {}
  Reflect.set(globalThis, RENDER_HOST_KEY, created)
  return created
}

function openWindows(): Map<number, () => void> {
  const slot = renderSlot()
  slot.windows ??= new Map()
  return slot.windows
}

/** Close every window from openWindow. IDs restart on remount, so none may survive it. */
function closeAllWindows(): void {
  for (const close of [...openWindows().values()]) close()
}

/**
 * Make `renderer` the host that openWindow() opens windows on, closing the
 * previous host's windows. For harnesses that mount without render(), such as
 * createSolidNativeTestRoot(). Not part of the public API.
 */
export function setWindowHost(renderer: NativeRenderer | undefined): void {
  closeAllWindows()
  renderSlot().renderer = renderer
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
      // App-level events go straight to their modules: render() clears the
      // element event registry, and URLs must be kept until a listener exists.
      if (event.elementId === 0) {
        if (event.eventType === "openUrls") receiveOpenUrls(event.urls)
        else if (event.eventType === "applicationReopen") receiveApplicationReopen()
        else if (event.eventType === "dockMenuAction") receiveDockMenuAction(event.value)
        else if (event.eventType === "screensChanged") receiveScreensChanged()
      }
      handleSoloEvent(event)
      onEvent?.(event)
    }
  })
  automateWhenPiped(renderer)
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
  }
  const host = slot.renderer as NativeRenderer

  if (slot.root) {
    slot.root.unmount()
  }
  closeAllWindows()
  resetIdCounter()
  clearEventHandlers()

  let disposeTree: () => void = mount(code, host)
  slot.root = {
    unmount: () => {
      closeAllWindows()
      disposeTree()
      disposeTree = () => {}
      slot.root = undefined
    },
    showWindow: () => requireCapability(host, "showWindow", "support showing windows")(),
    closeWindow: () => requireCapability(host, "closeWindow", "support closing windows")(),
    maximizeWindow: () =>
      requireCapability(host, "setWindowMaximized", "support maximizing windows")(true),
    restoreWindow: () =>
      requireCapability(host, "setWindowMaximized", "support restoring windows")(false),
    toggleWindowMaximized: () => {
      const toggling = "support toggling maximized windows"
      const isMaximized = requireCapability(host, "isWindowMaximized", toggling)
      requireCapability(host, "setWindowMaximized", toggling)(!isMaximized())
    },
    isWindowMaximized: () =>
      requireCapability(host, "isWindowMaximized", "expose maximized window state")(),
    getWindowSize: () => requireCapability(host, "getWindowSize", "expose the window size")(),
    quitApplication: () =>
      requireCapability(host, "quitApplication", "support quitting applications")(),
  }

  return slot.root
}

/**
 * Open another window that renders `code`, in the app started by render().
 * It shares the app's state and signals; closing it disposes only its tree.
 * macOS only.
 */
export function openWindow(code: () => SolidElement, options: OpenWindowOptions = {}): AppWindow {
  const host = renderSlot().renderer
  if (!host) throw new Error("Call render() before openWindow()")
  const open = requireCapability(host, "openWindow", "support multiple windows")
  const { onClose, ...windowOptions } = options
  const { rootId, dispose } = mountWindowRoot(code)
  try {
    open(rootId, windowOptions)
  } catch (error) {
    dispose()
    throw error
  }

  let closed = false
  const windows = openWindows()
  // Runs once, whichever way the window closes.
  const finish = (): void => {
    if (closed) return
    closed = true
    windows.delete(rootId)
    unregisterEventHandler(rootId, "windowClosed")
    dispose()
    onClose?.()
  }
  // Native closed the window (the user, or close() below); its root carries the event.
  registerEventHandler(rootId, "windowClosed", finish)
  const close = (): void => {
    if (closed) return
    // Closing first keeps the native window from painting a torn-down tree.
    try {
      host.closeWindow?.(rootId)
    } catch {
      // Already closed natively; its windowClosed event is still on its way.
    }
    finish()
  }
  windows.set(rootId, close)
  const assertOpen = (action: string): void => {
    if (closed) throw new Error(`Cannot ${action} a closed window`)
  }
  const maximized = (): boolean => {
    assertOpen("read")
    return requireCapability(host, "isWindowMaximized", "expose maximized window state")(rootId)
  }
  const setMaximized = (value: boolean): void => {
    assertOpen(value ? "maximize" : "restore")
    requireCapability(host, "setWindowMaximized", "support maximizing windows")(value, rootId)
  }

  return {
    id: rootId,
    get closed() {
      return closed
    },
    close,
    focus: () => {
      assertOpen("focus")
      host.showWindow?.(rootId)
    },
    setTitle: (title) => {
      assertOpen("retitle")
      host.setWindowTitle?.(title, rootId)
    },
    maximize: () => setMaximized(true),
    restore: () => setMaximized(false),
    toggleMaximized: () => setMaximized(!maximized()),
    isMaximized: maximized,
    size: () => {
      assertOpen("measure")
      return requireCapability(host, "getWindowSize", "expose the window size")(rootId)
    },
  }
}
