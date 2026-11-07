// Solo Core — private implementation kernel for @solo/solid.
//
// It deliberately does not import Solid: it owns the native mutation protocol
// vocabulary, event registry, and batching used by the Solid host.

export * from "./types.js"
export {
  handleSoloEvent,
  registerEventHandler,
  unregisterEventHandler,
  unregisterEventHandlers,
  clearEventHandlers,
} from "./event-registry.js"
export { EVENT_PROPS, soloEventTypeForProp, attachEventHandler } from "./events.js"
export { wrapWithBatching } from "./batching.js"
export type { BatchingHooks } from "./batching.js"
export { MockNativeRenderer } from "./mock-renderer.js"
export type { MockElement } from "./mock-renderer.js"
