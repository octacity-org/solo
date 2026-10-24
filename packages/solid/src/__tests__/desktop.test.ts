import { beforeEach, describe, expect, it, vi } from "vitest"
import * as solo from "@solo/solid"

const platform = vi.hoisted(() => ({
  openExternalUrl: vi.fn(),
  writeClipboardText: vi.fn(),
  moveToTrash: vi.fn(),
  revealPath: vi.fn(),
  openPath: vi.fn(),
}))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

beforeEach(() => { vi.resetAllMocks() })

describe("public desktop actions", () => {
  it("opens browser URLs without requiring a mounted renderer", () => {
    const url = "https://example.com/article?q=%E4%B8%96%E7%95%8C&literal=$(echo)"
    solo.openExternalUrl(url)
    expect(platform.openExternalUrl).toHaveBeenCalledExactlyOnceWith(url)
  })

  it("writes Unicode, multiline, and empty text unchanged", () => {
    for (const text of ["Hello 世界 👋\nİstanbul", "", "https://example.com/?q=';$()"])
      solo.writeClipboardText(text)
    expect(platform.writeClipboardText.mock.calls).toEqual([
      ["Hello 世界 👋\nİstanbul"], [""], ["https://example.com/?q=';$()"],
    ])
  })

  it("rejects non-string arguments before platform dispatch", () => {
    for (const value of [undefined, null, 42, {}, new URL("https://example.com")]) {
      expect(() => solo.openExternalUrl(value as never)).toThrow("URL must be a string")
      expect(() => solo.writeClipboardText(value as never)).toThrow("Clipboard text must be a string")
    }
    expect(platform.openExternalUrl).not.toHaveBeenCalled()
    expect(platform.writeClipboardText).not.toHaveBeenCalled()
  })

  it("preserves catchable validation, platform, and unsupported errors", () => {
    const error = new Error("macOS refused the operation")
    platform.openExternalUrl.mockImplementation(() => { throw error })
    platform.writeClipboardText.mockImplementation(() => { throw error })
    expect(() => solo.openExternalUrl("https://example.com")).toThrow(error)
    expect(() => solo.writeClipboardText("text")).toThrow(error)
  })
})

describe("moveToTrash", () => {
  it("returns one result per path, in order", async () => {
    const results = [
      { path: "/tmp/a b.txt", ok: true },
      { path: "/tmp/missing", ok: false, error: "The file doesn’t exist." },
    ]
    platform.moveToTrash.mockResolvedValue(results)
    await expect(solo.moveToTrash(["/tmp/a b.txt", "/tmp/missing"])).resolves.toEqual(results)
    expect(platform.moveToTrash).toHaveBeenCalledExactlyOnceWith(["/tmp/a b.txt", "/tmp/missing"])
  })

  it("resolves an empty list without platform dispatch", async () => {
    await expect(solo.moveToTrash([])).resolves.toEqual([])
    expect(platform.moveToTrash).not.toHaveBeenCalled()
  })

  it("rejects malformed input before platform dispatch", async () => {
    for (const paths of [undefined, "/tmp/a", [42], [""], ["relative/file"], ["/tmp/a", "./b"]]) {
      await expect(solo.moveToTrash(paths as never)).rejects.toThrow(TypeError)
    }
    expect(platform.moveToTrash).not.toHaveBeenCalled()
  })

  it("rejects when the platform cannot trash at all", async () => {
    platform.moveToTrash.mockImplementation(() => {
      throw new Error("Moving to the Trash is supported only on macOS")
    })
    await expect(solo.moveToTrash(["/tmp/a"])).rejects.toThrow("supported only on macOS")
  })
})

describe("revealPath and openPath", () => {
  it("pass absolute paths through unchanged", () => {
    solo.revealPath("/Users/me/Downloads/a b 世界.zip")
    solo.openPath("/Users/me/Documents/$(echo).txt")
    expect(platform.revealPath).toHaveBeenCalledExactlyOnceWith("/Users/me/Downloads/a b 世界.zip")
    expect(platform.openPath).toHaveBeenCalledExactlyOnceWith("/Users/me/Documents/$(echo).txt")
  })

  it("reject relative, empty, and non-string paths before platform dispatch", () => {
    for (const value of [undefined, 42, "", "relative/file", "./file", "~/file"]) {
      expect(() => solo.revealPath(value as never)).toThrow(TypeError)
      expect(() => solo.openPath(value as never)).toThrow(TypeError)
    }
    expect(platform.revealPath).not.toHaveBeenCalled()
    expect(platform.openPath).not.toHaveBeenCalled()
  })

  it("preserve catchable platform errors", () => {
    const error = new Error("\"/missing\": No such file or directory (os error 2)")
    platform.revealPath.mockImplementation(() => { throw error })
    platform.openPath.mockImplementation(() => { throw error })
    expect(() => solo.revealPath("/missing")).toThrow(error)
    expect(() => solo.openPath("/missing")).toThrow(error)
  })
})
