/// Cross-element text selection.

import { describe, expect, it } from "vitest"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import { Text } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative("text selection", () => {
  it("selects text inside one element", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <Text style={{ fontSize: 20 }}>hello world</Text>
      </div>
    ))

    const selected = renderer.dragSelect(21, 30, 900, 30)
    expect(selected).toBe("hello world")
  })

  it("selects across sibling elements in document order", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20, gap: 8 }}>
        <Text style={{ fontSize: 20 }}>first line</Text>
        <Text style={{ fontSize: 20 }}>second line</Text>
        <Text style={{ fontSize: 20 }}>third line</Text>
      </div>
    ))

    const selected = renderer.dragSelect(21, 30, 900, 300)
    expect(selected).toBe("first line\nsecond line\nthird line")
  })

  it("takes a partial span from the anchor element and whole spans below", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20, gap: 8 }}>
        <Text style={{ fontSize: 20 }}>aaaaaaaaaa</Text>
        <Text style={{ fontSize: 20 }}>bbbb</Text>
      </div>
    ))

    const selected = renderer.dragSelect(21, 30, 900, 300)
    expect(selected).toBe("aaaaaaaaaa\nbbbb")

    renderer.clearSelection()

    const partial = renderer.dragSelect(60, 30, 900, 300)
    expect(partial).not.toBeNull()
    expect(partial!.startsWith("aaaaaaaaaa")).toBe(false)
    expect(partial!.endsWith("\nbbbb")).toBe(true)
  })

  it("resolves a reversed drag the same way", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20, gap: 8 }}>
        <Text style={{ fontSize: 20 }}>alpha</Text>
        <Text style={{ fontSize: 20 }}>beta</Text>
      </div>
    ))

    const downward = renderer.dragSelect(21, 30, 900, 62)
    renderer.clearSelection()
    const upward = renderer.dragSelect(900, 62, 21, 30)
    expect(downward).toBe("alpha\nbeta")
    expect(upward).toBe(downward)
  })

  it("keeps text nested in styled divs selectable", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <div style={{ display: "flex", backgroundColor: "#1e1e2e", padding: 4 }}>
          <Text style={{ fontSize: 20, color: "#cdd6f4" }}>nested text</Text>
        </div>
      </div>
    ))

    expect(renderer.dragSelect(25, 34, 900, 34)).toBe("nested text")
  })

  it("opts out of selection with userSelect none", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <Text style={{ fontSize: 20, userSelect: "none" }}>untouchable</Text>
      </div>
    ))

    expect(renderer.dragSelect(21, 30, 900, 30)).toBeNull()
  })

  it("inherits userSelect none from an ancestor", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20, userSelect: "none" }}>
        <div style={{ display: "flex" }}>
          <Text style={{ fontSize: 20 }}>toolbar label</Text>
        </div>
      </div>
    ))

    expect(renderer.dragSelect(21, 34, 900, 34)).toBeNull()
  })

  it("re-enables selection under a userSelect none ancestor", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20, userSelect: "none" }}>
        <Text style={{ fontSize: 20, userSelect: "text" }}>selectable again</Text>
      </div>
    ))

    const selected = renderer.dragSelect(21, 34, 900, 34)
    expect(selected).toBe("selectable again")
  })

  it("clears the selection", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <Text style={{ fontSize: 20 }}>clearable</Text>
      </div>
    ))
    expect(renderer.dragSelect(21, 30, 900, 30)).toBe("clearable")

    renderer.clearSelection()
    expect(renderer.getSelectedText()).toBeNull()
  })

  it("selects nothing for a click without movement", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <Text style={{ fontSize: 20 }}>stationary</Text>
      </div>
    ))
    // dragSelect with identical start/end coordinates is a click.
    expect(renderer.dragSelect(60, 30, 60, 30)).toBeNull()
  })
})
