import { createRenderEffect, flush, getOwner, onCleanup, untrack } from "solid-js"
import { registerEventHandler, unregisterEventHandler } from "@solo/core"
import type { NativeRenderer } from "@solo/core"
import { getSoloRenderer } from "./runtime.js"
import { requireCapability } from "./capability.js"

/** An application command owned by the Solid component that registers it. */
export interface ApplicationCommand {
  /** Unique among currently registered commands. Reusable after disposal. */
  id: string
  label: string
  /** One chord, e.g. "s", "space", or "cmd-option-p". Not a system-wide hotkey. */
  shortcut?: string
  /** Limit keyboard dispatch to this element and its focused descendants. */
  scopeElementId?: number
  /** Allow held-key repeats. Defaults to false; menu clicks are unaffected. */
  allowRepeat?: boolean
  /** Optional top-level native menu containing this command. */
  menu?: string
  /** Only enabled is reactive; other options are fixed for this registration. */
  enabled?: boolean | (() => boolean)
  run: () => void
}

interface Entry {
  descriptor: {
    id: string
    label: string
    shortcut?: string
    scopeElementId?: number
    allowRepeat?: boolean
    menu?: string
    enabled: boolean
  }
  run: () => void
  enabled: () => boolean
}

const registries = new WeakMap<NativeRenderer, Map<string, Entry>>()
let nextToken = 0
let activeRegistry: Map<string, Entry> | undefined

/**
 * Register inside a Solid component/owner after render() installs its renderer.
 * The returned disposer removes the command early; owner cleanup also removes
 * it. Native registration errors leave existing commands unchanged.
 */
export function registerApplicationCommand(command: ApplicationCommand): () => void {
  if (!getOwner()) throw new Error("Register application commands inside a Solid component or owner")
  for (const key of ["id", "label"] as const) {
    if (typeof command[key] !== "string" || !command[key].trim()) {
      throw new TypeError(`Application command ${key} must be a non-empty string`)
    }
  }
  for (const key of ["menu", "shortcut"] as const) {
    if (command[key] !== undefined && (typeof command[key] !== "string" || !command[key].trim())) {
      throw new TypeError(`Application command ${key} must be a non-empty string`)
    }
  }
  if (typeof command.run !== "function") throw new TypeError("Application command run must be a function")
  if (command.scopeElementId !== undefined && (!Number.isSafeInteger(command.scopeElementId) || command.scopeElementId < 0)) {
    throw new TypeError("Application command scopeElementId must be a non-negative safe integer")
  }
  if (command.allowRepeat !== undefined && typeof command.allowRepeat !== "boolean") {
    throw new TypeError("Application command allowRepeat must be boolean")
  }
  const enabledOption = command.enabled
  const enabled = (): boolean => {
    const value = typeof enabledOption === "function" ? enabledOption() : enabledOption ?? true
    if (typeof value !== "boolean") throw new TypeError("Application command enabled must be boolean")
    return value
  }
  const renderer = getSoloRenderer()
  const replace = requireCapability(renderer, "setApplicationCommands", "support application commands")
  let registry = registries.get(renderer)
  if (!registry) {
    registry = new Map()
    registries.set(renderer, registry)
  }
  const entries = registry
  const id = command.id
  if (entries.has(id)) throw new Error(`Application command ${id} is already registered`)
  const entry: Entry = {
    descriptor: {
      id: String(++nextToken), label: command.label, shortcut: command.shortcut,
      menu: command.menu, enabled: untrack(enabled),
      scopeElementId: command.scopeElementId, allowRepeat: command.allowRepeat,
    },
    run: command.run,
    enabled,
  }
  const publish = (): void => replace(JSON.stringify([...entries.values()].map((e) => e.descriptor)))
  entries.set(id, entry)
  try {
    publish()
  } catch (error) {
    entries.delete(id)
    throw error
  }
  activeRegistry = entries
  registerEventHandler(0, "applicationCommand", (event) => {
    // Solid 2 stages writes until its scheduled flush; apply pending enabled
    // changes/owner disposal before accepting a queued native action.
    flush()
    const target = [...entries.values()].find((e) => e.descriptor.id === event.value)
    // Re-check enabled to reject events queued before a signal write or disposal.
    if (target && untrack(target.enabled)) target.run()
  })

  let disposed = false
  const dispose = (): void => {
    if (disposed) return
    disposed = true
    entries.delete(id)
    if (!entries.size && activeRegistry === entries) {
      unregisterEventHandler(0, "applicationCommand")
      activeRegistry = undefined
    }
    publish()
  }
  onCleanup(dispose)
  if (typeof enabledOption === "function") {
    createRenderEffect(enabled, (value) => {
      if (disposed || entry.descriptor.enabled === value) return
      const previous = entry.descriptor.enabled
      entry.descriptor.enabled = value
      try {
        publish()
      } catch (error) {
        entry.descriptor.enabled = previous
        throw error
      }
    })
  }
  return dispose
}
