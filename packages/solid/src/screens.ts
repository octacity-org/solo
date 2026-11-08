import * as native from "@solo/native"
import type { Screen } from "@solo/native"

export type { Screen, ScreenRect } from "@solo/native"

const changeListeners = new Set<() => void>()

/**
 * The connected screens (macOS). Each `id` is the display's UUID, stable
 * across reconnects and restarts: pass it as the `screen` window option, or
 * store it to reopen a window on the same screen. Rectangles are in points
 * from the screen's own top-left corner, so `bounds` starts at (0, 0) and
 * `visibleBounds` leaves out the menu bar and Dock. Throws off macOS.
 */
export function getScreens(): Screen[] {
  return native.getScreens()
}

/**
 * Called when a screen is connected or disconnected, or its size changes
 * (macOS). One change can call it more than once; read `getScreens()` for
 * the current list. Returns a function that stops listening.
 */
export function onScreensChanged(listener: () => void): () => void {
  if (typeof listener !== "function") throw new TypeError("onScreensChanged expects a function")
  changeListeners.add(listener)
  return () => {
    changeListeners.delete(listener)
  }
}

/** The renderer's element-0 `screensChanged` event (routed by root.ts). */
export function receiveScreensChanged(): void {
  for (const listener of [...changeListeners]) listener()
}
