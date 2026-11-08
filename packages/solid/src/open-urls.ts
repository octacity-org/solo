import * as native from "@solo/native"

/** Receives the URLs of one open request, in the order macOS sent them. */
export type OpenUrlsListener = (urls: string[]) => void

const listeners = new Set<OpenUrlsListener>()
// URLs that arrived before the first listener, such as the one that launched
// the app. A `solo dev` reload starts a new Worker, so this module, its
// listeners and its buffer start empty there.
let pending: string[] = []

/** The renderer's element-0 `openUrls` event (root.ts). */
export function receiveOpenUrls(urls: readonly string[] | null | undefined): void {
  if (!urls?.length) return
  if (!listeners.size) {
    pending.push(...urls)
    return
  }
  for (const listener of [...listeners]) listener([...urls])
}

/**
 * Receive the URLs macOS asks the app to open: links in a custom scheme the
 * app declares (`myapp://…`), and files opened with the app or chosen from
 * its recent documents, as `file://` URLs. Returns a function that stops
 * listening.
 *
 * URLs that arrived before the first listener, including the one that
 * launched the app, are passed to that listener immediately, before
 * `onOpenUrls` returns.
 *
 * Any app or web page can send these URLs: treat them as untrusted input.
 */
export function onOpenUrls(listener: OpenUrlsListener): () => void {
  if (typeof listener !== "function") throw new TypeError("onOpenUrls expects a function")
  listeners.add(listener)
  if (pending.length) {
    const urls = pending
    pending = []
    listener(urls)
  }
  return () => {
    listeners.delete(listener)
  }
}

/**
 * Make this app the default handler for `scheme`, given without `://`
 * (macOS 12+). The app must run from an installed bundle with an identifier,
 * and should also declare the scheme under `CFBundleURLTypes` in its
 * `Info.plist`, which is what lets macOS route the scheme to it at all.
 * Rejects an invalid scheme name or one the system owns, such as `https`.
 */
export async function registerUrlScheme(scheme: string): Promise<void> {
  if (typeof scheme !== "string") throw new TypeError("URL scheme must be a string")
  return native.registerUrlScheme(scheme)
}
