import { beforeEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({ registerUrlScheme: vi.fn() }))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

// Each test gets fresh module state, as each app Worker (and each `solo dev`
// reload) does.
let openUrls: typeof import("../open-urls.js")
beforeEach(async () => {
  vi.resetModules()
  vi.resetAllMocks()
  openUrls = await import("../open-urls.js")
})

describe("open URLs", () => {
  it("buffers URLs that arrive before the first listener, then delivers them once", () => {
    openUrls.receiveOpenUrls(["myapp://cold"])
    openUrls.receiveOpenUrls(["myapp://second", "file:///tmp/a%20b.txt"])
    const first = vi.fn()
    openUrls.onOpenUrls(first)
    expect(first.mock.calls).toEqual([[["myapp://cold", "myapp://second", "file:///tmp/a%20b.txt"]]])

    const second = vi.fn()
    openUrls.onOpenUrls(second)
    expect(second).not.toHaveBeenCalled()
  })

  it("delivers later URLs to every listener until it unsubscribes", () => {
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = openUrls.onOpenUrls(first)
    openUrls.onOpenUrls(second)
    openUrls.receiveOpenUrls(["myapp://item/42?x=%F0%9F%91%8B"])
    stopFirst()
    openUrls.receiveOpenUrls(["myapp://later"])
    expect(first.mock.calls).toEqual([[["myapp://item/42?x=%F0%9F%91%8B"]]])
    expect(second.mock.calls).toEqual([[["myapp://item/42?x=%F0%9F%91%8B"]], [["myapp://later"]]])
  })

  it("buffers again once every listener is gone", () => {
    openUrls.onOpenUrls(vi.fn())()
    openUrls.receiveOpenUrls(["myapp://while-unsubscribed"])
    const listener = vi.fn()
    openUrls.onOpenUrls(listener)
    expect(listener).toHaveBeenCalledExactlyOnceWith(["myapp://while-unsubscribed"])
  })

  it("ignores empty events and rejects a non-function listener", () => {
    openUrls.receiveOpenUrls([])
    openUrls.receiveOpenUrls(undefined)
    const listener = vi.fn()
    openUrls.onOpenUrls(listener)
    expect(listener).not.toHaveBeenCalled()
    expect(() => openUrls.onOpenUrls("nope" as never)).toThrow(TypeError)
  })

  it("passes a scheme to native registration and surfaces its rejection", async () => {
    platform.registerUrlScheme.mockResolvedValueOnce(undefined)
    await openUrls.registerUrlScheme("myapp")
    expect(platform.registerUrlScheme).toHaveBeenCalledExactlyOnceWith("myapp")

    platform.registerUrlScheme.mockImplementationOnce(() => {
      throw new Error('The "https" URL scheme belongs to the system and cannot be registered')
    })
    await expect(openUrls.registerUrlScheme("https")).rejects.toThrow("belongs to the system")
    await expect(openUrls.registerUrlScheme(42 as never)).rejects.toThrow(TypeError)
  })
})
