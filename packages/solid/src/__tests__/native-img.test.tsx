/// Tests for the custom <img> element — native image rendering via the
/// custom-element pipeline and visual screenshot behavior.

import fs from "fs"
import { beforeEach, describe, expect, it } from "vitest"
import { createSignal } from "solid-js"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import { Text, View } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

const IMAGE_FIXTURE_PATH = "/tmp/solo-img-fixture.svg"

function writeSvgFixture(filePath: string): void {
  const svg = [
    '<svg xmlns="http://www.w3.org/2000/svg" width="240" height="140" viewBox="0 0 240 140">',
    '<rect x="0" y="0" width="240" height="140" fill="#1e2d59"/>',
    '<rect x="16" y="16" width="208" height="108" rx="14" fill="#5ca9ff"/>',
    '<circle cx="68" cy="70" r="24" fill="#ffd166"/>',
    '<rect x="112" y="50" width="88" height="14" rx="7" fill="#20304f"/>',
    '<rect x="112" y="74" width="70" height="12" rx="6" fill="#2a3c61"/>',
    "</svg>",
  ].join("")
  fs.writeFileSync(filePath, svg, "utf8")
}

describeNative("custom element: img", () => {
  let testRoot: SolidNativeTestRoot

  beforeEach(() => {
    writeSvgFixture(IMAGE_FIXTURE_PATH)
    testRoot = createSolidNativeTestRoot()
  })

  describe("rendering", () => {
    it("should create img element and forward src/objectFit props", () => {
      testRoot.render(() => (
        <div style={{ width: 400, height: 240 }}>
          <img
            src={IMAGE_FIXTURE_PATH}
            objectFit="cover"
            style={{ width: 220, height: 120 }}
          />
        </div>
      ))

      const images = testRoot.renderer.findByType("img")
      expect(images.length).toBe(1)
      const raw = JSON.parse(
        (testRoot.renderer as unknown as { getAutomationTree(): string }).getAutomationTree()
      ) as any
      const image = (function find(n: any): any {
        if (n.type === "img") return n
        for (const c of n.children ?? []) {
          const hit = find(c)
          if (hit) return hit
        }
        return null
      })(raw)
      expect(image.customProps.src).toBe(IMAGE_FIXTURE_PATH)
      expect(image.customProps.objectFit).toBe("cover")
    })
  })

  describe("screenshots", () => {
    it("should capture screenshot changes after image source is set", () => {
      function ImageScreenshotProbe() {
        const [loaded, setLoaded] = createSignal(false)

        return (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "100%",
              height: "100%",
              backgroundColor: "#0f111a",
            }}
          >
            <div
              style={{
                width: 420,
                height: 260,
                display: "flex",
                flexDirection: "column",
                gap: 12,
                padding: 18,
                borderRadius: 16,
                backgroundColor: "#1d2135",
              }}
              onClick={() => setLoaded(true)}
            >
              <Text style={{ color: "#b3bddf", fontSize: 13 }}>
                click panel to load image
              </Text>
              <img
                src={loaded() ? IMAGE_FIXTURE_PATH : ""}
                objectFit="cover"
                style={{ width: 300, height: 170, borderRadius: 12 }}
              />
            </div>
          </div>
        )
      }

      testRoot.render(() => <ImageScreenshotProbe />)

      const path0 = "/tmp/solo-img-0.png"
      const path1 = "/tmp/solo-img-1.png"

      if (fs.existsSync(path0)) fs.unlinkSync(path0)
      if (fs.existsSync(path1)) fs.unlinkSync(path1)

      testRoot.renderer.captureScreenshot(path0)
      testRoot.renderer.nativeSimulateClick!(640, 400)
      testRoot.renderer.captureScreenshot(path1)

      expect(fs.existsSync(path0)).toBe(true)
      expect(fs.existsSync(path1)).toBe(true)
      expect(fs.statSync(path0).size).toBeGreaterThan(0)
      expect(fs.statSync(path1).size).toBeGreaterThan(0)
      expect(fs.readFileSync(path0).equals(fs.readFileSync(path1))).toBe(false)
    })
  })
})
