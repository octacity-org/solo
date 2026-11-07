/// The native <diff> element: file headers, hunk headers, notices,
/// collapse, events, gutters, virtualization, screenshots.

import fs from "fs"
import path from "path"
import { beforeAll, describe, expect, it, vi } from "vitest"
import {
  createSolidNativeTestRoot,
  hasNativeTestRenderer,
} from "../testing.js"
import { expectScreenshotsDiffer, SHOTS_DIR } from "./test-utils.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const PATCH = [
  "diff --git a/src/app.ts b/src/app.ts",
  "index 1111111..2222222 100644",
  "--- a/src/app.ts",
  "+++ b/src/app.ts",
  "@@ -1,7 +1,8 @@",
  "import { createServer } from 'http'",
  "",
  "-const port = 3000",
  "+const port = 8080",
  "+const host = '0.0.0.0'",
  "",
  " export function start() {",
  "-  return createServer().listen(port)",
  "+  return createServer().listen(port, host)",
  " }",
]
  .map((line, ix) =>
    ix >= 5 && !line.startsWith("-") && !line.startsWith("+") && !line.startsWith("@")
      ? ` ${line}`
      : line
  )
  .join("\n")

const TWO_FILES = [
  "diff --git a/README.md b/README.md",
  "--- a/README.md",
  "+++ b/README.md",
  "@@ -1,2 +1,2 @@",
  " # Title",
  "-old line",
  "+new line",
  "diff --git a/src/lib.rs b/src/lib.rs",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/src/lib.rs",
  "@@ -0,0 +1,3 @@",
  "+pub fn hello() -> &'static str {",
  '+    "hi"',
  "+}",
].join("\n")

function longPatch(lines: number): string {
  const rows = [
    "diff --git a/big.ts b/big.ts",
    "--- a/big.ts",
    "+++ b/big.ts",
    `@@ -1,${lines} +1,${lines} @@`,
  ]
  for (let i = 0; i < lines; i++) {
    rows.push(i % 7 === 0 ? `-const v${i} = ${i}` : ` const v${i} = ${i}`)
    if (i % 7 === 0) rows.push(`+const v${i} = ${i + 1}`)
  }
  return rows.join("\n")
}

beforeAll(() => {
  fs.mkdirSync(SHOTS_DIR, { recursive: true })
})

describeNative("<diff>", () => {
  it("renders file headers with addition and deletion counts", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <diff patch={PATCH} style={{ width: "100%", height: "100%" }} />)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("src/app.ts")
    expect(painted).toContain("+3")
    expect(painted).toContain("−2")
  })

  it("renders hunk headers and line numbers", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <diff patch={PATCH} style={{ width: "100%", height: "100%" }} />)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("@@ -1,7 +1,8 @@")
    expect(painted).toContain("1")
    expect(painted).toContain("+")
    expect(painted).toContain("−")
  })

  it("renders several files with their notices", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <diff patch={TWO_FILES} style={{ width: "100%", height: "100%" }} />)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("README.md")
    expect(painted).toContain("src/lib.rs")
    expect(painted).toContain("New file")
  })

  it("renders 'No changes' for an empty patch", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <diff patch="" style={{ width: "100%", height: "100%" }} />)
    expect(renderer.getPaintedText()).toContain("No changes")
  })

  it("collapses a file down to its header", () => {
    const expanded = createSolidNativeTestRoot()
    expanded.render(() => <diff patch={TWO_FILES} style={{ width: "100%", height: "100%" }} />)
    const expandedPainted = expanded.renderer.getPaintedText()

    const collapsed = createSolidNativeTestRoot()
    collapsed.render(() => (
      <diff patch={TWO_FILES} collapsedPaths={["README.md"]} style={{ width: "100%", height: "100%" }} />
    ))
    const collapsedPainted = collapsed.renderer.getPaintedText()

    expect(expandedPainted).toContain("old line")
    expect(collapsedPainted).not.toContain("old line")
    expect(collapsedPainted).toContain("README.md")
  })

  it("fires onToggleFile with the file path", () => {
    const onToggleFile = vi.fn()
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <diff
        patch={TWO_FILES}
        onToggleFile={onToggleFile}
        style={{ width: "100%", height: "100%" }}
      />
    ))

    renderer.nativeSimulateClick(40, 30)
    expect(onToggleFile).toHaveBeenCalled()
    expect(onToggleFile.mock.calls[0][0].value).toBe("README.md")
  })

  it("fires onLineClick with line text and numbers", () => {
    const onLineClick = vi.fn()
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <diff
        patch={TWO_FILES}
        onLineClick={onLineClick}
        collapsedPaths={["src/lib.rs"]}
        style={{ width: "100%", height: "100%" }}
      />
    ))

    const diff = renderer.findByType("diff")[0]!
    const [x, y] = renderer.getElementBounds(diff.id)!
    renderer.nativeSimulateClick(x + 60, y + 75)
    expect(onLineClick).toHaveBeenCalled()
    const event = onLineClick.mock.calls[0][0]
    expect(event.value).toBe("# Title")
    expect(event.oldLine).toBe(1)
    expect(event.newLine).toBe(1)
  })

  it("keeps diff code selectable but not the gutters", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => <diff patch={TWO_FILES} style={{ width: "100%", height: "100%" }} />)

    const diff = renderer.findByType("diff")[0]!
    const [x, y, width] = renderer.getElementBounds(diff.id)!
    const selected = renderer.dragSelect(x + 116, y + 75, x + width, y + 75)
    expect(selected).toBe("# Title")
    expect(renderer.getPaintedText()).toContain("·")
    expect(selected).not.toContain("·")
  })

  it("caps visible lines and paints Show more", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <diff patch={longPatch(400)} maxLines={50} style={{ width: "100%", height: "100%" }} />
    ))

    const painted = renderer.getPaintedText()
    expect(painted).toContain("big.ts")
    expect(painted.join("\n")).toMatch(/Show \d+ more lines/)
  })

  it("virtualizes very long patches instead of building every row", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <diff
        patch={longPatch(2000)}
        scroll
        style={{ width: "100%", height: 600 }}
      />
    ))

    // Only rows near the top are built; the rest materialize on demand.
    expect(renderer.getAllText().length).toBeLessThan(200)
    const list = renderer.findByType("diff")[0]
    renderer.scrollToItem(list.id, 1500)
    expect(renderer.getPaintedText().length).toBeGreaterThan(0)
  })

  it("changes appearance when a theme is applied", () => {
    const before = path.join(SHOTS_DIR, "diff-theme-before.png")
    const after = path.join(SHOTS_DIR, "diff-theme-after.png")

    const a = createSolidNativeTestRoot()
    a.render(() => <diff patch={TWO_FILES} style={{ width: "100%", height: "100%" }} />)
    a.renderer.captureScreenshot(before)

    const b = createSolidNativeTestRoot()
    b.render(() => (
      <diff
        patch={TWO_FILES}
        theme={{ diffAdd: "#00ff00", diffDel: "#ff0000" }}
        style={{ width: "100%", height: "100%" }}
      />
    ))
    b.renderer.captureScreenshot(after)

    expectScreenshotsDiffer(before, after)
  })

  it("captures a reference screenshot of a rendered patch", () => {
    const shot = path.join(SHOTS_DIR, "diff-reference.png")
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <div style={{ display: "flex", padding: 24, backgroundColor: "#060606" }}>
        <diff patch={PATCH} style={{ width: "100%", height: "100%" }} />
      </div>
    ))
    renderer.captureScreenshot(shot)

    expect(fs.existsSync(shot)).toBe(true)
    expect(fs.statSync(shot).size).toBeGreaterThan(0)
  })
})
