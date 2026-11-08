import { beforeEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({ getScreens: vi.fn() }))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

const { getScreens, onScreensChanged } = await import("@solo/solid")
const { receiveScreensChanged } = await import("../screens.js")

beforeEach(() => { vi.resetAllMocks() })

describe("screens", () => {
  it("returns the native screen list unchanged and surfaces its errors", () => {
    const screens = [{
      id: "37d8832a-2d66-02ca-b9f7-8f30a301b230",
      bounds: { x: 0, y: 0, width: 1512, height: 982 },
      visibleBounds: { x: 0, y: 33, width: 1512, height: 949 },
      primary: true,
    }]
    platform.getScreens.mockReturnValueOnce(screens)
    expect(getScreens()).toBe(screens)
    const error = new Error("Listing screens is supported only on macOS")
    platform.getScreens.mockImplementationOnce(() => { throw error })
    expect(() => getScreens()).toThrow(error)
  })

  it("calls every change listener until it unsubscribes", () => {
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = onScreensChanged(first)
    const stopSecond = onScreensChanged(second)
    receiveScreensChanged()
    stopFirst()
    receiveScreensChanged()
    stopSecond()
    receiveScreensChanged()
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledTimes(2)
    expect(() => onScreensChanged("nope" as never)).toThrow(TypeError)
  })
})
