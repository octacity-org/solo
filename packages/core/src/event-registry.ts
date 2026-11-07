import type { EventPayload } from "@solo/native"

// Event handler registry, keyed by numeric element ID. The native side emits
// events by element ID, and the Solid host registers each element's handlers
// here.
const eventHandlers = new Map<number, Map<string, (event: EventPayload) => void>>()

export function handleSoloEvent(payload: EventPayload): void {
  const elementHandlers = eventHandlers.get(payload.elementId)
  if (elementHandlers) {
    const handler = elementHandlers.get(payload.eventType)
    if (handler) {
      handler(payload)
    }
  }
}

export function registerEventHandler(
  elementId: number,
  eventType: string,
  handler: (event: EventPayload) => void
): void {
  let elementHandlers = eventHandlers.get(elementId)
  if (!elementHandlers) {
    elementHandlers = new Map()
    eventHandlers.set(elementId, elementHandlers)
  }
  elementHandlers.set(eventType, handler)
}

export function unregisterEventHandler(elementId: number, eventType: string): void {
  const m = eventHandlers.get(elementId)
  if (!m) return
  m.delete(eventType)
  if (m.size === 0) eventHandlers.delete(elementId)
}

export function unregisterEventHandlers(elementId: number): void {
  eventHandlers.delete(elementId)
}

export function clearEventHandlers(): void {
  eventHandlers.clear()
}
