import * as native from "@solo/native"

/** A button on a notification. */
export interface NotificationAction {
  /** Reported back as the response's `actionId`. */
  id: string
  label: string
}

export interface ShowNotificationOptions {
  /**
   * Identifies the notification: posting the same tag again replaces it,
   * `dismissNotification(tag)` removes it, and responses carry it back.
   */
  tag: string
  title: string
  body?: string
  /** Buttons offered on the notification, each with a unique `id`. */
  actions?: NotificationAction[]
}

/** The user clicked a notification (`actionId` null) or one of its buttons. */
export interface NotificationResponse {
  tag: string
  actionId: string | null
}

export type NotificationResponseListener = (response: NotificationResponse) => void

const listeners = new Set<NotificationResponseListener>()
// Responses that arrived before the first listener, such as the click that
// launched the app. A `solo dev` reload starts a new Worker, so this module
// starts empty there.
let pending: NotificationResponse[] = []

/**
 * Post a notification to the Notification Center (macOS). Posting a tag that
 * is still shown replaces that notification. The first post asks the user
 * for permission; once they deny it, macOS shows nothing and nothing throws.
 * Throws outside a packaged `.app`, and on other platforms.
 */
export function showNotification(options: ShowNotificationOptions): void {
  if (typeof options !== "object" || options === null) {
    throw new TypeError("showNotification expects an options object")
  }
  if (typeof options.tag !== "string" || typeof options.title !== "string") {
    throw new TypeError("showNotification needs a string tag and title")
  }
  native.showNotification(options)
}

/** Remove the notification posted with `tag`, whether shown or still pending (macOS). */
export function dismissNotification(tag: string): void {
  if (typeof tag !== "string") throw new TypeError("dismissNotification expects a string tag")
  native.dismissNotification(tag)
}

/**
 * Receive the user's clicks on this app's notifications. Returns a function
 * that stops listening. Responses that arrived before the first listener,
 * including the click that launched the app, are passed to that listener
 * before `onNotificationResponse` returns.
 */
export function onNotificationResponse(listener: NotificationResponseListener): () => void {
  if (typeof listener !== "function") throw new TypeError("onNotificationResponse expects a function")
  listeners.add(listener)
  if (pending.length) {
    const responses = pending
    pending = []
    for (const response of responses) listener(response)
  }
  return () => {
    listeners.delete(listener)
  }
}

function parseResponse(value: string | null | undefined): NotificationResponse | undefined {
  if (!value) return undefined
  let parsed: unknown
  try {
    parsed = JSON.parse(value)
  } catch {
    return undefined
  }
  if (typeof parsed !== "object" || parsed === null) return undefined
  const { tag, actionId } = parsed as Record<string, unknown>
  if (typeof tag !== "string" || (actionId !== null && typeof actionId !== "string")) return undefined
  return { tag, actionId }
}

/** The renderer's element-0 `notificationResponse` event (root.ts). */
export function receiveNotificationResponse(value: string | null | undefined): void {
  const response = parseResponse(value)
  if (!response) return
  if (!listeners.size) {
    pending.push(response)
    return
  }
  for (const listener of [...listeners]) listener({ ...response })
}
