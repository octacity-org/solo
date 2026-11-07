import { createSignal, flush, getOwner, onCleanup } from "solid-js"
import type { Accessor } from "solid-js"
import { registerEventHandler, unregisterEventHandler } from "@solo/core"
import type { NativeRenderer } from "@solo/core"
import { getSoloRenderer } from "./runtime.js"
import { requireCapability } from "./capability.js"

export type SystemAppearance = "light" | "dark"

interface Subscription {
  token: string
  appearance: SystemAppearance
  listeners: Set<(appearance: SystemAppearance) => void>
  replace: (token: string | null) => string
}

const byRenderer = new WeakMap<NativeRenderer, Subscription>()
const byToken = new Map<string, Subscription>()
let nextToken = 0

function isAppearance(value: unknown): value is SystemAppearance {
  return value === "light" || value === "dark"
}

/**
 * Read and follow macOS light/dark appearance inside a mounted Solid owner.
 * The observer is shared and removed after its last owner disposes. This only
 * reports system state; it never changes the application's theme preference.
 */
export function createSystemAppearance(): Accessor<SystemAppearance> {
  if (!getOwner()) throw new Error("Read system appearance inside a Solid component or owner")
  const renderer = getSoloRenderer()
  let subscription = byRenderer.get(renderer)
  if (!subscription) {
    const replace = requireCapability(renderer, "setSystemAppearanceSubscription", "support system appearance")
    const token = String(++nextToken)
    const initial = replace(token)
    if (!isAppearance(initial)) {
      replace(null)
      throw new Error("Native renderer returned an invalid system appearance")
    }
    subscription = { token, appearance: initial, listeners: new Set(), replace }
    byRenderer.set(renderer, subscription)
    byToken.set(token, subscription)
    registerEventHandler(0, "systemAppearanceChange", (event) => {
      // Apply pending owner disposal before accepting a queued native event.
      flush()
      let update: unknown
      try { update = JSON.parse(event.value ?? "") } catch { return }
      if (!update || typeof update !== "object") return
      const { token, appearance } = update as { token?: unknown; appearance?: unknown }
      if (typeof token !== "string" || !isAppearance(appearance)) return
      const target = byToken.get(token)
      if (!target || target.appearance === appearance) return
      target.appearance = appearance
      for (const listener of target.listeners) listener(appearance)
    })
  }
  const shared = subscription
  const [appearance, setAppearance] = createSignal<SystemAppearance>(shared.appearance)
  shared.listeners.add(setAppearance)
  onCleanup(() => {
    shared.listeners.delete(setAppearance)
    if (shared.listeners.size) return
    byRenderer.delete(renderer)
    byToken.delete(shared.token)
    if (!byToken.size) unregisterEventHandler(0, "systemAppearanceChange")
    shared.replace(null)
  })
  return appearance
}
