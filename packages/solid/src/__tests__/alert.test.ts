import { afterEach, describe, expect, it } from "vitest"
import { MockNativeRenderer } from "@solo/core"
import { setSoloRenderer, showAlert } from "@solo/solid"
import type { AppWindow } from "@solo/solid"

class AlertRenderer extends MockNativeRenderer {
  calls: Array<{ options: unknown; windowId: number | undefined }> = []
  answer = 0

  async showAlert(optionsJson: string, windowId?: number): Promise<number> {
    this.calls.push({ options: JSON.parse(optionsJson), windowId })
    return this.answer
  }
}

afterEach(() => setSoloRenderer(new MockNativeRenderer()))

describe("showAlert", () => {
  it("forwards the options and resolves to the clicked button", async () => {
    const renderer = new AlertRenderer()
    setSoloRenderer(renderer)
    renderer.answer = 1
    await expect(showAlert({
      message: "Delete 3 files?",
      detail: "İstanbul 世界",
      level: "critical",
      buttons: ["Delete", "Cancel"],
    })).resolves.toBe(1)
    await expect(showAlert({ message: "Saved" })).resolves.toBe(1)
    expect(renderer.calls).toEqual([
      {
        options: { message: "Delete 3 files?", detail: "İstanbul 世界", level: "critical", buttons: ["Delete", "Cancel"] },
        windowId: undefined,
      },
      { options: { message: "Saved" }, windowId: undefined },
    ])
  })

  it("attaches to the given window", async () => {
    const renderer = new AlertRenderer()
    setSoloRenderer(renderer)
    await showAlert({ message: "Close?", window: { id: 42 } as AppWindow })
    expect(renderer.calls[0]?.windowId).toBe(42)
  })

  it("rejects invalid options before reaching the renderer", async () => {
    const renderer = new AlertRenderer()
    setSoloRenderer(renderer)
    for (const options of [
      { message: " " },
      { message: 1 },
      { message: "m", detail: "" },
      { message: "m", level: "error" },
      { message: "m", buttons: [] },
      { message: "m", buttons: ["OK", ""] },
      { message: "m", buttons: ["OK", 2] },
    ]) {
      await expect(showAlert(options as never), JSON.stringify(options)).rejects.toThrow(TypeError)
    }
    expect(renderer.calls).toEqual([])
  })

  it("rejects when the renderer has no alerts", async () => {
    setSoloRenderer(new MockNativeRenderer())
    await expect(showAlert({ message: "m" })).rejects.toThrow("does not support alerts")
  })
})
