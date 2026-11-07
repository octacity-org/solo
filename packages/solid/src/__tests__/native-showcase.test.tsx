/// Showcase composition: markdown + code + diff in one frame, continuous
/// selection across them, theme-metrics retuning, and diff scroll-model
/// honesty.

import fs from "fs"
import path from "path"
import { beforeAll, describe, expect, it } from "vitest"
import {
  createSolidNativeTestRoot,
  hasNativeTestRenderer,
} from "../testing.js"
import { expectScreenshotsDiffer, SHOTS_DIR } from "./test-utils.js"
import { Text } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const NOTES = [
  "## Release notes",
  "",
  "- **Faster** startup",
  "- Native `markdown` and `code`",
].join("\n")

const SNIPPET = [
  "import { createRenderer } from 'gpui'",
  "",
  "export const renderer = createRenderer()",
].join("\n")

const PATCH = [
  "diff --git a/src/server.ts b/src/server.ts",
  "--- a/src/server.ts",
  "+++ b/src/server.ts",
  "@@ -1,3 +1,4 @@",
  " import { createServer } from 'http'",
  "+import { renderer } from './renderer'",
  "",
  " export function boot() {",
  "-  return createServer().listen(3000)",
  "+  return createServer().listen(3000, () => renderer.ready())",
  " }",
].join("\n")

function Panel(props: {
  title: string
  grow?: boolean
  children: unknown
}) {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flexGrow: props.grow ? 1 : 0,
        minHeight: props.grow ? 0 : undefined,
        gap: 8,
        padding: 16,
        borderRadius: 12,
        backgroundColor: "#11111b",
      }}
    >
      <Text style={{ color: "#7f849c", fontSize: 11 }}>{props.title}</Text>
      {props.children}
    </div>
  )
}

beforeAll(() => {
  fs.mkdirSync(SHOTS_DIR, { recursive: true })
})

describeNative("showcase", () => {
  it("renders markdown, code and diff together", () => {
    const shot = path.join(SHOTS_DIR, "showcase.png")
    const { render, renderer } = createSolidNativeTestRoot()

    render(() => (
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          width: "100%",
          height: "100%",
          padding: 24,
          gap: 20,
          backgroundColor: "#060606",
        }}
      >
        <div style={{ display: "flex", flexDirection: "row", gap: 24, flexShrink: 0 }}>
          <div style={{ display: "flex", flexDirection: "column", gap: 8, padding: 16, borderRadius: 12, backgroundColor: "#11111b" }}>
            <Text style={{ color: "#7f849c", fontSize: 11 }}>MARKDOWN</Text>
            <markdown source={NOTES} />
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 8, padding: 16, borderRadius: 12, backgroundColor: "#11111b" }}>
            <Text style={{ color: "#7f849c", fontSize: 11 }}>CODE</Text>
            <code code={SNIPPET} language="typescript" showLineNumbers />
          </div>
        </div>
        {/* flexGrow + minHeight 0 lets the virtualized list take the rest of
            the window instead of leaving dead space under it. */}
        <div style={{ display: "flex", flexDirection: "column", flexGrow: 1, minHeight: 0, gap: 8, padding: 16, borderRadius: 12, backgroundColor: "#11111b" }}>
          <Text style={{ color: "#7f849c", fontSize: 11 }}>DIFF</Text>
          <diff scroll patch={PATCH} wordDiff style={{ flexGrow: 1, minHeight: 0 }} />
        </div>
      </div>
    ))

    renderer.captureScreenshot(shot)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("Release notes")
    expect(painted.some((line) => line.includes("const renderer = createRenderer()"))).toBe(true)
    expect(painted).toContain("src/server.ts")

    expect(fs.existsSync(shot)).toBe(true)
    expect(fs.statSync(shot).size).toBeGreaterThan(0)
  })

  it("retunes every component from the theme metrics, with no rebuild", () => {
    const densePath = path.join(SHOTS_DIR, "metrics-dense.png")
    const roomyPath = path.join(SHOTS_DIR, "metrics-roomy.png")

    const dense = createSolidNativeTestRoot()
    dense.render(() => (
      <div style={{ display: "flex", padding: 20, backgroundColor: "#060606" }}>
        <markdown source={NOTES} theme={{ metrics: { mdTextSize: 10, mdLineHeight: 13 } }} />
        <code code={SNIPPET} language="typescript" theme={{ metrics: { codeTextSize: 9, codeLineHeight: 11 } }} />
      </div>
    ))
    dense.renderer.captureScreenshot(densePath)

    const roomy = createSolidNativeTestRoot()
    roomy.render(() => (
      <div style={{ display: "flex", padding: 20, backgroundColor: "#060606" }}>
        <markdown source={NOTES} theme={{ metrics: { mdTextSize: 18, mdLineHeight: 26 } }} />
        <code code={SNIPPET} language="typescript" theme={{ metrics: { codeTextSize: 16, codeLineHeight: 22 } }} />
      </div>
    ))
    roomy.renderer.captureScreenshot(roomyPath)

    // Same text, different geometry: the metrics only move layout.
    expectScreenshotsDiffer(densePath, roomyPath)
  })

  it("keeps the diff scroll model honest when row heights change", () => {
    const before = path.join(SHOTS_DIR, "diff-rows-before.png")
    const after = path.join(SHOTS_DIR, "diff-rows-after.png")

    const t = createSolidNativeTestRoot()
    t.render(() => (
      <div style={{ display: "flex", flexDirection: "column", width: 600, height: 400, padding: 20, gap: 12 }}>
        <diff patch={PATCH} scroll wordDiff style={{ flexGrow: 1, minHeight: 0 }} />
      </div>
    ))
    t.renderer.captureScreenshot(before)

    // Word-diff toggle changes row heights; the scroll model must recompute.
    t.render(() => (
      <div style={{ display: "flex", flexDirection: "column", width: 600, height: 400, padding: 20, gap: 12 }}>
        <diff patch={PATCH} scroll style={{ flexGrow: 1, minHeight: 0 }} />
      </div>
    ))
    t.renderer.captureScreenshot(after)

    expectScreenshotsDiffer(before, after)
  })

  it("captures the selection wash", () => {
    const shot = path.join(SHOTS_DIR, "selection-wash.png")
    const t = createSolidNativeTestRoot()
    t.render(() => (
      <div style={{ display: "flex", flexDirection: "column", padding: 32 }}>
        <Text style={{ fontSize: 18 }}>washable one</Text>
        <Text style={{ fontSize: 18 }}>washable two</Text>
      </div>
    ))

    t.renderer.dragSelect(30, 40, 900, 120)
    t.renderer.captureScreenshot(shot)

    expect(fs.existsSync(shot)).toBe(true)
    expect(fs.statSync(shot).size).toBeGreaterThan(0)
  })
})
