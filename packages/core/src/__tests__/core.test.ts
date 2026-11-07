import { afterEach, describe, expect, it, vi } from "vitest"
import { wrapWithBatching } from "../batching.js"
import {
  clearEventHandlers,
  handleSoloEvent,
  registerEventHandler,
  unregisterEventHandlers,
} from "../event-registry.js"
import type { NativeRenderer } from "../types.js"

function mockRenderer(): NativeRenderer & { applyBatch(json: string): number[]; calls: string[][] } {
  const calls: string[][] = []
  return {
    calls,
    createElement(id, elementType) {
      calls.push(["createElement", String(id), elementType])
    },
    destroyElement() {
      return []
    },
    appendChild(parentId, childId) {
      calls.push(["appendChild", String(parentId), String(childId)])
    },
    removeChild(parentId, childId) {
      calls.push(["removeChild", String(parentId), String(childId)])
    },
    insertBefore(parentId, childId, beforeId) {
      calls.push(["insertBefore", String(parentId), String(childId), String(beforeId)])
    },
    setStyle(id, styleJson) {
      calls.push(["setStyle", String(id), typeof styleJson === "string" ? styleJson : JSON.stringify(styleJson)])
    },
    setText(id, content) {
      calls.push(["setText", String(id), content])
    },
    setEventListener(id, eventType, hasHandler) {
      calls.push(["setEventListener", String(id), eventType, String(hasHandler)])
    },
    setRoot(id) {
      calls.push(["setRoot", String(id)])
    },
    setCustomProp(id, key, valueJson) {
      calls.push([
        "setCustomProp",
        String(id),
        key,
        typeof valueJson === "string" ? valueJson : JSON.stringify(valueJson),
      ])
    },
    commitMutations() {
      calls.push(["commitMutations"])
    },
    applyBatch(json: string) {
      calls.push(["applyBatch", json])
      return []
    },
  }
}

afterEach(() => {
  clearEventHandlers()
})

describe("wrapWithBatching", () => {
  it("queues mutations and flushes them in one applyBatch call", () => {
    const inner = mockRenderer()
    const batched = wrapWithBatching(inner)

    batched.createElement(1, "div")
    batched.setStyle(1, { padding: 8 })
    batched.setText(2, "hello")
    // Nothing reached native yet.
    expect(inner.calls).toEqual([])

    batched.commitMutations()

    expect(inner.calls).toHaveLength(1)
    expect(inner.calls[0][0]).toBe("applyBatch")
    const ops = JSON.parse(inner.calls[0][1])
    expect(ops).toEqual([
      ["createElement", 1, "div"],
      ["setStyle", 1, { padding: 8 }],
      ["setText", 2, "hello"],
    ])
  })

  it("queues raw style objects without double-stringifying them", () => {
    const inner = mockRenderer()
    const batched = wrapWithBatching(inner)

    batched.setStyle(1, { color: "#ff0000" })
    batched.commitMutations()

    const ops = JSON.parse(inner.calls[0][1]) as [string, number, unknown][]
    expect(typeof ops[0][2]).toBe("object")
    expect(ops[0][2]).toEqual({ color: "#ff0000" })
  })

  it("queues setCustomProp as setCustomPropValue so raw strings stay strings", () => {
    const inner = mockRenderer()
    const batched = wrapWithBatching(inner)

    batched.setCustomProp(3, "side", "top")
    batched.commitMutations()

    const ops = JSON.parse(inner.calls[0][1])
    expect(ops).toEqual([["setCustomPropValue", 3, "side", "top"]])
  })

  it("calls commitMutations directly when the queue is empty", () => {
    const inner = mockRenderer()
    const batched = wrapWithBatching(inner)

    batched.commitMutations()
    expect(inner.calls).toEqual([["commitMutations"]])
  })

  it("passes through non-batched methods directly to the inner renderer", () => {
    const inner = mockRenderer()
    const batched = wrapWithBatching(inner)

    batched.focusElement?.(7)
    expect(inner.calls).toEqual([])
  })
})

describe("event registry", () => {
  it("dispatches events to the registered handler by element ID and type", () => {
    const handler = vi.fn()
    registerEventHandler(5, "click", handler)

    handleSoloEvent({ elementId: 5, eventType: "click" } as any)
    expect(handler).toHaveBeenCalledTimes(1)

    handleSoloEvent({ elementId: 5, eventType: "mouseEnter" } as any)
    handleSoloEvent({ elementId: 6, eventType: "click" } as any)
    expect(handler).toHaveBeenCalledTimes(1)
  })

  it("unregisters a single event type and cleans up empty elements", () => {
    const click = vi.fn()
    registerEventHandler(5, "click", click)
    unregisterEventHandlers(5)

    handleSoloEvent({ elementId: 5, eventType: "click" } as any)
    expect(click).not.toHaveBeenCalled()
  })
})
