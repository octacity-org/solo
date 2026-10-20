import { afterEach, describe, expect, it } from "vitest"
import { MockNativeRenderer } from "@solo/core"
import { selectFiles, selectSavePath, setSoloRenderer } from "@solo/solid"

class DialogRenderer extends MockNativeRenderer {
  openOptions: string[] = []
  saveOptions: string[] = []
  openResult: string[] | null = ["/tmp/Newsprint Archive 世界.json"]
  saveResult: string | null = "/tmp/Exports/Newsprint Notes 世界.md"
  error: Error | null = null

  async selectFiles(optionsJson: string): Promise<string[] | null> {
    this.openOptions.push(optionsJson)
    if (this.error) throw this.error
    return this.openResult
  }

  async selectSavePath(optionsJson: string): Promise<string | null> {
    this.saveOptions.push(optionsJson)
    if (this.error) throw this.error
    return this.saveResult
  }
}

afterEach(() => setSoloRenderer(new MockNativeRenderer()))

describe("file dialogs", () => {
  it("forwards typed open options and preserves Unicode paths", async () => {
    const renderer = new DialogRenderer()
    setSoloRenderer(renderer)
    await expect(selectFiles({ multiple: true, prompt: "Import OPML" })).resolves.toEqual([
      "/tmp/Newsprint Archive 世界.json",
    ])
    expect(JSON.parse(renderer.openOptions[0]!)).toEqual({ multiple: true, prompt: "Import OPML" })
  })

  it("forwards folder selection", async () => {
    const renderer = new DialogRenderer()
    setSoloRenderer(renderer)
    renderer.openResult = ["/Users/me/Folder 世界"]
    await expect(selectFiles({ directories: true, prompt: "Scan" })).resolves.toEqual(["/Users/me/Folder 世界"])
    expect(JSON.parse(renderer.openOptions[0]!)).toEqual({ directories: true, prompt: "Scan" })
  })

  it("forwards suggested save name and initial directory", async () => {
    const renderer = new DialogRenderer()
    setSoloRenderer(renderer)
    await expect(selectSavePath({
      suggestedName: "Newsprint Notes 世界.md",
      initialDirectory: "/tmp/Newsprint Exports",
    })).resolves.toBe("/tmp/Exports/Newsprint Notes 世界.md")
    expect(JSON.parse(renderer.saveOptions[0]!)).toEqual({
      suggestedName: "Newsprint Notes 世界.md",
      initialDirectory: "/tmp/Newsprint Exports",
    })
  })

  it("returns null only for cancellation and preserves bridge errors", async () => {
    const renderer = new DialogRenderer()
    setSoloRenderer(renderer)
    renderer.openResult = null
    renderer.saveResult = null
    await expect(selectFiles()).resolves.toBeNull()
    await expect(selectSavePath()).resolves.toBeNull()
    renderer.error = new Error("Dialog channel closed during shutdown")
    await expect(selectFiles()).rejects.toThrow("channel closed")
    await expect(selectSavePath()).rejects.toThrow("channel closed")
  })

  it("reports an unavailable renderer capability", async () => {
    setSoloRenderer(new MockNativeRenderer())
    await expect(selectFiles()).rejects.toThrow(/support.*file dialogs/i)
    await expect(selectSavePath()).rejects.toThrow(/support.*file dialogs/i)
  })

  it("validates options before opening native UI", async () => {
    const renderer = new DialogRenderer()
    setSoloRenderer(renderer)
    await expect(selectFiles({ multiple: "yes" as never })).rejects.toThrow(/multiple.*boolean/i)
    await expect(selectFiles({ directories: 1 as never })).rejects.toThrow(/directories.*boolean/i)
    await expect(selectFiles({ prompt: " " })).rejects.toThrow(/prompt.*non-empty/i)
    await expect(selectSavePath({ suggestedName: "folder/export.md" })).rejects.toThrow(/filename/i)
    await expect(selectSavePath({ initialDirectory: "relative" })).rejects.toThrow(/absolute/i)
    expect(renderer.openOptions).toEqual([])
    expect(renderer.saveOptions).toEqual([])
  })
})
