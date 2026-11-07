/// GPUI debug frame overlay: mode cycling and a visual paint check.

import fs from "fs"
import path from "path"
import { beforeAll, describe, expect, it } from "vitest"
import {
  createSolidNativeTestRoot,
  hasNativeTestRenderer,
} from "../testing.js"
import { expectScreenshotsDiffer, SHOTS_DIR } from "./test-utils.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

beforeAll(() => {
  fs.mkdirSync(SHOTS_DIR, { recursive: true })
})

describeNative("debug frame overlay", () => {
  it("defaults to hidden and cycles hidden → minimal → full → hidden", () => {
    const { renderer } = createSolidNativeTestRoot() as unknown as {
      renderer: {
        getDebugFrameOverlay(): string
        cycleDebugFrameOverlay(): string
      }
    }
    expect(renderer.getDebugFrameOverlay()).toBe("hidden")
    expect(renderer.cycleDebugFrameOverlay()).toBe("minimal")
    expect(renderer.cycleDebugFrameOverlay()).toBe("full")
    expect(renderer.cycleDebugFrameOverlay()).toBe("hidden")
  })

  it("sets a mode and keeps it after reset", () => {
    const { renderer } = createSolidNativeTestRoot() as unknown as {
      renderer: {
        setDebugFrameOverlay(m: string): string
        getDebugFrameOverlay(): string
        resetDebugFrameOverlayStats(): void
      }
    }
    expect(renderer.setDebugFrameOverlay("full")).toBe("full")
    expect(renderer.getDebugFrameOverlay()).toBe("full")
    renderer.resetDebugFrameOverlayStats()
    expect(renderer.getDebugFrameOverlay()).toBe("full")
  })

  it("rejects an unknown mode", () => {
    const { renderer } = createSolidNativeTestRoot() as unknown as {
      renderer: { setDebugFrameOverlay(m: string): string }
    }
    expect(() => renderer.setDebugFrameOverlay("nope")).toThrow(
      /hidden, minimal, or full/
    )
  })

  it("paints the overlay into the window screenshot", () => {
    const before = path.join(SHOTS_DIR, "debug-frame-overlay-off.png")
    const after = path.join(SHOTS_DIR, "debug-frame-overlay-full.png")
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div
        style={{
          backgroundColor: "#111111",
          width: "100%",
          height: "100%",
        }}
      />
    ))
    renderer.captureScreenshot(before)
    ;(renderer as unknown as { setDebugFrameOverlay(m: string): string }).setDebugFrameOverlay(
      "full"
    )
    renderer.captureScreenshot(after)
    expectScreenshotsDiffer(before, after)
  })
})
