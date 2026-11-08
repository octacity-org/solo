import { createRenderEffect, flush, getOwner, onCleanup, untrack } from "solid-js"
import { registerEventHandler, unregisterEventHandler } from "@solo/core"
import type { NativeRenderer } from "@solo/core"
import { getSoloRenderer } from "./runtime.js"
import { requireCapability } from "./capability.js"

export type MenuBarItem =
  | { readonly type: "separator" }
  | { readonly type: "status"; readonly label: string }
  | {
      readonly id: string
      readonly label: string
      readonly enabled?: boolean
      readonly checked?: boolean
      readonly run: () => void
    }

export interface MenuBarConfiguration {
  /** Absolute template-image path. Omit to retain the icon supplied to render(). */
  readonly iconPath?: string
  /** Set null to remove the tooltip; omit to retain the initial tooltip. */
  readonly tooltip?: string | null
  /** Custom entries shown before Solo's built-in Open and Quit items. */
  readonly items?: readonly MenuBarItem[]
}

interface ActiveConfiguration {
  actions: Map<string, MenuAction>
}

const byRenderer = new WeakMap<NativeRenderer, ActiveConfiguration>()
let activeConfiguration: ActiveConfiguration | undefined
let nextToken = 0

function validateText(value: unknown, name: string, noun = "Menu-bar"): string {
  if (typeof value !== "string" || !value.trim()) {
    throw new TypeError(`${noun} ${name} must be a non-empty string`)
  }
  return value
}

type MenuAction = { enabled: boolean; run: () => void }

/**
 * Validate menu items and give each action a fresh token, which the native
 * menu sends back when it is chosen. Shared with the Dock menu; `noun`
 * names the menu in errors.
 */
export function toNativeMenuItems(
  items: readonly MenuBarItem[] | undefined,
  noun: string,
): { items: Array<Record<string, unknown>>; actions: Map<string, MenuAction> } {
  if (items !== undefined && !Array.isArray(items)) {
    throw new TypeError(`${noun} items must be an array`)
  }
  const native: Array<Record<string, unknown>> = []
  const ids = new Set<string>()
  const actions = new Map<string, MenuAction>()
  for (const item of items ?? []) {
    if (!item || typeof item !== "object") throw new TypeError(`${noun} items must be objects`)
    if ("type" in item && item.type === "separator") {
      native.push({ type: "separator" })
      continue
    }
    if ("type" in item && item.type === "status") {
      native.push({ type: "status", label: validateText(item.label, "status label", noun) })
      continue
    }
    if (!("id" in item) || !("label" in item) || !("run" in item)) {
      throw new TypeError(`${noun} actions require id, label, and run`)
    }
    const id = validateText(item.id, "action ID", noun)
    const label = validateText(item.label, "action label", noun)
    if (ids.has(id)) throw new TypeError(`Duplicate ${noun.toLowerCase()} action ID: ${id}`)
    ids.add(id)
    if (typeof item.run !== "function") throw new TypeError(`${noun} action run must be a function`)
    if (item.enabled !== undefined && typeof item.enabled !== "boolean") {
      throw new TypeError(`${noun} action enabled must be boolean`)
    }
    if (item.checked !== undefined && typeof item.checked !== "boolean") {
      throw new TypeError(`${noun} action checked must be boolean`)
    }
    const enabled = item.enabled ?? true
    const token = String(++nextToken)
    actions.set(token, { enabled, run: item.run })
    native.push({ type: "action", token, label, enabled, checked: item.checked ?? false })
  }
  return { items: native, actions }
}

/**
 * Reactively configure the application's existing macOS status item.
 * The owner restores Solo's default Open/Quit menu when disposed.
 */
export function configureMenuBar(source: () => MenuBarConfiguration): () => void {
  if (!getOwner()) throw new Error("Configure the menu bar inside a Solid component or owner")
  if (typeof source !== "function") throw new TypeError("Menu-bar configuration must be a function")
  const renderer = getSoloRenderer()
  const replace = requireCapability(renderer, "setMenuBar", "support menu-bar configuration")
  if (byRenderer.has(renderer)) throw new Error("This renderer already has a menu-bar configuration")

  const configuration: ActiveConfiguration = { actions: new Map() }
  let registered = false
  let disposed = false

  createRenderEffect(source, (value) => {
    if (disposed || !value || typeof value !== "object") {
      if (!disposed) throw new TypeError("Menu-bar configuration must be an object")
      return
    }
    const native: {
      iconPath?: string
      tooltip?: string | null
      items: Array<Record<string, unknown>>
    } = { items: [] }
    if (value.iconPath !== undefined) native.iconPath = validateText(value.iconPath, "iconPath")
    if (value.tooltip !== undefined) {
      native.tooltip = value.tooltip === null ? null : validateText(value.tooltip, "tooltip")
    }
    const { items, actions } = toNativeMenuItems(value.items, "Menu-bar")
    native.items = items

    replace(JSON.stringify(native))
    configuration.actions = actions
    if (!registered) {
      registered = true
      byRenderer.set(renderer, configuration)
      activeConfiguration = configuration
      registerEventHandler(0, "menuBarAction", (event) => {
        flush()
        const action = activeConfiguration?.actions.get(event.value ?? "")
        if (action?.enabled) untrack(action.run)
      })
    }
  })

  const dispose = (): void => {
    if (disposed) return
    disposed = true
    configuration.actions.clear()
    if (!registered) return
    registered = false
    byRenderer.delete(renderer)
    if (activeConfiguration === configuration) {
      activeConfiguration = undefined
      unregisterEventHandler(0, "menuBarAction")
    }
    replace(null)
  }
  onCleanup(dispose)
  return dispose
}
