/// SSE stdin/stdout transport. Logs without a `data:` prefix cannot break it.

import { describe, expect, it } from "vitest"
import {
  connectStdio,
  connectTest,
  encodeSse,
  handleAutomationRequest,
  InProcessBackend,
  PROTOCOL_VERSION,
} from "../automation.js"
import type { TestAutomationRenderer } from "../automation.js"

function fakeRenderer(tree?: object): TestAutomationRenderer {
  let clicks = 0
  return {
    nativeSimulateClick() {
      clicks += 1
    },
    nativeSimulateMouseDown() {},
    nativeSimulateMouseUp() {},
    nativeSimulateMouseMove() {},
    nativeSimulateScrollWheel() {},
    simulateKeystrokes() {},
    nativeSimulateKeystrokes() {},
    nativeSimulateKeyDown() {},
    nativeSimulateKeyUp() {},
    scrollTo() {},
    getScrollOffset: () => null,
    getAllText: () => [`clicks:${clicks}`],
    getPaintedText: () => [`clicks:${clicks}`],
    getSelectedText: () => null,
    clearSelection() {},
    captureScreenshot() {},
    getAutomationTree: () =>
      JSON.stringify(tree ?? {
        id: 1,
        type: "div",
        testId: "inc",
        bounds: { x: 0, y: 0, width: 40, height: 20 },
        children: [{ id: 2, type: "text", text: `clicks:${clicks}` }],
      }),
    getElementBounds: () => [0, 0, 40, 20],
    clockPause: () => 0,
    clockSet: (nowMs) => nowMs,
    clockFastForward: (deltaMs) => deltaMs,
    clockResume: () => 0,
  }
}

describe("Locator.textContent", () => {
  it("includes descendant text in document order", async () => {
    // <Text testId="count">Presses: {count()}</Text> with a nested element.
    const app = await connectTest(fakeRenderer({
      id: 1,
      type: "div",
      children: [{
        id: 2,
        type: "text",
        testId: "count",
        children: [
          { id: 3, type: "text", text: "Presses: " },
          { id: 4, type: "text", text: "3" },
          { id: 5, type: "div", children: [{ id: 6, type: "text", text: " (max)" }] },
        ],
      }],
    }))
    expect(await app.getByTestId("count").textContent()).toBe("Presses: 3 (max)")
    expect(await app.getByText("Presses:").textContent()).toBe("Presses: ")
    await app.close()
  })
})

describe("automation stdio", () => {
  it("round-trips through data: lines with log noise", async () => {
    const backend = new InProcessBackend(fakeRenderer())
    let listener: ((chunk: string) => void) | undefined
    const app = await connectStdio({
      write: (chunk) => {
        const raw = JSON.parse(chunk.replace(/^data: /, "").trim())
        void handleAutomationRequest(raw, backend).then((reply) => {
          listener?.(`[child] still starting\n${reply}`)
        })
      },
      feed: (fn) => {
        listener = fn
      },
    })

    await app.getByTestId("inc").click()
    expect(await app.getByText("clicks:1").textContent()).toBe("clicks:1")
    await app.close()
  })

  it("initialize handshake matches the protocol version", async () => {
    const backend = new InProcessBackend(fakeRenderer())
    const reply = await handleAutomationRequest(
      {
        id: 1,
        method: "initialize",
        params: { protocolVersion: PROTOCOL_VERSION, client: "test" },
      },
      backend
    )
    expect(reply.startsWith("data: ")).toBe(true)
    expect(reply).toContain('"protocolVersion":1')
  })

  it("encodeSse prefixes every protocol message", () => {
    expect(encodeSse({ id: 1, method: "blur", params: {} })).toMatch(
      /^data: \{/
    )
  })
})
