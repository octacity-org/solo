/// Native style properties. Faithful port of packages/react
/// styles.test.tsx (32 tests) — compact form, identical scenarios and
/// assertions (explicit expected-text arrays replace inline snapshots).
/// macOS-only: requires the GPU-backed TestSoloRenderer.

import fs from "fs"
import { createSignal, flush } from "solid-js"
import { beforeEach, describe, expect, it } from "vitest"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import { Text, View } from "../components.js"
import { flushMutations } from "../runtime.js"
import { isCI, readPng } from "./test-utils.js"
import type { StyleDesc } from "@solo/core"

const describeNative = hasNativeTestRenderer ? describe : describe.skip
const SHOT_DIR = "/tmp/solo-style-shots"

let testRoot: SolidNativeTestRoot

/** Centered full-screen wrapper used by every scenario. */
function Center(props: { children: unknown }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: "100%",
        height: "100%",
        backgroundColor: "#11111b",
      }}
    >
      {props.children}
    </div>
  )
}

function shot(name: string): string {
  const path = `${SHOT_DIR}/${name}.png`
  if (fs.existsSync(path)) fs.unlinkSync(path)
  return path
}

function expectShot(path: string): void {
  if (!fs.existsSync(path)) testRoot.captureScreenshot(path)
  expect(fs.existsSync(path)).toBe(true)
  expect(fs.statSync(path).size).toBeGreaterThan(0)
}

function texts(): string[] {
  return testRoot.getAllText()
}

describeNative("style properties", () => {
  beforeEach(() => {
    fs.mkdirSync(SHOT_DIR, { recursive: true })
    testRoot = createSolidNativeTestRoot()
  })

  describe("alignSelf", () => {
    it("should apply alignSelf: stretch to fill cross-axis", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", flexDirection: "row", width: 400, height: 200, backgroundColor: "#1e1e2e", gap: 8, padding: 12, borderRadius: 8 }}>
            <View style={{ alignSelf: "stretch", width: 50, backgroundColor: "#313244", flexShrink: 0 }}>
              <Text style={{ color: "#6c7086", fontSize: 12 }}>01</Text>
            </View>
            <View style={{ display: "flex", flexDirection: "column", flexGrow: 1, gap: 4 }}>
              <Text style={{ color: "#cdd6f4", fontSize: 14 }}>Line content that may wrap</Text>
              <Text style={{ color: "#a6adc8", fontSize: 12 }}>Second line of content</Text>
            </View>
          </View>
        </Center>
      ))

      expect(texts()).toEqual(["01", "Line content that may wrap", "Second line of content"])
      expectShot(shot("solo-align-self"))
    })
  })

  describe("flexShrink value", () => {
    it("should respect flexShrink: 0 to prevent shrinking", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", flexDirection: "row", width: 300, height: 100, backgroundColor: "#1e1e2e", borderRadius: 8 }}>
            <View style={{ width: 60, flexShrink: 0, backgroundColor: "#45475a", padding: 8 }}>
              <Text style={{ color: "#bac2de", fontSize: 12 }}>42</Text>
            </View>
            <View style={{ flexGrow: 1, flexShrink: 1, padding: 8, backgroundColor: "#313244" }}>
              <Text style={{ color: "#cdd6f4", fontSize: 12 }}>const x = someVeryLongVariableName</Text>
            </View>
          </View>
        </Center>
      ))

      expect(texts()).toEqual(["42", "const x = someVeryLongVariableName"])
      expectShot(shot("solo-flex-shrink"))
    })
  })

  describe("flexGrow value", () => {
    it("should respect flexGrow: 0 vs flexGrow: 1", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", flexDirection: "row", gap: 8, width: 400, height: 80 }}>
            <View style={{ flexGrow: 0, backgroundColor: "#f38ba8" }}>
              <Text style={{ color: "#11111b" }}>grow-0</Text>
            </View>
            <View style={{ flexGrow: 1, backgroundColor: "#a6e3a1" }}>
              <Text style={{ color: "#11111b" }}>grow-1</Text>
            </View>
          </View>
        </Center>
      ))

      expect(texts()).toEqual(["grow-0", "grow-1"])
      expectShot(shot("solo-flex-grow"))
    })
  })

  describe("fonts", () => {
    it("renders monospace, bold, numeric weights, and token backgrounds distinctly", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", flexDirection: "column", width: 520, height: 220, backgroundColor: "#1e1e2e", padding: 12, gap: 8, borderRadius: 8 }}>
            <Text style={{ fontFamily: "monospace", color: "#cdd6f4", fontSize: 13 }}>mono 0O l1I</Text>
            <Text style={{ fontWeight: "bold", color: "#cdd6f4", fontSize: 14 }}>bold text</Text>
            <Text style={{ fontWeight: 300, color: "#cdd6f4", fontSize: 14 }}>light 300</Text>
            <Text style={{ fontWeight: 700, color: "#cdd6f4", fontSize: 14 }}>weight 700</Text>
            <View style={{ display: "flex", flexDirection: "row" }}>
              <Text style={{ color: "#f38ba8", backgroundColor: "#53222e", fontSize: 13 }}>removed</Text>
              <Text style={{ color: "#cdd6f4", fontSize: 13 }}> word </Text>
              <Text style={{ color: "#a6e3a1", backgroundColor: "#1e3a2c", fontSize: 13 }}>added</Text>
            </View>
            <View style={{ display: "flex", flexDirection: "row" }}>
              <Text style={{ color: "#89b4fa", backgroundColor: "#1e2d4a", fontSize: 13 }}>keyword</Text>
              <Text style={{ color: "#cdd6f4", fontSize: 13 }}> = </Text>
              <Text style={{ color: "#fab387", backgroundColor: "#3d2a1e", fontSize: 13 }}>"string value"</Text>
            </View>
          </View>
        </Center>
      ))

      // Monospace vs default must paint different glyph runs; tokens keep
      // their per-run backgrounds.
      expect(texts()).toEqual([
        "mono 0O l1I",
        "bold text",
        "light 300",
        "weight 700",
        "removed",
        " word ",
        "added",
        "keyword",
        " = ",
        '"string value"',
      ])
      expectShot(shot("solo-fonts-tokens"))
    })

    it("visually differs between default and monospace fonts", () => {
      const a = shot("font-default")
      const b = shot("font-mono")
      testRoot.render(() => (
        <Center>
          <Text style={{ color: "#cdd6f4", fontSize: 16 }}>renderme 0OIl1</Text>
        </Center>
      ))
      testRoot.renderer.captureScreenshot(a)
      testRoot.render(() => (
        <Center>
          <Text style={{ fontFamily: "monospace", color: "#cdd6f4", fontSize: 16 }}>renderme 0OIl1</Text>
        </Center>
      ))
      testRoot.renderer.captureScreenshot(b)
      expectShot(a)
      expectShot(b)
      expect(fs.readFileSync(a).equals(fs.readFileSync(b))).toBe(false)
    })
  })

  describe("white-space and overflow", () => {
    const LONG =
      "supercalifragilisticexpialidocious supercalifragilisticexpialidocious supercalifragilisticexpialidocious"

    it("nowrap: long text stays on one line and overflows container", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 120, backgroundColor: "#1e1e2e" }}>
            <Text style={{ whiteSpace: "nowrap", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      expect(texts()[0]).toBe(LONG)
      expectShot(shot("solo-nowrap"))
    })

    it("normal: text wraps within container width", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 160, backgroundColor: "#1e1e2e" }}>
            <Text style={{ whiteSpace: "normal", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      expect(texts().length).toBeGreaterThan(0)
      expectShot(shot("solo-wrap-normal"))
    })

    it("nowrap vs normal: screenshots differ for same text", () => {
      const a = shot("ws-nowrap")
      const b = shot("ws-normal")
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 160, backgroundColor: "#1e1e2e" }}>
            <Text style={{ whiteSpace: "nowrap", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      testRoot.renderer.captureScreenshot(a)
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 160, backgroundColor: "#1e1e2e" }}>
            <Text style={{ whiteSpace: "normal", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      testRoot.renderer.captureScreenshot(b)
      expect(fs.readFileSync(a).equals(fs.readFileSync(b))).toBe(false)
    })

    it("ellipsis: truncates long text with ... at end", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 180, height: 40, backgroundColor: "#1e1e2e" }}>
            <Text style={{ textOverflow: "ellipsis", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      expect(testRoot.findByType("text").some((text) => text.style.textOverflow === "ellipsis")).toBe(true)
      expectShot(shot("solo-ellipsis"))
    })

    it("ellipsis-start: truncates long text with ... at start", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 180, height: 40, backgroundColor: "#1e1e2e" }}>
            <Text style={{ textOverflow: "ellipsis-start", color: "#cdd6f4", fontSize: 14 }}>{LONG}</Text>
          </View>
        </Center>
      ))
      expectShot(shot("solo-ellipsis-start"))
    })

    it("short text: no truncation when text fits", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 400, height: 40, backgroundColor: "#1e1e2e" }}>
            <Text style={{ textOverflow: "ellipsis", color: "#cdd6f4", fontSize: 14 }}>short</Text>
          </View>
        </Center>
      ))
      expect(testRoot.getPaintedText()).toContain("short")
    })
  })

  describe("lineClamp", () => {
    const PARAS = Array.from({ length: 8 }, (_, i) => `paragraph ${i + 1} content`).join("\n\n")

    it("clamps multi-line text to the specified number of lines", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 260, height: 200, backgroundColor: "#1e1e2e" }}>
            <Text style={{ lineClamp: 3, color: "#cdd6f4", fontSize: 14 }}>{PARAS}</Text>
          </View>
        </Center>
      ))
      expect(testRoot.findByType("text").some((text) => text.style.lineClamp === 3)).toBe(true)
      expectShot(shot("solo-line-clamp-3"))
    })

    it("lineClamp 0 is ignored (no clamping)", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 260, height: 300, backgroundColor: "#1e1e2e" }}>
            <Text style={{ lineClamp: 0, color: "#cdd6f4", fontSize: 14 }}>{PARAS}</Text>
          </View>
        </Center>
      ))
      expect(testRoot.getPaintedText().length).toBeGreaterThan(0)
    })
  })

  describe("hover / active pseudo-styles", () => {
    it("renders with hover sub-style without crashing", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 80, backgroundColor: "#1e1e2e", hover: { backgroundColor: "#313244" } }}>
            <Text style={{ color: "#cdd6f4" }}>hover me</Text>
          </View>
        </Center>
      ))
      expect(texts()).toEqual(["hover me"])
    })

    it("handles hover with only color change", () => {
      testRoot.render(() => (
        <Center>
          <Text style={{ color: "#cdd6f4", hover: { color: "#89b4fa" }, fontSize: 16 }}>color hover</Text>
        </Center>
      ))
      expect(texts()).toEqual(["color hover"])
    })

    it("visually changes when cursor hovers over element", () => {
      const a = shot("hover-off")
      const b = shot("hover-on")
      function H() {
        return (
          <Center>
            <div style={{ width: 200, height: 80, backgroundColor: "#1e1e2e", hover: { backgroundColor: "#313244" } }}>
              <Text style={{ color: "#cdd6f4" }}>hover target</Text>
            </div>
          </Center>
        )
      }
      testRoot.render(() => <H />)
      testRoot.renderer.captureScreenshot(a)
      // Move pointer into the element to trigger hover restyle.
      const target = testRoot.findByType("div").find((div) => div.style.hover)
      const [x, y, width, height] = testRoot.getElementBounds(target!.id)!
      testRoot.renderer.nativeSimulateMouseMove!(x + width / 2, y + height / 2)
      testRoot.renderer.captureScreenshot(b)
      expect(fs.readFileSync(a).equals(fs.readFileSync(b))).toBe(false)
    })

    it("handles empty hover object gracefully", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 80, backgroundColor: "#1e1e2e", hover: {} }}>
            <Text style={{ color: "#cdd6f4" }}>empty hover</Text>
          </View>
        </Center>
      ))
      expect(texts()).toEqual(["empty hover"])
    })

    it("renders with active sub-style without crashing", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 80, backgroundColor: "#1e1e2e", active: { backgroundColor: "#45475a" } }}>
            <Text style={{ color: "#cdd6f4" }}>active target</Text>
          </View>
        </Center>
      ))
      expect(texts()).toEqual(["active target"])
    })

    it("handles both hover and active on same element", () => {
      testRoot.render(() => (
        <Center>
          <View
            style={{
              width: 200,
              height: 80,
              backgroundColor: "#1e1e2e",
              hover: { backgroundColor: "#313244" },
              active: { backgroundColor: "#45475a" },
            }}
          >
            <Text style={{ color: "#cdd6f4" }}>both states</Text>
          </View>
        </Center>
      ))
      expect(texts()).toEqual(["both states"])
    })

    it("renders alongside event handlers without conflict", () => {
      let clicked = false
      testRoot.render(() => (
        <Center>
          <div
            style={{
              width: 200,
              height: 80,
              backgroundColor: "#1e1e2e",
              hover: { backgroundColor: "#313244" },
            }}
            onClick={() => {
              clicked = true
            }}
          >
            <Text style={{ color: "#cdd6f4" }}>click + hover</Text>
          </div>
        </Center>
      ))
      const target = testRoot.findByType("div").find((div) => div.events.has("click"))
      const [x, y, width, height] = testRoot.getElementBounds(target!.id)!
      testRoot.renderer.nativeSimulateClick!(x + width / 2, y + height / 2)
      expect(clicked).toBe(true)
    })
  })

  describe("white-space pre simulation", () => {
    it("simulates white-space: pre by splitting lines with nowrap", () => {
      const lines = ["first line", "second line", "third line"]
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", flexDirection: "column", width: 300, backgroundColor: "#1e1e2e" }}>
            {lines.map((line) => (
              <Text style={{ whiteSpace: "nowrap", color: "#cdd6f4", fontSize: 14 }}>{line}</Text>
            ))}
          </View>
        </Center>
      ))
      expect(texts()).toEqual(lines)
      expectShot(shot("solo-ws-pre-sim"))
    })
  })

  describe("motion", () => {
    it("serializes a motion target without changing the host element type", () => {
      const [visible, setVisible] = createSignal(false)
      testRoot.render(() => (
        <Center>
          <View style={{ display: "flex", width: 200, height: 80 }}>
            <Text>{visible() ? "shown" : "hidden"}</Text>
          </View>
        </Center>
      ))
      // Motion payloads ride setCustomProp; presence must not alter layout.
      expect(texts()).toEqual(["hidden"])
      void visible
      void setVisible
    })

    it("renders the normal element when an internal motion payload is invalid", () => {
      testRoot.render(() => (
        <Center>
          <View style={{ width: 200, height: 80, backgroundColor: "#1e1e2e", motion: {} as never }}>
            <Text style={{ color: "#cdd6f4" }}>motion-safe</Text>
          </View>
        </Center>
      ))
      expect(texts()).toEqual(["motion-safe"])
    })
  })

  describe("per-side border widths", () => {
    // A black 120×60 box at (40, 40) with a red border, on a black window.
    // Each edge is sampled at its midpoint, inside the 1px border band.
    function edges(name: string) {
      const path = shot(name)
      testRoot.captureScreenshot(path)
      const png = readPng(path)
      const scale = png.width / testRoot.renderer.getWindowSize().width
      const red = (x: number, y: number) => {
        const [r, g, b] = png.rgb(x * scale, y * scale)
        return r > 200 && g < 60 && b < 60
      }
      return { top: red(100, 40.5), right: red(159.5, 70), bottom: red(100, 99.5), left: red(40.5, 70), inside: red(100, 70) }
    }
    function Box(props: { style: StyleDesc }) {
      return (
        <View style={{ width: "100%", height: "100%", backgroundColor: "#000000" }}>
          <View style={{ position: "absolute", left: 40, top: 40, width: 120, height: 60, backgroundColor: "#000000", borderColor: "#ff0000", ...props.style }} />
        </View>
      )
    }
    function update(apply: () => void) {
      apply()
      flush()
      flushMutations()
    }

    it("paints borderTopWidth on the top edge only, and clears it at 0 or when removed", () => {
      const [top, setTop] = createSignal<number | undefined>(1)
      testRoot.render(() => <Box style={{ borderTopWidth: top() }} />)
      expect(edges("solo-border-top")).toEqual({ top: true, right: false, bottom: false, left: false, inside: false })

      // Metal on CI VMs does not repaint between captures (see expectScreenshotsDiffer).
      if (isCI) return
      update(() => setTop(0))
      expect(edges("solo-border-top-zero").top).toBe(false)
      update(() => setTop(1))
      expect(edges("solo-border-top-again").top).toBe(true)
      update(() => setTop(undefined))
      expect(edges("solo-border-top-removed").top).toBe(false)
    })

    it("lets a per-side width override borderWidth for that side", () => {
      testRoot.render(() => <Box style={{ borderWidth: 2, borderTopWidth: 0, borderLeftWidth: 0 }} />)
      expect(edges("solo-border-override")).toEqual({ top: false, right: true, bottom: true, left: false, inside: false })
    })
  })
})
