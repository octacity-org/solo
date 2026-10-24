import { describe, expect, it } from "vitest"
import { openWindow, showAlert } from "@solo/solid"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"

describe.skipIf(!hasNativeTestRenderer)("native alerts", () => {
  it("resolves to the clicked button and rejects a second alert", async () => {
    const root = createSolidNativeTestRoot()
    try {
      root.render(() => <div />)
      const answer = showAlert({
        message: "Delete 3 files?",
        detail: "They move to the Trash.",
        level: "warning",
        buttons: ["Delete", "Cancel"],
      })
      expect(JSON.parse(root.renderer.pendingAlert()!)).toEqual({
        windowId: null,
        level: "warning",
        message: "Delete 3 files?",
        detail: "They move to the Trash.",
        buttons: ["Delete", "Cancel"],
      })
      await expect(showAlert({ message: "Again?" })).rejects.toThrow("An alert is already open")
      expect(() => root.renderer.answerAlert(2)).toThrow("does not exist")
      root.renderer.answerAlert(1)
      await expect(answer).resolves.toBe(1)
      expect(root.renderer.pendingAlert()).toBeNull()

      // The answered alert released its slot.
      const next = showAlert({ message: "Saved" })
      expect(JSON.parse(root.renderer.pendingAlert()!).buttons).toEqual(["OK"])
      root.renderer.answerAlert(0)
      await expect(next).resolves.toBe(0)
    } finally {
      root.unmount()
    }
  })

  it("attaches to an openWindow window", async () => {
    const root = createSolidNativeTestRoot()
    try {
      root.render(() => <div />)
      const win = openWindow(() => <div />, { title: "Second" })
      const answer = showAlert({ message: "Close Second?", window: win })
      expect(JSON.parse(root.renderer.pendingAlert()!).windowId).toBe(win.id)
      root.renderer.answerAlert(0)
      await expect(answer).resolves.toBe(0)
      win.close()
      await expect(showAlert({ message: "Gone", window: win })).rejects.toThrow(`Window ${win.id} is not open`)
    } finally {
      root.unmount()
    }
  })
})
