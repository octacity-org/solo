import { beforeEach, describe, expect, it, vi } from "vitest"

const platform = vi.hoisted(() => ({ showNotification: vi.fn(), dismissNotification: vi.fn() }))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

// Fresh module state per test, as each app Worker (and `solo dev` reload) has.
let notifications: typeof import("../notifications.js")
beforeEach(async () => {
  vi.resetModules()
  vi.resetAllMocks()
  notifications = await import("../notifications.js")
})

const response = (tag: string, actionId: string | null) => JSON.stringify({ tag, actionId })

describe("posting notifications", () => {
  it("passes the notification and the tag to native", () => {
    const options = {
      tag: "download-1",
      title: "Download finished",
      body: "report.pdf",
      actions: [{ id: "open", label: "Open" }],
    }
    notifications.showNotification(options)
    notifications.dismissNotification("download-1")
    expect(platform.showNotification).toHaveBeenCalledExactlyOnceWith(options)
    expect(platform.dismissNotification).toHaveBeenCalledExactlyOnceWith("download-1")
  })

  it("surfaces native errors, such as running outside an app bundle", () => {
    const error = new Error("showNotification needs a packaged .app with a CFBundleIdentifier")
    platform.showNotification.mockImplementationOnce(() => { throw error })
    expect(() => notifications.showNotification({ tag: "t", title: "Title" })).toThrow(error)
  })

  it("rejects arguments of the wrong type before dispatch", () => {
    expect(() => notifications.showNotification(undefined as never)).toThrow(TypeError)
    expect(() => notifications.showNotification({ tag: 1, title: "Title" } as never)).toThrow(TypeError)
    expect(() => notifications.dismissNotification(undefined as never)).toThrow(TypeError)
    expect(platform.showNotification).not.toHaveBeenCalled()
    expect(platform.dismissNotification).not.toHaveBeenCalled()
  })
})

describe("notification responses", () => {
  it("buffers responses that arrive before the first listener, such as the click that launched the app", () => {
    notifications.receiveNotificationResponse(response("download-1", null))
    notifications.receiveNotificationResponse(response("message-7", "reply"))
    const first = vi.fn()
    notifications.onNotificationResponse(first)
    expect(first.mock.calls).toEqual([
      [{ tag: "download-1", actionId: null }],
      [{ tag: "message-7", actionId: "reply" }],
    ])
    const second = vi.fn()
    notifications.onNotificationResponse(second)
    expect(second).not.toHaveBeenCalled()
  })

  it("delivers later responses to every listener until it unsubscribes", () => {
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = notifications.onNotificationResponse(first)
    notifications.onNotificationResponse(second)
    notifications.receiveNotificationResponse(response("a", "open"))
    stopFirst()
    notifications.receiveNotificationResponse(response("b", null))
    expect(first.mock.calls).toEqual([[{ tag: "a", actionId: "open" }]])
    expect(second.mock.calls).toEqual([[{ tag: "a", actionId: "open" }], [{ tag: "b", actionId: null }]])
  })

  it("ignores malformed payloads", () => {
    const listener = vi.fn()
    notifications.onNotificationResponse(listener)
    for (const value of [undefined, null, "", "not json", "{}", JSON.stringify({ tag: 3, actionId: null })])
      notifications.receiveNotificationResponse(value)
    expect(listener).not.toHaveBeenCalled()
  })

  it("expects a function", () => {
    expect(() => notifications.onNotificationResponse("nope" as never)).toThrow(TypeError)
  })
})
