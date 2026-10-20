/// openWindow() on the GPU-backed test renderer: offscreen secondary windows
/// that share the app's state, with input routed to the element's window.

import { createSignal } from "solid-js"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { handleSoloEvent } from "@solo/core"
import type { EventPayload } from "@solo/native"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import { Text } from "../components.js"
import { openWindow } from "../root.js"
import { flushMutations } from "../runtime.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative("openWindow on the test renderer", () => {
  let testRoot: SolidNativeTestRoot

  beforeEach(() => {
    testRoot = createSolidNativeTestRoot()
  })

  afterEach(() => {
    testRoot.unmount()
  })

  function mountApp() {
    const [count, setCount] = createSignal(0)
    const [lastKey, setLastKey] = createSignal("none")
    testRoot.render(() => (
      <div style={{ width: 200, height: 50 }} onClick={() => setCount((c) => c + 1)}>
        <Text>{`Main: ${count()}`}</Text>
      </div>
    ))
    const closed: string[] = []
    const inspector = openWindow(
      () => (
        <div
          style={{ width: 200, height: 50 }}
          tabIndex={0}
          onClick={() => setCount((c) => c + 10)}
          onKeyDown={(event: EventPayload) => setLastKey(event.key ?? "unknown")}
        >
          <Text>{`Inspector: ${count()} ${lastKey()}`}</Text>
        </div>
      ),
      { title: "Inspector", width: 320, height: 200, onClose: () => closed.push("inspector") }
    )
    const inspectorDiv = testRoot.findByType("div").find((div) => div.events.has("keyDown"))!
    return { inspector, inspectorDiv, closed }
  }

  it("shares state with the main window and receives its own input", () => {
    const { inspectorDiv } = mountApp()
    expect(inspectorDiv).toBeDefined()
    expect(testRoot.getPaintedText()).toContain("Inspector: 0 none")

    // Same window-local point: the element picks the window.
    testRoot.renderer.nativeSimulateClick(10, 10, inspectorDiv.id)
    expect(testRoot.getAllText()).toEqual(["Main: 10"])
    testRoot.renderer.nativeSimulateClick(10, 10)
    expect(testRoot.getAllText()).toEqual(["Main: 11"])

    testRoot.renderer.nativeSimulateKeystrokes(inspectorDiv.id, "a")
    expect(testRoot.getPaintedText()).toContain("Inspector: 11 a")
    expect(testRoot.getElementBounds(inspectorDiv.id)?.[2]).toBe(200)
  })

  it("disposes the window tree when it closes", () => {
    const { inspector, inspectorDiv, closed } = mountApp()
    inspector.close()
    expect(inspector.closed).toBe(true)
    expect(closed).toEqual(["inspector"])
    expect(testRoot.getElement(inspectorDiv.id)).toBeUndefined()
    expect(testRoot.getPaintedText().some((text) => text.startsWith("Inspector"))).toBe(false)
    expect(() => testRoot.renderer.closeWindow(inspector.id)).toThrow(/is not open/)
  })

  it("reports a native close through windowClosed", () => {
    const { inspector, inspectorDiv, closed } = mountApp()
    testRoot.renderer.closeWindow(inspector.id)
    for (const event of testRoot.renderer.drainEvents()) handleSoloEvent(event)
    flushMutations()
    expect(inspector.closed).toBe(true)
    expect(closed).toEqual(["inspector"])
    expect(testRoot.getElement(inspectorDiv.id)).toBeUndefined()
  })

  it("closes its windows on remount and unmount", () => {
    const first = mountApp()
    const second = mountApp()
    expect(first.inspector.closed).toBe(true)
    testRoot.unmount()
    expect(second.inspector.closed).toBe(true)
    expect(() => openWindow(() => <Text>late</Text>)).toThrow(/Call render\(\) before openWindow/)
  })
})
