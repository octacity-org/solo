import { isAbsolute } from "node:path"
import * as native from "@solo/native"

/**
 * Open an absolute HTTP/HTTPS URL in the default browser (macOS).
 * Throws on invalid input, unsupported platforms, or OS rejection. Success
 * means macOS accepted the request, not that the page finished loading.
 */
export function openExternalUrl(url: string): void {
  if (typeof url !== "string") throw new TypeError("URL must be a string")
  native.openExternalUrl(url)
}

/** Replace the macOS clipboard with plain Unicode text; throws on failure. */
export function writeClipboardText(text: string): void {
  if (typeof text !== "string") throw new TypeError("Clipboard text must be a string")
  native.writeClipboardText(text)
}

/** The outcome of moving one path to the Trash. */
export interface TrashResult {
  path: string
  ok: boolean
  /** Why the move failed, when `ok` is false. */
  error?: string
}

/**
 * Move files and folders to the Trash (macOS), in order. Resolves with one
 * result per path: a path that cannot be moved reports why and does not stop
 * the others. Rejects for a path that is not absolute, or off macOS.
 */
export async function moveToTrash(paths: readonly string[]): Promise<TrashResult[]> {
  if (!Array.isArray(paths)) throw new TypeError("Paths must be an array of absolute paths")
  for (const path of paths) {
    if (typeof path !== "string" || !isAbsolute(path)) {
      throw new TypeError(`Expected an absolute path, got ${JSON.stringify(path)}`)
    }
  }
  if (paths.length === 0) return []
  return native.moveToTrash([...paths])
}
