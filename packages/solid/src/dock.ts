import { isAbsolute } from "node:path"
import { createRenderEffect, flush, getOwner, onCleanup, untrack } from "solid-js"
import * as native from "@solo/native"
import { toNativeMenuItems } from "./menu-bar.js"
import type { MenuBarItem } from "./menu-bar.js"

/** A Dock menu entry: the same shape as a menu-bar item. */
export type DockMenuItem = MenuBarItem

let active: Map<string, { enabled: boolean; run: () => void }> | undefined
const reopenListeners = new Set<() => void>()

/**
 * Reactively set the app-defined items of the Dock icon's menu (macOS), shown
 * above the items macOS adds itself. The owner removes them when disposed.
 */
export function configureDockMenu(source: () => readonly DockMenuItem[]): () => void {
  if (!getOwner()) throw new Error("Configure the Dock menu inside a Solid component or owner")
  if (typeof source !== "function") throw new TypeError("Dock menu configuration must be a function")
  if (active) throw new Error("The Dock menu is already configured")
  let registered = false
  let disposed = false

  createRenderEffect(source, (items) => {
    if (disposed) return
    const update = toNativeMenuItems(items, "Dock menu")
    native.setDockMenu(JSON.stringify(update.items))
    registered = true
    active = update.actions
  })

  const dispose = (): void => {
    if (disposed) return
    disposed = true
    if (!registered) return
    active = undefined
    native.setDockMenu("[]")
  }
  onCleanup(dispose)
  return dispose
}

/**
 * Called when the user clicks the app's Dock icon, or opens the app again,
 * while it shows no window (macOS). A background app (`menuBar` in `render`)
 * can reopen its window here with `root.showWindow()`. Returns a function
 * that stops listening.
 */
export function onApplicationReopen(listener: () => void): () => void {
  if (typeof listener !== "function") throw new TypeError("onApplicationReopen expects a function")
  reopenListeners.add(listener)
  return () => {
    reopenListeners.delete(listener)
  }
}

// App-level events reach these from the renderer callback (root.ts), not the
// element event registry, which render() clears.

/** The renderer's element-0 `dockMenuAction` event. */
export function receiveDockMenuAction(token: string | null | undefined): void {
  // Apply pending owner disposal before accepting a queued native event.
  flush()
  const action = active?.get(token ?? "")
  if (action?.enabled) untrack(action.run)
}

/** The renderer's element-0 `applicationReopen` event. */
export function receiveApplicationReopen(): void {
  for (const listener of [...reopenListeners]) listener()
}

/**
 * Add an existing file to the app's recent documents (macOS), listed in the
 * Dock menu. Choosing one sends the app its `file://` URL through
 * `onOpenUrls`.
 */
export function addRecentDocument(path: string): void {
  if (typeof path !== "string" || !isAbsolute(path)) {
    throw new TypeError(`Expected an absolute path, got ${JSON.stringify(path)}`)
  }
  native.addRecentDocument(path)
}
