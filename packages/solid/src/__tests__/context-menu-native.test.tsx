import { describe, expect, it } from "vitest"
import { createContextMenu, getSoloRenderer, Text } from "@solo/solid"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"

describe.skipIf(!hasNativeTestRenderer)("native context menu bridge", () => {
  it("routes automation keys to the native menu and settles owner destruction", async () => {
    const root = createSolidNativeTestRoot()
    root.render(() => <div><Text>Owner</Text></div>)
    const owner = root.findByType("div")[0]!.id
    const selected: string[] = []
    const menu = createContextMenu([{ id: "read", label: "Read" }], (id) => selected.push(id))
    const watchdog = setTimeout(() => menu.dismiss(), 3000)
    try {
      const result = menu.show({ elementId: owner })
      root.simulateKeystrokes("down enter")
      await expect(result).resolves.toBe("read")
      expect(selected).toEqual(["read"])
      const cancelled = menu.show({ elementId: owner })
      root.unmount()
      await expect(cancelled).resolves.toBeNull()
      expect(selected).toEqual(["read"])
    } finally { clearTimeout(watchdog); menu.dispose(); root.unmount() }
  }, 8000)

  it("rejects missing owners before opening a native menu", async () => {
    const root = createSolidNativeTestRoot()
    try {
      root.render(() => <Text>Menu owner</Text>)
      expect(getSoloRenderer().showContextMenu).toBeTypeOf("function")
      const menu = createContextMenu([{ id: "read", label: "Read" }])
      await expect(menu.show({ elementId: 999999 })).rejects.toThrow(/owner|element/i)
      menu.dispose()
    } finally { root.unmount() }
  })
})
