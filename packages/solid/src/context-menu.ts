import { flush, getOwner, onCleanup } from "solid-js"
import { flushMutations, getSoloRenderer } from "./runtime.js"
import { requireCapability } from "./capability.js"

export type ContextMenuItem =
  | { readonly type: "separator" }
  | { readonly id: string; readonly label: string; readonly disabled?: boolean; readonly checked?: boolean }

/** A pointer event, or an element ID to anchor below that element. Coordinates
 * are window content coordinates in logical points, as supplied by Solo events. */
export type ContextMenuAnchor = { elementId: number; x?: number; y?: number }

export interface ContextMenu {
  /** Resolve with the selected ID, or null on dismissal, replacement or disposal. */
  show(anchor: ContextMenuAnchor): Promise<string | null>
  /** Dismiss any visible menu. The controller may be used again. */
  dismiss(): void
  /** Dismiss and release the controller. Automatic when its Solid owner disposes. */
  dispose(): void
}

let nextRequestId = 0

/** Create a native macOS menu. onSelect is never called after disposal.
 * Outside a Solid owner, call dispose() when the controller is no longer used. */
export function createContextMenu(
  items: readonly ContextMenuItem[],
  onSelect?: (id: string) => void,
): ContextMenu {
  if (!Array.isArray(items) || items.length === 0) throw new TypeError("Context menu must contain items")
  const actions = new Map<string, { disabled: boolean }>()
  const snapshot = items.map((item) => {
    if ("type" in item && item.type === "separator") return { type: "separator" as const }
    if (!("id" in item) || typeof item.id !== "string" || !item.id.trim() ||
      typeof item.label !== "string" || !item.label.trim()) {
      throw new TypeError("Context menu IDs and labels must be non-empty strings")
    }
    if (actions.has(item.id)) throw new TypeError(`Duplicate context menu ID: ${item.id}`)
    for (const flag of [item.disabled, item.checked]) {
      if (flag !== undefined && typeof flag !== "boolean") throw new TypeError("Context menu flags must be boolean")
    }
    actions.set(item.id, { disabled: item.disabled ?? false })
    return { ...item }
  })
  if (!actions.size) throw new TypeError("Context menu must contain an action")
  const renderer = getSoloRenderer()
  let disposed = false
  let cancelPending: (() => void) | undefined
  const controller: ContextMenu = {
    async show(anchor) {
      if (disposed) throw new Error("Context menu is disposed")
      if (!Number.isSafeInteger(anchor.elementId) || anchor.elementId <= 0) {
        throw new TypeError("Context menu elementId must be a positive integer")
      }
      if ((anchor.x === undefined) !== (anchor.y === undefined) ||
        (anchor.x !== undefined && (!Number.isFinite(anchor.x) || !Number.isFinite(anchor.y)))) {
        throw new TypeError("Context menu pointer coordinates must both be finite")
      }
      const showMenu = requireCapability(renderer, "showContextMenu", "support native context menus")
      const cancelMenu = requireCapability(renderer, "cancelContextMenu", "support native context menus")
      controller.dismiss()
      flushMutations()
      const requestId = ++nextRequestId
      return new Promise<string | null>((resolve, reject) => {
        let settled = false
        const cancel = () => {
          if (settled) return
          settled = true
          cancelPending = undefined
          try { cancelMenu(requestId) } finally { resolve(null) }
        }
        cancelPending = cancel
        const finish = (id: string | null) => {
          // Consume staged Solid disposal before accepting a queued native action.
          flush()
          if (settled) return
          settled = true
          if (cancelPending === cancel) cancelPending = undefined
          if (id !== null && (!actions.has(id) || actions.get(id)!.disabled)) {
            reject(new Error("Native context menu returned an unavailable action"))
            return
          }
          try {
            if (id !== null && !disposed) onSelect?.(id)
            resolve(id)
          } catch (error) { reject(error) }
        }
        try {
          showMenu(JSON.stringify({ requestId, elementId: anchor.elementId,
            ...(anchor.x === undefined ? {} : { x: anchor.x, y: anchor.y }), items: snapshot }))
            .then(finish, (error: unknown) => {
              if (settled) return
              settled = true
              if (cancelPending === cancel) cancelPending = undefined
              reject(error)
            })
        } catch (error) {
          settled = true
          cancelPending = undefined
          reject(error)
        }
      })
    },
    dismiss: () => cancelPending?.(),
    dispose: () => { disposed = true; controller.dismiss() },
  }
  if (getOwner()) onCleanup(controller.dispose)
  return controller
}
