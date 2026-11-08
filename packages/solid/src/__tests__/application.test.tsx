import { createRoot, flush } from "solid-js"
import { beforeEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({
  getKeyboardLayout: vi.fn(),
  getThermalState: vi.fn(),
  hideApplication: vi.fn(),
  hideOtherApplications: vi.fn(),
  restartApplication: vi.fn(),
  shouldAutoHideScrollbars: vi.fn(),
  showAllApplications: vi.fn(),
}))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

const solo = await import("@solo/solid")
const { receiveKeyboardLayoutChange, receiveThermalStateChange } = await import("../application.js")

beforeEach(() => { vi.resetAllMocks() })

describe("application controls", () => {
  it("forwards each control to native and surfaces its errors", () => {
    solo.hideApplication()
    solo.hideOtherApplications()
    solo.showAllApplications()
    platform.shouldAutoHideScrollbars.mockReturnValueOnce(true)
    expect(solo.shouldAutoHideScrollbars()).toBe(true)
    expect(platform.hideApplication).toHaveBeenCalledOnce()
    expect(platform.hideOtherApplications).toHaveBeenCalledOnce()
    expect(platform.showAllApplications).toHaveBeenCalledOnce()

    const error = new Error("restartApplication works only in a packaged .app bundle")
    platform.restartApplication.mockImplementationOnce(() => { throw error })
    expect(() => solo.restartApplication()).toThrow(error)
  })

  it("follows the thermal state until its owner is disposed", () => {
    platform.getThermalState.mockReturnValue("nominal")
    const { state, dispose } = createRoot((dispose) => ({ state: solo.createThermalState(), dispose }))
    expect(state()).toBe("nominal")
    receiveThermalStateChange("serious"); flush()
    expect(state()).toBe("serious")
    receiveThermalStateChange("boiling"); flush()
    expect(state()).toBe("serious")
    dispose(); flush()
    receiveThermalStateChange("critical"); flush()
    expect(state()).toBe("serious")
  })

  it("follows the keyboard layout until its owner is disposed", () => {
    platform.getKeyboardLayout.mockReturnValue({ id: "com.apple.keylayout.US", name: "U.S." })
    const { layout, dispose } = createRoot((dispose) => ({ layout: solo.createKeyboardLayout(), dispose }))
    const initial = layout()
    expect(initial).toEqual({ id: "com.apple.keylayout.US", name: "U.S." })
    receiveKeyboardLayoutChange(JSON.stringify({ id: "com.apple.keylayout.US", name: "U.S." })); flush()
    expect(layout()).toBe(initial)
    receiveKeyboardLayoutChange(JSON.stringify({ id: "com.apple.keylayout.German", name: "German" })); flush()
    expect(layout()).toEqual({ id: "com.apple.keylayout.German", name: "German" })
    receiveKeyboardLayoutChange("not json"); flush()
    receiveKeyboardLayoutChange(JSON.stringify({ id: 1 }))
    expect(layout().id).toBe("com.apple.keylayout.German")
    dispose(); flush()
    receiveKeyboardLayoutChange(JSON.stringify({ id: "com.apple.keylayout.French", name: "French" })); flush()
    expect(layout().id).toBe("com.apple.keylayout.German")
  })

  it("needs an owner for the accessors", () => {
    platform.getThermalState.mockReturnValue("nominal")
    expect(() => solo.createThermalState()).toThrow(/owner/)
    expect(() => solo.createKeyboardLayout()).toThrow(/owner/)
  })
})
