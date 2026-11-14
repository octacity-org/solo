import * as native from "@solo/native"
import type { KeyboardLayout } from "@solo/native"
import { createSignal, flush, getOwner, onCleanup } from "solid-js"
import type { Accessor } from "solid-js"

export type { KeyboardLayout } from "@solo/native"

/** How hard the system is working to keep the machine cool. */
export type ThermalState = "nominal" | "fair" | "serious" | "critical"

const thermalListeners = new Set<(state: ThermalState) => void>()
const layoutListeners = new Set<(layout: KeyboardLayout) => void>()

function isThermalState(value: unknown): value is ThermalState {
  return value === "nominal" || value === "fair" || value === "serious" || value === "critical"
}

/** Hide the application, as its Hide menu command does (macOS). */
export function hideApplication(): void {
  native.hideApplication()
}

/** Hide every other application, as Hide Others does (macOS). */
export function hideOtherApplications(): void {
  native.hideOtherApplications()
}

/** Show the applications hidden by Hide Others, as Show All does (macOS). */
export function showAllApplications(): void {
  native.showAllApplications()
}

/**
 * Play the system alert sound, for invalid input or a failed action. The user
 * chooses the sound, or silences it, in System Settings. On Linux it rings
 * the active window's bell, so it throws while no window is open.
 */
export function beep(): void {
  native.beep()
}

/**
 * Whether the system shows scroll bars only while scrolling, the user's
 * System Settings → Appearance → "Show scroll bars" choice (macOS). There is
 * no change notification; read it again when it matters.
 */
export function shouldAutoHideScrollbars(): boolean {
  return native.shouldAutoHideScrollbars()
}

/**
 * Quit, then open the application again once this process has exited
 * (macOS). The app quits the same way as with Cmd+Q. Works only in a
 * packaged `.app`; anywhere else it throws and the app keeps running.
 */
export function restartApplication(): void {
  native.restartApplication()
}

/**
 * Read and follow the thermal state inside a Solid owner (macOS). The
 * accessor stops updating when its owner is disposed.
 */
export function createThermalState(): Accessor<ThermalState> {
  if (!getOwner()) throw new Error("Read the thermal state inside a Solid component or owner")
  const initial = native.getThermalState()
  if (!isThermalState(initial)) throw new Error(`Native renderer returned an invalid thermal state: ${initial}`)
  const [state, setState] = createSignal<ThermalState>(initial)
  thermalListeners.add(setState)
  onCleanup(() => thermalListeners.delete(setState))
  return state
}

/**
 * Read and follow the selected keyboard layout (input source) inside a Solid
 * owner (macOS). The accessor stops updating when its owner is disposed.
 */
export function createKeyboardLayout(): Accessor<KeyboardLayout> {
  if (!getOwner()) throw new Error("Read the keyboard layout inside a Solid component or owner")
  const [layout, setLayout] = createSignal<KeyboardLayout>(native.getKeyboardLayout(), {
    equals: (a, b) => a.id === b.id && a.name === b.name,
  })
  layoutListeners.add(setLayout)
  onCleanup(() => layoutListeners.delete(setLayout))
  return layout
}

/** The renderer's element-0 `thermalStateChange` event (routed by root.ts). */
export function receiveThermalStateChange(value: string | undefined | null): void {
  if (!isThermalState(value)) return
  // Apply pending owner disposal before delivering a queued native event.
  flush()
  for (const listener of [...thermalListeners]) listener(value)
}

/** The renderer's element-0 `keyboardLayoutChange` event (routed by root.ts). */
export function receiveKeyboardLayoutChange(value: string | undefined | null): void {
  let layout: unknown
  try { layout = JSON.parse(value ?? "") } catch { return }
  if (!layout || typeof layout !== "object") return
  const { id, name } = layout as { id?: unknown; name?: unknown }
  if (typeof id !== "string" || typeof name !== "string") return
  flush()
  for (const listener of [...layoutListeners]) listener({ id, name })
}
