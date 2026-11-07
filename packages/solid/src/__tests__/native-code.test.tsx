/// The native <code> element: line rendering, headers, line numbers,
/// selection, themes.

import fs from "fs"
import path from "path"
import { beforeAll, describe, expect, it } from "vitest"
import {
  createSolidNativeTestRoot,
  hasNativeTestRenderer,
} from "../testing.js"
import { expectScreenshotsDiffer, SHOTS_DIR } from "./test-utils.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const TS_SOURCE = [
  "interface User {",
  "  id: number",
  "  name: string",
  "}",
  "",
  "const u: User = { id: 1, name: 'ada' }",
].join("\n")

beforeAll(() => {
  fs.mkdirSync(SHOTS_DIR, { recursive: true })
})

describeNative("<code>", () => {
  it("renders one row per source line", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code={"a\nb\nc"} language="ts" />)

    expect(renderer.getPaintedText()).toEqual(["ts", "a", "b", "c"])
  })

  it("keeps JSON-looking source strings as source text", () => {
    const code = '{"not":"a real object"}'
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code={code} language="txt" />)
    expect(renderer.getPaintedText()).toContain(code)
  })

  it("renders an empty code block without crashing", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code="" language="ts" />)
    expect(renderer.findByType("code")).toHaveLength(1)
  })

  it("shows the language header only when a language is given", () => {
    const withLanguage = createSolidNativeTestRoot()
    withLanguage.render(() => <code code="x = 1" language="python" />)
    expect(withLanguage.renderer.getPaintedText()).toContain("python")

    const withoutLanguage = createSolidNativeTestRoot()
    withoutLanguage.render(() => <code code="x = 1" />)
    expect(withoutLanguage.renderer.getPaintedText()).not.toContain("python")
  })

  it("hides the header when showHeader is false", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code="x = 1" language="python" showHeader={false} />)
    expect(renderer.getPaintedText()).not.toContain("python")
  })

  it("renders line numbers when asked", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code={"a\nb\nc"} language="ts" showHeader={false} showLineNumbers />)

    expect(renderer.getPaintedText()).toEqual(["1", "a", "2", "b", "3", "c"])
  })

  it("keeps code text selectable", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <code code={"const answer = 42"} language="ts" showHeader={false} />
      </div>
    ))

    const code = renderer.findByType("code")[0]!
    const [x, y, width] = renderer.getElementBounds(code.id)!
    const selected = renderer.dragSelect(x + 16, y + 16, x + width, y + 16)
    expect(selected).toBe("const answer = 42")
  })

  it("selects across several code lines", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 20 }}>
        <code code={"one\ntwo\nthree"} language="ts" showHeader={false} />
      </div>
    ))

    const code = renderer.findByType("code")[0]!
    const [x, y, width, height] = renderer.getElementBounds(code.id)!
    const selected = renderer.dragSelect(x + 16, y + 16, x + width, y + height)
    expect(selected).toBe("one\ntwo\nthree")
  })

  it("does not select the line-number gutter", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <code code={"alpha\nbeta"} language="ts" showHeader={false} showLineNumbers />)

    const code = renderer.findByType("code")[0]!
    const [x, y, width, height] = renderer.getElementBounds(code.id)!
    const selected = renderer.dragSelect(x + 40, y + 16, x + width, y + height)

    expect(renderer.getPaintedText()).toContain("1")
    expect(selected).not.toMatch(/\d/)
    expect(selected?.endsWith("beta")).toBe(true)
  })

  it("changes appearance when a syntax theme is applied", () => {
    const before = path.join(SHOTS_DIR, "code-theme-before.png")
    const after = path.join(SHOTS_DIR, "code-theme-after.png")

    const a = createSolidNativeTestRoot()
    a.render(() => <code code={TS_SOURCE} language="typescript" showLineNumbers />)
    a.renderer.captureScreenshot(before)

    const b = createSolidNativeTestRoot()
    b.render(() => (
      <code
        code={TS_SOURCE}
        language="typescript"
        showLineNumbers
        theme={{
          syntax: {
            keyword: "#ff0000",
            typeName: "#00ff00",
            property: "#0000ff",
          },
        }}
      />
    ))
    b.renderer.captureScreenshot(after)

    expectScreenshotsDiffer(before, after)
  })

  it("captures a reference screenshot of a highlighted block", () => {
    const shot = path.join(SHOTS_DIR, "code-highlighted.png")
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", padding: 24, backgroundColor: "#060606" }}>
        <code code={TS_SOURCE} language="typescript" showLineNumbers />
      </div>
    ))
    renderer.captureScreenshot(shot)

    expect(fs.existsSync(shot)).toBe(true)
    expect(fs.statSync(shot).size).toBeGreaterThan(0)
  })
})
