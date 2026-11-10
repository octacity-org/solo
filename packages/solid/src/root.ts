/// Solo — render Solid apps into a native Solo window.
///
/// Creates the napi SoloRenderer, opens the window, wires native events into
/// Solid's handler registry, and batches mutations.

import { isAbsolute } from "node:path"
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
import { receiveKeyboardLayoutChange, receiveThermalStateChange } from "./application.js"
import { receiveOpenUrls } from "./open-urls.js"

export interface Root {
  unmount: () => void
  /** Reopen a closed background-app window, or activate the existing one. */
  showWindow: () => void
  /** Close the current window without necessarily quitting a background app. */
  closeWindow: () => void
  /**
   * Ask to close the main window as its close button does: with
   * `onCloseRequest`, the app decides; otherwise it closes.
   */
  requestCloseWindow: () => void
  /** Enter the platform's standard zoomed/maximized window state. */
  maximizeWindow: () => void
  /** Restore the window frame saved by the platform's zoom operation. */
  restoreWindow: () => void
  /** Toggle the standard zoomed/maximized window state. */
  toggleWindowMaximized: () => void
  /** Read the current native zoomed/maximized state. */
  isWindowMaximized: () => boolean
  /** Enter native fullscreen. The transition is animated. */
  enterFullscreen: () => void
  exitFullscreen: () => void
  toggleFullscreen: () => void
  isFullscreen: () => boolean
  /** Minimize the window into the Dock. */
  minimizeWindow: () => void
  setWindowBackground: (background: WindowBackground) => void
  /** The window's content size in logical pixels. */
  getWindowSize: () => { width: number; height: number }
  /** Resize the content to a size in logical pixels, as getWindowSize reports it. */
  resizeWindow: (size: { width: number; height: number }) => void
  /**
   * Let the window merge into tabs with windows that share `identifier`;
   * undefined clears it. macOS uses AppKit's window tabs, Linux a tab strip
   * Solo draws in the window.
   */
  setTabbingIdentifier: (identifier: string | undefined) => void
  /** Show or hide the native tab overview (Show All Tabs). */
  toggleTabOverview: () => void
  /**
   * Mark the window's document as having unsaved changes, shown as a dot in
   * the close button on macOS and as a `*` title prefix elsewhere.
   */
  setDocumentEdited: (edited: boolean) => void
  /**
   * Show the file the window edits in the title bar, with its icon and path
   * menu. The path must be absolute; null clears it. Only macOS draws it.
   */
  setDocumentPath: (path: string | null) => void
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
  /**
   * Ask to close the window as its close button does: with
   * `onCloseRequest`, the app decides; otherwise it closes.
   */
  requestClose: () => void
  /** Bring the window to the front. */
  focus: () => void
  setTitle: (title: string) => void
  maximize: () => void
  restore: () => void
  toggleMaximized: () => void
  isMaximized: () => boolean
  /** Enter native fullscreen. The transition is animated. */
  enterFullscreen: () => void
  exitFullscreen: () => void
  toggleFullscreen: () => void
  isFullscreen: () => boolean
  /** Minimize the window into the Dock. */
  minimize: () => void
  setBackground: (background: WindowBackground) => void
  /** The window's content size in logical pixels. */
  size: () => { width: number; height: number }
  /** Resize the content to a size in logical pixels, as size() reports it. */
  resize: (size: { width: number; height: number }) => void
  /**
   * Let the window merge into tabs with windows that share `identifier`;
   * undefined clears it. macOS uses AppKit's window tabs, Linux a tab strip
   * Solo draws in the window.
   */
  setTabbingIdentifier: (identifier: string | undefined) => void
  /** Show or hide the native tab overview (Show All Tabs). */
  toggleTabOverview: () => void
  /**
   * Mark the window's document as having unsaved changes, shown as a dot in
   * the close button on macOS and as a `*` title prefix elsewhere.
   */
  setDocumentEdited: (edited: boolean) => void
  /**
   * Show the file the window edits in the title bar, with its icon and path
   * menu. The path must be absolute; null clears it. Only macOS draws it.
   */
  setDocumentPath: (path: string | null) => void
}

/** A window's background, as the `windowBackground` option takes it. */
export type WindowBackground = "opaque" | "transparent" | "blurred"

/**
 * Decides whether a window closes. The window's close button and
 * `requestClose()` call it instead of closing; close the window yourself, or
 * return to keep it open. While a returned promise is pending, further
 * requests are ignored.
 */
export type CloseRequestHandler = () => void | Promise<void>

export interface OpenWindowOptions extends Omit<WindowOptions, "menuBar"> {
  /**
   * Application identifier the desktop groups the window under, such as
   * `dev.solo.tasks` (Linux). It becomes X11 `WM_CLASS` and the Wayland
   * `app_id`, and should match the desktop entry's `StartupWMClass`.
   */
  appId?: string
  /** Called once after the window closes, however it closed. */
  onClose?: () => void
  /** Confirm or cancel closing; close with `close()`. */
  onCloseRequest?: CloseRequestHandler
}

export interface RenderOptions extends WindowOptions {
  /**
   * Application identifier the desktop groups the window under, such as
   * `dev.solo.tasks` (Linux). It becomes X11 `WM_CLASS` and the Wayland
   * `app_id`, and should match the desktop entry's `StartupWMClass`.
   */
  appId?: string
  onEvent?: (event: EventPayload) => void
  /** Confirm or cancel closing the main window; close with `closeWindow()`. */
  onCloseRequest?: CloseRequestHandler
  /** Inject a renderer (tests). Defaults to a real SoloRenderer window. */
  renderer?: NativeRenderer
}

const RENDER_HOST_KEY = "__soloSolidRenderHost"

type RenderSlot = {
  renderer?: NativeRenderer
  root?: Root
  /** The main window has a native close guard. */
  closeGuarded?: boolean
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

/** The native event a guarded window's close request arrives as. */
const CLOSE_REQUESTED = "windowCloseRequested"

/**
 * Call `handler` per close request, ignoring requests while its promise is
 * pending. `settled` runs once the handler has returned or its promise has
 * settled.
 */
function closeRequestListener(handler: CloseRequestHandler, settled?: () => void): () => void {
  let pending = false
  return () => {
    if (pending) return
    const result = handler()
    if (result instanceof Promise) {
      pending = true
      // A rejection still surfaces as unhandled.
      void result.finally(() => {
        pending = false
        settled?.()
      })
    } else {
      settled?.()
    }
  }
}

/** Main-window close requests carry no element; native sends them on element 0. */
function setMainCloseGuard(slot: RenderSlot, host: NativeRenderer, handler?: CloseRequestHandler): void {
  if (handler) {
    requireCapability(host, "setWindowCloseGuard", "support close requests")(true)
    registerEventHandler(0, CLOSE_REQUESTED, closeRequestListener(handler))
  } else if (slot.closeGuarded) {
    host.setWindowCloseGuard?.(false)
  }
  slot.closeGuarded = Boolean(handler)
}

/** Window-state calls shared by the render root and AppWindow. */
function windowState(host: NativeRenderer, windowId?: number) {
  const fullscreen = (): boolean =>
    requireCapability(host, "isWindowFullscreen", "expose fullscreen window state")(windowId)
  const setFullscreen = (value: boolean): void =>
    requireCapability(host, "setWindowFullscreen", "support fullscreen windows")(value, windowId)
  return {
    enterFullscreen: () => setFullscreen(true),
    exitFullscreen: () => setFullscreen(false),
    toggleFullscreen: () => setFullscreen(!fullscreen()),
    isFullscreen: fullscreen,
    minimize: () => requireCapability(host, "minimizeWindow", "support minimizing windows")(windowId),
    setBackground: (background: WindowBackground) =>
      requireCapability(host, "setWindowBackground", "support window backgrounds")(background, windowId),
    resize: ({ width, height }: { width: number; height: number }) =>
      requireCapability(host, "resizeWindow", "support resizing windows")(width, height, windowId),
    setTabbingIdentifier: (identifier: string | undefined) =>
      requireCapability(host, "setWindowTabbingIdentifier", "support window tabs")(
        identifier ?? null,
        windowId,
      ),
    toggleTabOverview: () =>
      requireCapability(host, "toggleWindowTabOverview", "support window tabs")(windowId),
    setDocumentEdited: (edited: boolean) => {
      if (typeof edited !== "boolean") throw new TypeError(`Expected a boolean, got ${JSON.stringify(edited)}`)
      requireCapability(host, "setWindowDocumentEdited", "support document windows")(edited, windowId)
    },
    setDocumentPath: (path: string | null) => {
      if (path !== null && (typeof path !== "string" || !isAbsolute(path))) {
        throw new TypeError(`Expected an absolute path or null, got ${JSON.stringify(path)}`)
      }
      requireCapability(host, "setWindowDocumentPath", "support document windows")(path, windowId)
    },
  }
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
      // The app is quitting (AppKit on macOS, the GPUI loop ending on Linux).
      // Exit from JS so its `exit` handlers run before the process ends.
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
        else if (event.eventType === "thermalStateChange") receiveThermalStateChange(event.value)
        else if (event.eventType === "keyboardLayoutChange") receiveKeyboardLayoutChange(event.value)
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
    if (slot.renderer !== options.renderer) slot.closeGuarded = false
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
  setMainCloseGuard(slot, host, options.onCloseRequest)
  const state = windowState(host)
  slot.root = {
    unmount: () => {
      closeAllWindows()
      setMainCloseGuard(slot, host)
      unregisterEventHandler(0, CLOSE_REQUESTED)
      disposeTree()
      disposeTree = () => {}
      slot.root = undefined
    },
    showWindow: () => requireCapability(host, "showWindow", "support showing windows")(),
    closeWindow: () => requireCapability(host, "closeWindow", "support closing windows")(),
    requestCloseWindow: () =>
      requireCapability(host, "requestWindowClose", "support close requests")(),
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
    enterFullscreen: state.enterFullscreen,
    exitFullscreen: state.exitFullscreen,
    toggleFullscreen: state.toggleFullscreen,
    isFullscreen: state.isFullscreen,
    minimizeWindow: state.minimize,
    setWindowBackground: state.setBackground,
    getWindowSize: () => requireCapability(host, "getWindowSize", "expose the window size")(),
    resizeWindow: state.resize,
    setTabbingIdentifier: state.setTabbingIdentifier,
    toggleTabOverview: state.toggleTabOverview,
    setDocumentEdited: state.setDocumentEdited,
    setDocumentPath: state.setDocumentPath,
    quitApplication: () =>
      requireCapability(host, "quitApplication", "support quitting applications")(),
  }

  return slot.root
}

/**
 * Open another window that renders `code`, in the app started by render().
 * It shares the app's state and signals; closing it disposes only its tree.
 */
export function openWindow(code: () => SolidElement, options: OpenWindowOptions = {}): AppWindow {
  const host = renderSlot().renderer
  if (!host) throw new Error("Call render() before openWindow()")
  const open = requireCapability(host, "openWindow", "support multiple windows")
  const { onClose, onCloseRequest, ...windowOptions } = options
  const guard = onCloseRequest
    ? requireCapability(host, "setWindowCloseGuard", "support close requests")
    : undefined
  const { rootId, dispose } = mountWindowRoot(code)
  try {
    open(rootId, windowOptions)
  } catch (error) {
    dispose()
    throw error
  }
  try {
    guard?.(true, rootId)
  } catch (error) {
    host.closeWindow?.(rootId)
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
    unregisterEventHandler(rootId, CLOSE_REQUESTED)
    dispose()
    onClose?.()
  }
  // Native closed the window (the user, or close() below); its root carries the event.
  registerEventHandler(rootId, "windowClosed", finish)
  // A handler that finishes with the window still open declined: native drops
  // the whole-window close a close button may have left waiting on it, so a
  // later close() of this tab closes only the tab.
  const declined = (): void => {
    if (closed) return
    try {
      host.declineWindowClose?.(rootId)
    } catch {
      // Already closed natively; its windowClosed event is still on its way.
    }
  }
  if (onCloseRequest) {
    registerEventHandler(rootId, CLOSE_REQUESTED, closeRequestListener(onCloseRequest, declined))
  }
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
  const state = windowState(host, rootId)
  /** `call`, once the window is known to be open. */
  const whileOpen = <A extends unknown[], R>(action: string, call: (...args: A) => R) =>
    (...args: A): R => {
      assertOpen(action)
      return call(...args)
    }

  return {
    id: rootId,
    get closed() {
      return closed
    },
    close,
    requestClose: () => {
      assertOpen("close")
      requireCapability(host, "requestWindowClose", "support close requests")(rootId)
    },
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
    enterFullscreen: whileOpen("change", state.enterFullscreen),
    exitFullscreen: whileOpen("change", state.exitFullscreen),
    toggleFullscreen: whileOpen("change", state.toggleFullscreen),
    isFullscreen: whileOpen("read", state.isFullscreen),
    minimize: whileOpen("minimize", state.minimize),
    setBackground: whileOpen("change", state.setBackground),
    size: () => {
      assertOpen("measure")
      return requireCapability(host, "getWindowSize", "expose the window size")(rootId)
    },
    resize: whileOpen("resize", state.resize),
    setTabbingIdentifier: whileOpen("change", state.setTabbingIdentifier),
    toggleTabOverview: whileOpen("show the tabs of", state.toggleTabOverview),
    setDocumentEdited: whileOpen("change", state.setDocumentEdited),
    setDocumentPath: whileOpen("change", state.setDocumentPath),
  }
}
