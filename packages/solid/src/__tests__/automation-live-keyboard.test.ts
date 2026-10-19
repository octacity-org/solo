/// Live keyboard injection — `liveRendererAsTest` used to throw "not live
/// yet" for all four keyboard methods, so `Locator.fill()` and
/// `Locator.press()` were unusable against a launched Solo application.
///
/// These tests drive the adapter with a recording renderer, so they are
/// deterministic and need no window. Coverage against a real window lives in
/// `examples/tasks/automation.test.ts`, which can only assert end-to-end
/// acceptance when a display server is present.

import { describe, expect, it } from "vitest"
import {
  AutomationError,
  connectStdio,
  handleAutomationRequest,
  InProcessBackend,
  liveRendererAsTest,
} from "../automation.js"
import type { App, LiveAutomationRenderer } from "../automation.js"

const INPUT_ID = 11

/**
 * A live renderer that records every call the adapter makes, in order.
 *
 * Order is the thing under test: keystrokes are delivered to whatever has
 * focus, and an element only exposes its text input handler while focused, so
 * focusing first is not incidental.
 */
function recordingRenderer(
  overrides: Partial<LiveAutomationRenderer> = {}
): { renderer: LiveAutomationRenderer; calls: string[] } {
  const calls: string[] = []
  const renderer: LiveAutomationRenderer = {
    simulateClick() {},
    simulateMouseDown() {},
    simulateMouseUp() {},
    simulateMouseMove() {},
    focusElement(id) {
      calls.push(`focus:${id}`)
    },
    blur() {},
    scrollTo() {},
    getScrollOffset: () => null,
    getAllText: () => [],
    getPaintedText: () => [],
    getSelectedText: () => null,
    clearSelection() {},
    captureScreenshot() {},
    getAutomationTree: () =>
      JSON.stringify({ id: INPUT_ID, type: "input", testId: "search-input" }),
    getElementBounds: () => null,
    clockPause: () => 0,
    clockSet: (ms) => ms,
    clockFastForward: (ms) => ms,
    clockResume: () => 0,
    simulateKeystrokes(keys) {
      calls.push(`keystrokes:${keys}`)
    },
    simulateKeyDown(key, isHeld) {
      calls.push(`keyDown:${key}:${String(isHeld ?? false)}`)
    },
    simulateKeyUp(key) {
      calls.push(`keyUp:${key}`)
    },
    ...overrides,
  }
  return { renderer, calls }
}

/** An `App` whose protocol calls land in `backend`, with no real process. */
async function appOver(backend: InProcessBackend): Promise<App> {
  let listener: ((chunk: string) => void) | undefined
  return connectStdio({
    write: (chunk) => {
      const raw = JSON.parse(chunk.replace(/^data: /, "").trim())
      void handleAutomationRequest(raw, backend).then((reply) => {
        listener?.(reply)
      })
    },
    feed: (fn) => {
      listener = fn
    },
  })
}

/** `Locator.fill` prefixes a select-all, which is platform-dependent. */
const selectAll = process.platform === "darwin" ? "cmd-a" : "ctrl-a"

describe("live keyboard injection", () => {
  it("types into the focused element", () => {
    const { renderer, calls } = recordingRenderer()
    const test = liveRendererAsTest(renderer)

    test.nativeSimulateKeystrokes(INPUT_ID, "n e w s")

    expect(calls).toEqual([`focus:${INPUT_ID}`, "keystrokes:n e w s"])
  })

  it("types without an element when nothing is targeted", () => {
    const { renderer, calls } = recordingRenderer()
    const test = liveRendererAsTest(renderer)

    test.simulateKeystrokes("enter")

    // No focus call: the caller did not name an element, so whatever is
    // already focused receives the keys.
    expect(calls).toEqual(["keystrokes:enter"])
  })

  it("dispatches key down and key up separately, honouring isHeld", () => {
    const { renderer, calls } = recordingRenderer()
    const test = liveRendererAsTest(renderer)

    test.nativeSimulateKeyDown(INPUT_ID, "shift", true)
    test.nativeSimulateKeyUp(INPUT_ID, "shift")

    expect(calls).toEqual([
      `focus:${INPUT_ID}`,
      "keyDown:shift:true",
      `focus:${INPUT_ID}`,
      "keyUp:shift",
    ])
  })

  it("defaults isHeld to false", () => {
    const { renderer, calls } = recordingRenderer()
    liveRendererAsTest(renderer).nativeSimulateKeyDown(INPUT_ID, "a")

    expect(calls).toContain("keyDown:a:false")
  })

  it("reports Unsupported when the renderer has no keyboard at all", () => {
    const { renderer } = recordingRenderer({
      simulateKeystrokes: undefined,
      simulateKeyDown: undefined,
      simulateKeyUp: undefined,
    })
    const test = liveRendererAsTest(renderer)

    const caught = capture(() => test.nativeSimulateKeystrokes(INPUT_ID, "a"))
    expect(caught).toBeInstanceOf(AutomationError)
    expect((caught as AutomationError).code).toBe("Unsupported")
  })

  it("runs the issue's regression snippet through the protocol", async () => {
    const { renderer, calls } = recordingRenderer()
    const app = await appOver(new InProcessBackend(liveRendererAsTest(renderer)))
    try {
      const input = app.getByTestId("search-input")

      await input.fill("news")
      await input.press("cmd-a")
      await input.press("backspace")
      await input.fill("İstanbul 世界")
      await input.press("enter")
    } finally {
      await app.close()
    }

    // Every command focuses the input first, then types. `fill` encodes its
    // text as space-separated keystrokes with a leading select-all, and a
    // space becomes the literal token "space".
    expect(calls).toEqual([
      `focus:${INPUT_ID}`,
      `keystrokes:${selectAll} n e w s`,
      `focus:${INPUT_ID}`,
      "keystrokes:cmd-a",
      `focus:${INPUT_ID}`,
      "keystrokes:backspace",
      `focus:${INPUT_ID}`,
      `keystrokes:${selectAll} İ s t a n b u l space 世 界`,
      `focus:${INPUT_ID}`,
      "keystrokes:enter",
    ])
  })
})

function capture(run: () => void): unknown {
  try {
    run()
  } catch (error) {
    return error
  }
  return undefined
}
