import type { EventPayload } from "@solo/native"
import { registerEventHandler, unregisterEventHandler } from "./event-registry.js"

/// Mapping from framework event prop names (onClick, onKeyDown, ...) to the
/// native event types Rust emits in EventPayload.eventType for Solid props.

export const EVENT_PROPS = [
  // Custom element events
  ["onToggleFile", "toggleFile"],
  ["onShowMore", "showMore"],
  ["onLineClick", "lineClick"],
  ["onLinkClick", "linkClick"],
  ["onRangeChange", "rangeChange"],
  ["onChange", "change"],
  ["onSubmit", "submit"],
  // <webview> events (macOS only)
  ["onLoad", "load"],
  ["onNavigation", "navigation"],
  ["onNavigationRequest", "navigationRequest"],
  ["onLoadError", "loadError"],
  // Mouse events
  ["onClick", "click"],
  ["onMouseDown", "mouseDown"],
  ["onMouseUp", "mouseUp"],
  ["onMouseEnter", "mouseEnter"],
  ["onMouseLeave", "mouseLeave"],
  ["onMouseMove", "mouseMove"],
  ["onMouseDownOutside", "mouseDownOutside"],
  // Keyboard events (require focus — tabIndex or autoFocus)
  ["onKeyDown", "keyDown"],
  ["onKeyUp", "keyUp"],
  // Focus events
  ["onFocus", "focus"],
  ["onBlur", "blur"],
  // Scroll events
  ["onScroll", "scroll"],
] as const

const EVENT_PROP_TO_TYPE = new Map<string, string>(
  EVENT_PROPS.map(([prop, type]) => [prop, type])
)

/** The native event type for a framework event prop name, or null. */
export function soloEventTypeForProp(name: string): string | null {
  return EVENT_PROP_TO_TYPE.get(name) ?? null
}

// Deprecated alias — remove in next major
export const gpuixEventTypeForProp = soloEventTypeForProp

/** Attach one event handler through the shared registry + native listener. */
export function attachEventHandler(
  renderer: { setEventListener(id: number, eventType: string, hasHandler: boolean): void },
  id: number,
  eventType: string,
  handler: ((event: EventPayload) => void) | null | undefined
): void {
  if (handler) {
    registerEventHandler(id, eventType, handler)
    renderer.setEventListener(id, eventType, true)
  } else {
    unregisterEventHandler(id, eventType)
    renderer.setEventListener(id, eventType, false)
  }
}
