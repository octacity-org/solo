import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { createRoot, createSignal, flush } from "solid-js"

const platform = vi.hoisted(() => ({ setDockMenu: vi.fn(), addRecentDocument: vi.fn() }))
vi.mock("@solo/native", async (original) => ({
  ...await original<typeof import("@solo/native")>(),
  ...platform,
}))

const { addRecentDocument, configureDockMenu, onApplicationReopen } = await import("@solo/solid")
const { receiveApplicationReopen, receiveDockMenuAction } = await import("../dock.js")

const disposers: Array<() => void> = []
afterEach(() => {
  for (const dispose of disposers.splice(0).reverse()) dispose()
})
beforeEach(() => { vi.resetAllMocks() })
function owned(fn: () => void): () => void {
  let dispose!: () => void
  createRoot((cleanup) => { dispose = cleanup; fn() })
  disposers.push(dispose)
  return dispose
}
const lastMenu = () => JSON.parse(platform.setDockMenu.mock.lastCall![0]) as Array<Record<string, unknown>>
const choose = (token: unknown) => receiveDockMenuAction(String(token))

describe("Dock menu", () => {
  it("sends menu-bar-shaped items and runs only the current enabled action", () => {
    const [label, setLabel] = createSignal("New Note")
    const newNote = vi.fn()
    const sync = vi.fn()
    owned(() => configureDockMenu(() => [
      { id: "new", label: label(), run: newNote },
      { type: "separator" },
      { type: "status", label: "Synced" },
      { id: "sync", label: "Sync", enabled: false, checked: true, run: sync },
    ]))
    const first = lastMenu()
    expect(first).toEqual([
      { type: "action", token: expect.any(String), label: "New Note", enabled: true, checked: false },
      { type: "separator" },
      { type: "status", label: "Synced" },
      { type: "action", token: expect.any(String), label: "Sync", enabled: false, checked: true },
    ])
    choose(first[3]!.token)
    choose(first[0]!.token)
    expect(sync).not.toHaveBeenCalled()
    expect(newNote).toHaveBeenCalledOnce()

    setLabel("New Window"); flush()
    expect(lastMenu()[0]).toMatchObject({ label: "New Window" })
    choose(first[0]!.token)
    expect(newNote).toHaveBeenCalledOnce()
    choose(lastMenu()[0]!.token)
    expect(newNote).toHaveBeenCalledTimes(2)
  })

  it("clears the native items and stops actions when its owner is disposed", () => {
    const run = vi.fn()
    const dispose = owned(() => configureDockMenu(() => [{ id: "a", label: "A", run }]))
    const token = lastMenu()[0]!.token
    dispose()
    expect(platform.setDockMenu).toHaveBeenLastCalledWith("[]")
    choose(token)
    expect(run).not.toHaveBeenCalled()
    owned(() => configureDockMenu(() => []))
  })

  it("rejects invalid items, a second configuration, and use outside an owner", () => {
    expect(() => owned(() => configureDockMenu(() => [{ id: "a", label: " ", run() {} }]))).toThrow("Dock menu action label")
    expect(() => owned(() => configureDockMenu(() => [
      { id: "a", label: "A", run() {} }, { id: "a", label: "B", run() {} },
    ]))).toThrow("Duplicate dock menu action ID: a")
    expect(() => configureDockMenu(() => [])).toThrow("inside a Solid component")
    owned(() => configureDockMenu(() => []))
    expect(() => owned(() => configureDockMenu(() => []))).toThrow("already configured")
  })
})

describe("application reopen", () => {
  it("calls every listener until it unsubscribes", () => {
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = onApplicationReopen(first)
    const stopSecond = onApplicationReopen(second)
    receiveApplicationReopen()
    stopFirst()
    receiveApplicationReopen()
    stopSecond()
    receiveApplicationReopen()
    expect(first).toHaveBeenCalledOnce()
    expect(second).toHaveBeenCalledTimes(2)
  })
})

describe("recent documents", () => {
  it("passes absolute paths through and rejects relative ones", () => {
    addRecentDocument("/Users/me/Notes/a b 世界.txt")
    expect(platform.addRecentDocument).toHaveBeenCalledExactlyOnceWith("/Users/me/Notes/a b 世界.txt")
    expect(() => addRecentDocument("notes.txt")).toThrow(TypeError)
    expect(platform.addRecentDocument).toHaveBeenCalledOnce()
  })
})
