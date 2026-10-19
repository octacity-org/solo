/// End-to-end event pipeline tests. Faithful port of packages/react
/// events.test.tsx ("events" describe) — macOS-only (GPU-backed renderer).

import fs from "fs"
import { createSignal } from "solid-js"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { EventPayload } from "@solo/native"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import { Text } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative("events", () => {
  let testRoot: SolidNativeTestRoot

  beforeEach(() => {
    testRoot = createSolidNativeTestRoot()
  })

  describe("click events", () => {
    it("should handle onClick and trigger re-render", () => {
      function Counter() {
        const [count, setCount] = createSignal(0)
        return (
          <div style={{ width: 200, height: 50 }} onClick={() => setCount((c) => c + 1)}>
            <Text>{`Count: ${count()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <Counter />)
      expect(testRoot.getAllText()).toEqual(["Count: 0"])

      testRoot.renderer.nativeSimulateClick(10, 10)
      expect(testRoot.getAllText()).toEqual(["Count: 1"])

      testRoot.renderer.nativeSimulateClick(10, 10)
      expect(testRoot.getAllText()).toEqual(["Count: 2"])
    })
  })

  describe("keyboard events", () => {
    it("should handle onKeyDown and update state", () => {
      function KeyTracker() {
        const [lastKey, setLastKey] = createSignal("none")
        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => setLastKey(e.key ?? "unknown")}
          >
            <Text>{`Key: ${lastKey()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <KeyTracker />)
      expect(testRoot.getAllText()).toEqual(["Key: none"])

      const div = testRoot.findByType("div").find((d) => d.events?.has("keyDown"))!
      testRoot.renderer.nativeSimulateKeystrokes(div.id, "down")
      expect(testRoot.getAllText()).toEqual(["Key: down"])

      testRoot.renderer.nativeSimulateKeystrokes(div.id, "escape")
      expect(testRoot.getAllText()).toEqual(["Key: escape"])
    })

    it("should pass modifiers in keyboard events", () => {
      const receivedEvents: EventPayload[] = []

      function ModifierTracker() {
        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => receivedEvents.push(e)}
          />
        )
      }

      testRoot.render(() => <ModifierTracker />)
      const div = testRoot.findByType("div").find((d) => d.events?.has("keyDown"))!

      testRoot.renderer.nativeSimulateKeystrokes(div.id, "cmd-s")

      expect(receivedEvents.length).toBeGreaterThanOrEqual(1)
      const event = receivedEvents.find((e) => e.key === "s")
      expect(event).toBeDefined()
      expect(event!.modifiers?.cmd).toBe(true)
    })
  })

  describe("hover events", () => {
    it("should handle mouseEnter and mouseLeave via mouse move", () => {
      function HoverBox() {
        const [hovered, setHovered] = createSignal(false)
        return (
          <div
            style={{ width: 200, height: 100 }}
            onMouseEnter={() => setHovered(true)}
            onMouseLeave={() => setHovered(false)}
          >
            <Text>{hovered() ? "hovered" : "not hovered"}</Text>
          </div>
        )
      }

      testRoot.render(() => <HoverBox />)
      expect(testRoot.getAllText()).toEqual(["not hovered"])

      testRoot.renderer.nativeSimulateMouseMove!(50, 50)
      expect(testRoot.getAllText()).toEqual(["hovered"])

      testRoot.renderer.nativeSimulateMouseMove!(500, 500)
      expect(testRoot.getAllText()).toEqual(["not hovered"])
    })
  })

  describe("mouseDownOutside", () => {
    it("should handle click outside to close pattern", () => {
      function Dropdown() {
        const [open, setOpen] = createSignal(false)
        return (
          <div style={{ width: 400, height: 400 }}>
            <div style={{ width: 100, height: 30 }} onClick={() => setOpen(true)}>
              <Text>trigger</Text>
            </div>
            {open() && (
              <div style={{ width: 100, height: 100 }} onMouseDownOutside={() => setOpen(false)}>
                <Text>dropdown content</Text>
              </div>
            )}
          </div>
        )
      }

      testRoot.render(() => <Dropdown />)
      expect(testRoot.getAllText()).toEqual(["trigger"])

      testRoot.renderer.nativeSimulateClick(10, 10)
      expect(testRoot.getAllText()).toEqual(["trigger", "dropdown content"])

      testRoot.renderer.nativeSimulateClick(350, 350)
      expect(testRoot.getAllText()).toEqual(["trigger"])
    })
  })

  describe("dialog overlay", () => {
    it("should open a tooltip-like dialog on button click and close on outside click", () => {
      function DialogDemo() {
        const [open, setOpen] = createSignal(false)
        return (
          <div style={{ width: 420, height: 260, position: "relative" }}>
            <div
              style={{
                width: 120,
                height: 32,
                marginTop: 16,
                marginLeft: 16,
                borderRadius: 8,
                backgroundColor: "#2f4ea3",
              }}
              onClick={() => setOpen(true)}
            >
              <Text>Open dialog</Text>
            </div>
            {open() && (
              <div
                style={{
                  position: "absolute",
                  top: 140,
                  left: 220,
                  width: 170,
                  height: 90,
                  padding: 10,
                  gap: 6,
                  borderRadius: 10,
                  borderWidth: 1,
                  borderColor: "#3d4660",
                  backgroundColor: "#1c2233",
                }}
                onMouseDownOutside={() => setOpen(false)}
              >
                <Text>Tooltip Dialog</Text>
                <Text>Some content inside</Text>
              </div>
            )}
          </div>
        )
      }

      testRoot.render(() => <DialogDemo />)
      expect(testRoot.getAllText()).toEqual(["Open dialog"])

      testRoot.renderer.nativeSimulateClick(20, 20)
      expect(testRoot.getAllText()).toEqual(["Open dialog", "Tooltip Dialog", "Some content inside"])

      testRoot.renderer.nativeSimulateClick(260, 170)
      expect(testRoot.getAllText()).toEqual(["Open dialog", "Tooltip Dialog", "Some content inside"])

      testRoot.renderer.nativeSimulateClick(40, 220)
      expect(testRoot.getAllText()).toEqual(["Open dialog"])
    })

    it("should capture screenshot changes when the dialog opens", () => {
      function DialogScreenshotProbe() {
        const [open, setOpen] = createSignal(false)
        return (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "100%",
              height: "100%",
              backgroundColor: "#0f1320",
            }}
          >
            <div
              style={{
                width: 460,
                height: 260,
                position: "relative",
                borderRadius: 18,
                backgroundColor: "#1a2238",
                padding: 20,
              }}
              onClick={() => setOpen(true)}
            >
              <div style={{ width: 148, height: 36, borderRadius: 10, backgroundColor: "#3a5ecf" }}>
                <Text>Open dialog</Text>
              </div>
              {open() && (
                <div
                  style={{
                    position: "absolute",
                    top: 84,
                    left: 188,
                    width: 236,
                    height: 130,
                    padding: 12,
                    gap: 8,
                    borderRadius: 12,
                    borderWidth: 1,
                    borderColor: "#4a5678",
                    backgroundColor: "#0d172b",
                  }}
                >
                  <Text>Tooltip Dialog</Text>
                  <Text>Visual screenshot probe</Text>
                </div>
              )}
            </div>
          </div>
        )
      }

      testRoot.render(() => <DialogScreenshotProbe />)

      const path0 = "/tmp/solo-dialog-0.png"
      const path1 = "/tmp/solo-dialog-1.png"

      if (fs.existsSync(path0)) fs.unlinkSync(path0)
      if (fs.existsSync(path1)) fs.unlinkSync(path1)

      testRoot.renderer.captureScreenshot(path0)
      testRoot.renderer.nativeSimulateClick(640, 400)
      testRoot.renderer.captureScreenshot(path1)

      expect(fs.existsSync(path0)).toBe(true)
      expect(fs.existsSync(path1)).toBe(true)
      expect(fs.statSync(path0).size).toBeGreaterThan(0)
      expect(fs.statSync(path1).size).toBeGreaterThan(0)
      expect(fs.readFileSync(path0).equals(fs.readFileSync(path1))).toBe(false)
    })

    it("should support anchored deferred dialog overlays", () => {
      function AnchoredDialogDemo() {
        const [open, setOpen] = createSignal(false)
        return (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "100%",
              height: "100%",
              backgroundColor: "#0c1020",
            }}
          >
            <div
              style={{
                width: 320,
                height: 180,
                borderRadius: 14,
                backgroundColor: "#1e2b4f",
                padding: 16,
              }}
              onClick={() => setOpen(true)}
            >
              <Text>Open anchored</Text>
              {open() && (
                <anchored position={{ x: 700, y: 360 }} anchor="topLeft" deferred priority={1}>
                  <div
                    style={{
                      width: 190,
                      height: 96,
                      padding: 10,
                      gap: 6,
                      borderRadius: 12,
                      borderWidth: 1,
                      borderColor: "#4f5b82",
                      backgroundColor: "#131c34",
                    }}
                    onMouseDownOutside={() => setOpen(false)}
                  >
                    <Text>Anchored Dialog</Text>
                    <Text>Deferred popover layer</Text>
                  </div>
                </anchored>
              )}
            </div>
          </div>
        )
      }

      testRoot.render(() => <AnchoredDialogDemo />)
      expect(testRoot.getAllText()).toEqual(["Open anchored"])

      testRoot.renderer.nativeSimulateClick(640, 400)
      expect(testRoot.getAllText()).toEqual(["Open anchored", "Anchored Dialog", "Deferred popover layer"])

      testRoot.renderer.nativeSimulateClick(730, 390)
      expect(testRoot.getAllText()).toEqual(["Open anchored", "Anchored Dialog", "Deferred popover layer"])

      testRoot.renderer.nativeSimulateClick(80, 80)
      expect(testRoot.getAllText()).toEqual(["Open anchored"])
    })
  })

  describe("keyboard navigation", () => {
    it("should support arrow key navigation in a list", () => {
      function SelectableList() {
        const items = ["Apple", "Banana", "Cherry"]
        const [selected, setSelected] = createSignal(0)

        return (
          <div
            style={{ width: 200, height: 200 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => {
              if (e.key === "down") setSelected((s) => Math.min(s + 1, items.length - 1))
              else if (e.key === "up") setSelected((s) => Math.max(s - 1, 0))
            }}
          >
            {items.map((item, i) => (
              <div>
                <Text>{`${i === selected() ? "> " : "  "}${item}`}</Text>
              </div>
            ))}
          </div>
        )
      }

      testRoot.render(() => <SelectableList />)
      expect(testRoot.getAllText()).toEqual(["> Apple", "  Banana", "  Cherry"])

      const list = testRoot.findByType("div").find((d) => d.events?.has("keyDown"))!

      testRoot.renderer.nativeSimulateKeystrokes(list.id, "down")
      expect(testRoot.getAllText()).toEqual(["  Apple", "> Banana", "  Cherry"])

      testRoot.renderer.nativeSimulateKeystrokes(list.id, "down")
      expect(testRoot.getAllText()).toEqual(["  Apple", "  Banana", "> Cherry"])

      testRoot.renderer.nativeSimulateKeystrokes(list.id, "down")
      expect(testRoot.getAllText()).toEqual(["  Apple", "  Banana", "> Cherry"])

      testRoot.renderer.nativeSimulateKeystrokes(list.id, "up")
      expect(testRoot.getAllText()).toEqual(["  Apple", "> Banana", "  Cherry"])
    })
  })

  describe("scroll events", () => {
    it("should handle onScroll and receive exact delta values", () => {
      const receivedEvents: EventPayload[] = []

      function ScrollBox() {
        return (
          <div style={{ width: 200, height: 200 }} onScroll={(e: EventPayload) => receivedEvents.push(e)}>
            <Text>scrollable</Text>
          </div>
        )
      }

      testRoot.render(() => <ScrollBox />)
      testRoot.renderer.nativeSimulateScrollWheel!(100, 100, 0, -50)

      expect(receivedEvents.length).toBeGreaterThanOrEqual(1)
      const scrollEvent = receivedEvents.find((e) => e.eventType === "scroll")
      expect(scrollEvent).toBeDefined()
      expect(scrollEvent!.eventType).toBe("scroll")
      expect(scrollEvent!.deltaX).toBe(0)
      expect(scrollEvent!.deltaY).toBe(-50)
      expect(scrollEvent!.touchPhase).toBe("moved")
    })

    it("should update state on scroll", () => {
      function ScrollCounter() {
        const [scrollCount, setScrollCount] = createSignal(0)
        return (
          <div style={{ width: 200, height: 200 }} onScroll={() => setScrollCount((c) => c + 1)}>
            <Text>{`Scrolls: ${scrollCount()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <ScrollCounter />)
      expect(testRoot.getAllText()).toEqual(["Scrolls: 0"])

      testRoot.renderer.nativeSimulateScrollWheel!(100, 100, 0, -30)
      expect(testRoot.getAllText()).toEqual(["Scrolls: 1"])
    })
  })

  describe("keyDown and keyUp events", () => {
    it("should handle onKeyDown via nativeSimulateKeyDown", () => {
      function KeyTracker() {
        const [lastKey, setLastKey] = createSignal("none")
        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => setLastKey(e.key ?? "unknown")}
          >
            <Text>{`Key: ${lastKey()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <KeyTracker />)
      const div = testRoot.findByType("div").find((d) => d.events?.has("keyDown"))!

      testRoot.renderer.nativeSimulateKeyDown!(div.id, "a")

      expect(testRoot.getAllText()).toEqual(["Key: a"])
    })

    it("should handle onKeyUp via nativeSimulateKeyUp", () => {
      const events: string[] = []

      function KeyUpTracker() {
        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => events.push(`down:${e.key}`)}
            onKeyUp={(e: EventPayload) => events.push(`up:${e.key}`)}
          />
        )
      }

      testRoot.render(() => <KeyUpTracker />)
      const div = testRoot.findByType("div").find((d) => d.events?.has("keyDown") && d.events?.has("keyUp"))!

      testRoot.renderer.nativeSimulateKeyDown!(div.id, "enter")
      testRoot.renderer.nativeSimulateKeyUp!(div.id, "enter")

      expect(events).toContain("down:enter")
      expect(events).toContain("up:enter")
    })

    it("should handle onKeyUp state update", () => {
      function KeyUpStateTracker() {
        const [lastKey, setLastKey] = createSignal("none")
        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyUp={(e: EventPayload) => setLastKey(e.key ?? "unknown")}
          >
            <Text>{`Released: ${lastKey()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <KeyUpStateTracker />)
      expect(testRoot.getAllText()).toEqual(["Released: none"])

      const div = testRoot.findByType("div").find((d) => d.events?.has("keyUp"))!

      testRoot.renderer.nativeSimulateKeyUp!(div.id, "a")
      expect(testRoot.getAllText()).toEqual(["Released: a"])
    })
  })

  describe("mouseDown and mouseUp events", () => {
    it("should handle onMouseDown and onMouseUp", () => {
      function PressTracker() {
        const [pressed, setPressed] = createSignal(false)
        return (
          <div
            style={{ width: 200, height: 100 }}
            onMouseDown={() => setPressed(true)}
            onMouseUp={() => setPressed(false)}
          >
            <Text>{pressed() ? "pressed" : "released"}</Text>
          </div>
        )
      }

      testRoot.render(() => <PressTracker />)
      expect(testRoot.getAllText()).toEqual(["released"])

      testRoot.renderer.nativeSimulateMouseDown!(10, 10)
      expect(testRoot.getAllText()).toEqual(["pressed"])

      testRoot.renderer.nativeSimulateMouseUp!(10, 10)
      expect(testRoot.getAllText()).toEqual(["released"])
    })

    it("should receive correct mouse button in mouseDown payload", () => {
      const receivedEvents: EventPayload[] = []

      function ButtonTracker() {
        return (
          <div style={{ width: 200, height: 100 }} onMouseDown={(e: EventPayload) => receivedEvents.push(e)} />
        )
      }

      testRoot.render(() => <ButtonTracker />)

      testRoot.renderer.nativeSimulateMouseDown!(10, 10, 0)
      expect(receivedEvents[0].button).toBe(0)

      testRoot.renderer.nativeSimulateMouseDown!(10, 10, 2)
      expect(receivedEvents[1].button).toBe(2)

      testRoot.renderer.nativeSimulateMouseDown!(10, 10, 1)
      expect(receivedEvents[2].button).toBe(1)
    })
  })

  describe("mouseMove events", () => {
    it("should handle onMouseMove and receive exact position", () => {
      const receivedEvents: EventPayload[] = []

      function MoveTracker() {
        return (
          <div style={{ width: 300, height: 300 }} onMouseMove={(e: EventPayload) => receivedEvents.push(e)} />
        )
      }

      testRoot.render(() => <MoveTracker />)
      testRoot.renderer.nativeSimulateMouseMove!(50, 75)

      expect(receivedEvents.length).toBeGreaterThanOrEqual(1)
      const moveEvent = receivedEvents.find((e) => e.eventType === "mouseMove")
      expect(moveEvent).toBeDefined()
      expect(moveEvent!.eventType).toBe("mouseMove")
      expect(moveEvent!.x).toBe(50)
      expect(moveEvent!.y).toBe(75)
    })

    it("should receive pressedButton during drag", () => {
      const receivedEvents: EventPayload[] = []

      function DragTracker() {
        return (
          <div style={{ width: 300, height: 300 }} onMouseMove={(e: EventPayload) => receivedEvents.push(e)} />
        )
      }

      testRoot.render(() => <DragTracker />)

      testRoot.renderer.nativeSimulateMouseMove!(10, 10)
      expect(receivedEvents.length).toBeGreaterThanOrEqual(1)
      const noButtonEvent = receivedEvents.find((e) => e.eventType === "mouseMove")!
      expect(noButtonEvent.pressedButton).toBeUndefined()

      receivedEvents.length = 0
      testRoot.renderer.nativeSimulateMouseMove!(50, 50, 0)
      const dragEvent = receivedEvents.find((e) => e.eventType === "mouseMove")!
      expect(dragEvent.pressedButton).toBe(0)
    })

    it("should update state on mouse move", () => {
      function PositionTracker() {
        const [pos, setPos] = createSignal("0,0")
        return (
          <div
            style={{ width: 300, height: 300 }}
            onMouseMove={(e: EventPayload) => setPos(`${Math.round(e.x ?? 0)},${Math.round(e.y ?? 0)}`)}
          >
            <Text>{`Position: ${pos()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <PositionTracker />)
      expect(testRoot.getAllText()).toEqual(["Position: 0,0"])

      testRoot.renderer.nativeSimulateMouseMove!(42, 99)
      expect(testRoot.getAllText()).toEqual(["Position: 42,99"])
    })
  })

  describe("combined event interactions", () => {
    it("should support keyboard shortcuts with modifiers", () => {
      function ShortcutHandler() {
        const [action, setAction] = createSignal("none")

        return (
          <div
            style={{ width: 200, height: 50 }}
            tabIndex={0}
            onKeyDown={(e: EventPayload) => {
              const mods = e.modifiers
              if (mods?.cmd && e.key === "s") {
                setAction("save")
              } else if (mods?.cmd && mods?.shift && e.key === "p") {
                setAction("command-palette")
              } else if (e.key === "escape") {
                setAction("cancel")
              }
            }}
          >
            <Text>{`Action: ${action()}`}</Text>
          </div>
        )
      }

      testRoot.render(() => <ShortcutHandler />)
      expect(testRoot.getAllText()).toEqual(["Action: none"])

      const div = testRoot.findByType("div").find((d) => d.events?.has("keyDown"))!

      testRoot.renderer.nativeSimulateKeystrokes(div.id, "cmd-s")
      expect(testRoot.getAllText()).toEqual(["Action: save"])

      testRoot.renderer.nativeSimulateKeystrokes(div.id, "cmd-shift-p")
      expect(testRoot.getAllText()).toEqual(["Action: command-palette"])

      testRoot.renderer.nativeSimulateKeystrokes(div.id, "escape")
      expect(testRoot.getAllText()).toEqual(["Action: cancel"])
    })
  })

  describe("element tree", () => {
    it("should produce correct element tree", () => {
      function App() {
        return (
          <div style={{ display: "flex", gap: 8 }}>
            <Text>Hello</Text>
            <div onClick={() => {}}>
              <Text>Click me</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <App />)

      // Structure assertion (ids are runtime-assigned; shape and content are
      // what matters across runtimes).
      const raw = JSON.parse(
        (testRoot.renderer as unknown as { getTreeJson(): string }).getTreeJson()
      ) as any
      const strip = (n: any): any => ({
        type: n.type,
        text: n.text,
        events: n.events,
        style: n.style,
        children: (n.children ?? []).map(strip).filter((c: any) => c.type !== "canvas"),
      })
      const root = strip(raw)
      // Root wrapper > retained root > two children.
      const retained = root.children![0].children!
      expect(retained.map((c: any) => c.type)).toEqual(["text", "div"])
      expect(retained[0].children![0]).toMatchObject({ type: "text", text: "Hello" })
      expect(retained[1].events).toEqual(["click"])
      expect(retained[1].children![0].children![0]).toMatchObject({ type: "text", text: "Click me" })
    })
  })

  describe("scrollable containers", () => {
    it("should scroll content when overflow is scroll", () => {
      function ScrollableList() {
        return (
          <div style={{ width: 300, height: 200, overflow: "scroll" }}>
            {["#ff0000", "#00ff00", "#0000ff", "#ffff00", "#ff00ff"].map((bg, i) => (
              <div style={{ height: 100, backgroundColor: bg }}>
                <Text>{`Item ${i + 1}`}</Text>
              </div>
            ))}
          </div>
        )
      }

      testRoot.render(() => <ScrollableList />)

      const scrollContainer = testRoot.findByType("div").find((d) => d.style?.overflow === "scroll")!
      expect(scrollContainer).toBeDefined()

      const initialOffset = testRoot.renderer.getScrollOffset(scrollContainer.id)
      expect(initialOffset).toEqual([0, 0])

      testRoot.renderer.nativeSimulateScrollWheel!(150, 100, 0, -50)

      const afterScrollOffset = testRoot.renderer.getScrollOffset(scrollContainer.id)
      expect(afterScrollOffset).not.toBeNull()
      expect(afterScrollOffset![1]).toBeLessThan(0)
    })

    it("does not remap a vertical wheel onto overflow-x", () => {
      function NestedAxisScroll() {
        return (
          <div style={{ width: 240, height: 120, overflowY: "scroll" }}>
            <div style={{ width: 240, height: 80, overflowX: "scroll" }}>
              <div style={{ width: 800, height: 80 }}>
                <Text>wide row</Text>
              </div>
            </div>
            <div style={{ height: 400 }}>
              <Text>below</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <NestedAxisScroll />)

      const parent = testRoot.findByType("div").find((d) => d.style?.overflowY === "scroll")!
      const inner = testRoot.findByType("div").find((d) => d.style?.overflowX === "scroll")!

      expect(testRoot.renderer.getScrollOffset(parent.id)).toEqual([0, 0])
      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([0, 0])

      testRoot.renderer.nativeSimulateScrollWheel!(80, 40, 0, -60)

      const parentOffset = testRoot.renderer.getScrollOffset(parent.id)
      const innerOffset = testRoot.renderer.getScrollOffset(inner.id)
      expect(parentOffset).not.toBeNull()
      expect(parentOffset![1]).toBeLessThan(0)
      expect(innerOffset).toEqual([0, 0])
    })

    it("contains vertical wheel input inside a nested scroller at both boundaries", () => {
      function NestedScroll() {
        return (
          <div style={{ width: 240, height: 180, overflowY: "scroll" }}>
            <div
              style={{
                width: 240,
                height: 120,
                overflowY: "scroll",
                overscrollBehaviorY: "contain",
              }}
            >
              <div style={{ width: 240, height: 360 }}>
                <Text>inner content</Text>
              </div>
            </div>
            <div style={{ width: 240, height: 360 }}>
              <Text>outer content</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <NestedScroll />)

      const scrollers = testRoot.findByType("div").filter((d) => d.style?.overflowY === "scroll")
      const [outer, inner] = scrollers

      testRoot.renderer.scrollTo!(inner.id, 0, -240)
      testRoot.renderer.nativeSimulateScrollWheel!(80, 60, 0, -60)
      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([0, -240])
      expect(testRoot.renderer.getScrollOffset(outer.id)).toEqual([0, 0])

      testRoot.renderer.scrollTo!(outer.id, 0, -40)
      testRoot.renderer.scrollTo!(inner.id, 0, 0)
      testRoot.renderer.nativeSimulateScrollWheel!(80, 20, 0, 60)
      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([0, 0])
      expect(testRoot.renderer.getScrollOffset(outer.id)).toEqual([0, -40])
    })

    it("chains wheel input to an ancestor by default at an inner boundary", () => {
      function NestedScroll() {
        return (
          <div style={{ width: 240, height: 180, overflowY: "scroll" }}>
            <div style={{ width: 240, height: 120, overflowY: "scroll" }}>
              <div style={{ width: 240, height: 360 }}>
                <Text>inner content</Text>
              </div>
            </div>
            <div style={{ width: 240, height: 360 }}>
              <Text>outer content</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <NestedScroll />)

      const [outer, inner] = testRoot
        .findByType("div")
        .filter((d) => d.style?.overflowY === "scroll")
      testRoot.renderer.scrollTo!(inner.id, 0, -240)
      testRoot.renderer.nativeSimulateScrollWheel!(80, 60, 0, -60)

      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([0, -240])
      expect(testRoot.renderer.getScrollOffset(outer.id)![1]).toBeLessThan(0)
    })

    it("keeps the uncontained axis available to an ancestor scroller", () => {
      function NestedAxisScroll() {
        return (
          <div style={{ width: 240, height: 120, overflowY: "scroll" }}>
            <div
              style={{
                width: 240,
                height: 80,
                overflowX: "scroll",
                overscrollBehaviorX: "contain",
              }}
            >
              <div style={{ width: 800, height: 80 }}>
                <Text>wide row</Text>
              </div>
            </div>
            <div style={{ height: 400 }}>
              <Text>below</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <NestedAxisScroll />)

      const parent = testRoot.findByType("div").find((d) => d.style?.overflowY === "scroll")!
      const inner = testRoot.findByType("div").find((d) => d.style?.overflowX === "scroll")!
      testRoot.renderer.nativeSimulateScrollWheel!(80, 40, 0, -60)

      expect(testRoot.renderer.getScrollOffset(parent.id)![1]).toBeLessThan(0)
      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([0, 0])
    })

    it("contains horizontal wheel input inside a nested scroller at its boundary", () => {
      function NestedHorizontalScroll() {
        return (
          <div style={{ width: 300, height: 100, overflowX: "scroll" }}>
            <div
              style={{
                width: 180,
                height: 100,
                overflowX: "scroll",
                overscrollBehaviorX: "contain",
              }}
            >
              <div style={{ width: 500, height: 100 }}>
                <Text>inner wide row</Text>
              </div>
            </div>
            <div style={{ width: 400, height: 100 }}>
              <Text>outer wide row</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <NestedHorizontalScroll />)

      const [outer, inner] = testRoot
        .findByType("div")
        .filter((d) => d.style?.overflowX === "scroll")
      testRoot.renderer.scrollTo!(inner.id, -320, 0)
      testRoot.renderer.nativeSimulateScrollWheel!(80, 40, -60, 0)

      expect(testRoot.renderer.getScrollOffset(inner.id)).toEqual([-320, 0])
      expect(testRoot.renderer.getScrollOffset(outer.id)).toEqual([0, 0])
    })

    it("pans overflow-x when the child is wider than the viewport", () => {
      function WideRow() {
        return (
          <div style={{ width: 240, height: 80 }}>
            <div style={{ width: "100%", height: 80, overflowX: "scroll" }}>
              <div style={{ width: 800, height: 80, flexShrink: 0 }}>
                <Text>wide row</Text>
              </div>
            </div>
          </div>
        )
      }

      testRoot.render(() => <WideRow />)

      const scroller = testRoot.findByType("div").find((d) => d.style?.overflowX === "scroll")!
      expect(testRoot.renderer.getScrollOffset(scroller.id)).toEqual([0, 0])

      testRoot.renderer.nativeSimulateScrollWheel!(80, 40, -80, 0)
      const offset = testRoot.renderer.getScrollOffset(scroller.id)
      expect(offset).not.toBeNull()
      expect(offset![0]).toBeLessThan(0)
    })

    it("lets a parent scroller take a vertical wheel over a filled child", () => {
      function FilledColumn() {
        return (
          <div style={{ width: 240, height: 120, overflowY: "scroll" }}>
            <div style={{ height: 80, width: "100%", backgroundColor: "#1e1e2e" }}>
              <Text>card</Text>
            </div>
            <div style={{ height: 400 }}>
              <Text>below</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <FilledColumn />)
      const parent = testRoot.findByType("div").find((d) => d.style?.overflowY === "scroll")!
      testRoot.renderer.nativeSimulateScrollWheel!(80, 40, 0, -80)
      const offset = testRoot.renderer.getScrollOffset(parent.id)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBeLessThan(0)
    })

    it("should support overflow-y scroll only", () => {
      function VerticalScroll() {
        return (
          <div style={{ width: 300, height: 100, overflowY: "scroll" }}>
            <div style={{ height: 500 }}>
              <Text>Tall content</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <VerticalScroll />)

      const container = testRoot.findByType("div").find((d) => d.style?.overflowY === "scroll")!
      expect(container).toBeDefined()

      const initialOffset = testRoot.renderer.getScrollOffset(container.id)
      expect(initialOffset).toEqual([0, 0])

      testRoot.renderer.nativeSimulateScrollWheel!(150, 50, 0, -80)
      const offset = testRoot.renderer.getScrollOffset(container.id)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBeLessThan(0)
    })

    it("should support programmatic scrollTo", () => {
      function ScrollableBox() {
        return (
          <div style={{ width: 200, height: 100, overflow: "scroll" }}>
            <div style={{ height: 500 }}>
              <Text>Very tall content</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <ScrollableBox />)

      const container = testRoot.findByType("div").find((d) => d.style?.overflow === "scroll")!

      expect(testRoot.renderer.getScrollOffset(container.id)).toEqual([0, 0])

      testRoot.renderer.scrollTo!(container.id, 0, -100)

      const offset = testRoot.renderer.getScrollOffset(container.id)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBe(-100)
    })

    it("should support programmatic scrollToItem", () => {
      function ItemList() {
        return (
          <div style={{ display: "flex", flexDirection: "column", width: 200, height: 100, overflow: "scroll" }}>
            <div style={{ height: 80, flexShrink: 0 }}><Text>Item A</Text></div>
            <div style={{ height: 80, flexShrink: 0 }}><Text>Item B</Text></div>
            <div style={{ height: 80, flexShrink: 0 }}><Text>Item C</Text></div>
            <div style={{ height: 80, flexShrink: 0 }}><Text>Item D</Text></div>
          </div>
        )
      }

      testRoot.render(() => <ItemList />)

      const container = testRoot.findByType("div").find((d) => d.style?.overflow === "scroll")!

      expect(testRoot.renderer.getScrollOffset(container.id)).toEqual([0, 0])

      testRoot.renderer.scrollToItem!(container.id, 3)

      const offset = testRoot.renderer.getScrollOffset(container.id)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBeLessThan(0)
    })

    it("should render scrollable container with visible screenshot diff", () => {
      function ScreenshotScroller() {
        return (
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              width: "100%",
              height: "100%",
              backgroundColor: "#1a1a2e",
            }}
          >
            <div
              style={{
                width: 300,
                height: 200,
                overflow: "scroll",
                backgroundColor: "#16213e",
                borderRadius: 8,
                borderWidth: 2,
                borderColor: "#0f3460",
              }}
            >
              {[
                ["#e94560", "Section 1 (Red)"],
                ["#0f3460", "Section 2 (Blue)"],
                ["#533483", "Section 3 (Purple)"],
                ["#e94560", "Section 4 (Red)"],
                ["#0f3460", "Section 5 (Blue)"],
              ].map(([bg, label]) => (
                <div style={{ height: 80, backgroundColor: bg, padding: 16 }}>
                  <Text style={{ color: "#ffffff", fontSize: 20 }}>{label}</Text>
                </div>
              ))}
            </div>
          </div>
        )
      }

      testRoot.render(() => <ScreenshotScroller />)

      const path0 = "/tmp/solo-scroll-before.png"
      const path1 = "/tmp/solo-scroll-after.png"

      if (fs.existsSync(path0)) fs.unlinkSync(path0)
      if (fs.existsSync(path1)) fs.unlinkSync(path1)

      testRoot.renderer.captureScreenshot(path0)
      testRoot.renderer.nativeSimulateScrollWheel!(640, 400, 0, -150)
      testRoot.renderer.captureScreenshot(path1)

      expect(fs.existsSync(path0)).toBe(true)
      expect(fs.existsSync(path1)).toBe(true)
      expect(fs.statSync(path0).size).toBeGreaterThan(0)
      expect(fs.statSync(path1).size).toBeGreaterThan(0)
      expect(fs.readFileSync(path0).equals(fs.readFileSync(path1))).toBe(false)
    })

    it("should combine onScroll event with overflow scroll", () => {
      const receivedScrollEvents: EventPayload[] = []

      function ScrollWithEvent() {
        return (
          <div
            style={{ width: 300, height: 100, overflow: "scroll" }}
            onScroll={(e: EventPayload) => receivedScrollEvents.push(e)}
          >
            <div style={{ height: 500 }}>
              <Text>Scrollable with events</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <ScrollWithEvent />)

      testRoot.renderer.nativeSimulateScrollWheel!(150, 50, 0, -40)

      expect(receivedScrollEvents.length).toBeGreaterThanOrEqual(1)
      const scrollEvent = receivedScrollEvents.find((e) => e.eventType === "scroll")
      expect(scrollEvent).toBeDefined()
      expect(scrollEvent!.deltaY).toBe(-40)

      const container = testRoot.findByType("div").find((d) => d.style?.overflow === "scroll")!
      const offset = testRoot.renderer.getScrollOffset(container.id)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBeLessThan(0)
    })

    it("exposes the scrolled element for programmatic scroll via ref", () => {
      // Solid's ref compiles to universal.ref: the callback receives the
      // Solo node, whose numeric id drives the scroll API.
      let capturedId: number | null = null

      function RefScroller() {
        return (
          <div
            ref={(el: { id: number }) => {
              capturedId = el.id
            }}
            style={{ width: 200, height: 100, overflow: "scroll" }}
          >
            <div style={{ height: 80 }}>
              <Text>Item A</Text>
            </div>
            <div style={{ height: 80 }}>
              <Text>Item B</Text>
            </div>
          </div>
        )
      }

      testRoot.render(() => <RefScroller />)

      expect(capturedId).not.toBeNull()
      expect(typeof capturedId).toBe("number")
      expect(capturedId!).toBeGreaterThan(0)

      const elementId = capturedId!
      expect(testRoot.renderer.getScrollOffset(elementId)).toEqual([0, 0])

      testRoot.renderer.scrollTo!(elementId, 0, -60)
      const offset = testRoot.renderer.getScrollOffset(elementId)
      expect(offset).not.toBeNull()
      expect(offset![1]).toBe(-60)
    })
  })
})
