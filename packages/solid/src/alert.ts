import type { AppWindow } from "./root.js"
import { getSoloRenderer } from "./runtime.js"

export type AlertLevel = "info" | "warning" | "critical"

export interface ShowAlertOptions {
  /** The alert's headline, such as "Delete 3 files?". */
  message: string
  /** Secondary text under the message. */
  detail?: string
  /** Icon and emphasis. Defaults to "info". */
  level?: AlertLevel
  /**
   * Button labels, in order. The first is the default (Return); a label
   * reading "Cancel" answers Escape. Defaults to ["OK"].
   */
  buttons?: string[]
  /** The window the alert attaches to. Defaults to the main window. */
  window?: AppWindow
}

const LEVELS: readonly AlertLevel[] = ["info", "warning", "critical"]

function nonEmpty(value: unknown): value is string {
  return typeof value === "string" && value.trim().length > 0
}

/**
 * Show a native alert and resolve to the index of the clicked button. Only one
 * alert may be open at a time; showing another while one is open rejects.
 */
export async function showAlert(options: ShowAlertOptions): Promise<number> {
  if (!nonEmpty(options.message)) throw new TypeError("Alert message must be a non-empty string")
  if (options.detail !== undefined && !nonEmpty(options.detail)) {
    throw new TypeError("Alert detail must be a non-empty string")
  }
  if (options.level !== undefined && !LEVELS.includes(options.level)) {
    throw new TypeError("Alert level must be info, warning or critical")
  }
  if (options.buttons !== undefined) {
    if (!Array.isArray(options.buttons) || options.buttons.length === 0) {
      throw new TypeError("An alert needs at least one button")
    }
    if (!options.buttons.every(nonEmpty)) {
      throw new TypeError("Alert button labels must be non-empty strings")
    }
  }
  const renderer = getSoloRenderer()
  const show = renderer.showAlert?.bind(renderer)
  if (!show) throw new Error("This renderer does not support alerts")
  return show(JSON.stringify({
    message: options.message,
    ...(options.detail !== undefined ? { detail: options.detail } : {}),
    ...(options.level !== undefined ? { level: options.level } : {}),
    ...(options.buttons !== undefined ? { buttons: options.buttons } : {}),
  }), options.window?.id)
}
