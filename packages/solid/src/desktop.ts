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

function requireAbsolutePath(path: unknown): asserts path is string {
  if (typeof path !== "string" || !isAbsolute(path)) {
    throw new TypeError(`Expected an absolute path, got ${JSON.stringify(path)}`)
  }
}

/**
 * Show a file or folder selected in a Finder window (macOS). Throws for a
 * path that is not absolute or does not exist, off macOS, or on OS rejection.
 */
export function revealPath(path: string): void {
  requireAbsolutePath(path)
  native.revealPath(path)
}

/**
 * Open a file or folder with its default application, as a double-click in
 * Finder would (macOS). Throws like `revealPath`.
 */
export function openPath(path: string): void {
  requireAbsolutePath(path)
  native.openPath(path)
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
  for (const path of paths) requireAbsolutePath(path)
  if (paths.length === 0) return []
  return native.moveToTrash([...paths])
}
