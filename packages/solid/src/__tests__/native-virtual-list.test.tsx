/// The native <virtual-list>: lazy rows, programmatic scrolling, and chat
/// tail following. Faithful port of packages/react virtual-list.test.tsx.

import { createSignal } from "solid-js"
import { describe, expect, it } from "vitest"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import { Text } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

function Rows({ count }: { count: number }) {
  return Array.from({ length: count }, (_, index) => (
    <div
      style={{
        display: "flex",
        height: 40,
        flexShrink: 0,
        alignItems: "center",
      }}
    >
      <Text>{`row-${index}`}</Text>
    </div>
  ))
}

function FocusableRows({ inputIndex = 0 }: { inputIndex?: number }) {
  const [value, setValue] = createSignal("")
  return (
    <virtual-list
      overdraw={0}
      estimatedItemHeight={40}
      style={{ width: 400, height: 160 }}
    >
      {Array.from({ length: 30 }, (_, index) => (
        <div style={{ height: 40, flexShrink: 0 }}>
          {index === inputIndex ? (
            <input
              autoFocus
              placeholder="focused-input"
              value={value()}
              onChange={(event) => setValue(event.value ?? "")}
            />
          ) : (
            <Text>{`row-${index}`}</Text>
          )}
        </div>
      ))}
    </virtual-list>
  )
}

function DynamicFocusableRows({ enabled }: { enabled: boolean }) {
  const [value, setValue] = createSignal("")
  return (
    <virtual-list
      overdraw={0}
      estimatedItemHeight={40}
      style={{ width: 400, height: 160 }}
    >
      {Array.from({ length: 30 }, (_, index) => (
        <div style={{ height: 40, flexShrink: 0 }}>
          {index === 0 && enabled ? (
            <input
              autoFocus
              value={value()}
              onChange={(event) => setValue(event.value ?? "")}
            />
          ) : (
            <Text>{`row-${index}`}</Text>
          )}
        </div>
      ))}
    </virtual-list>
  )
}


describeNative("<virtual-list>", () => {
  it("builds and paints only rows near the viewport", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={40}
        style={{ width: 400, height: 160 }}
      >
        {Rows({ count: 100 })}
      </virtual-list>
    ))

    expect(renderer.getAllText()).toHaveLength(100)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("row-0")
    expect(painted).not.toContain("row-99")
    expect(painted.length).toBeLessThan(10)
  })

  it("builds a distant row when it is scrolled into view", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={40}
        style={{ width: 400, height: 160 }}
      >
        {Rows({ count: 100 })}
      </virtual-list>
    ))

    const list = renderer.findByType("virtual-list")[0]
    renderer.scrollToItem(list.id, 99)
    expect(renderer.getScrollOffset(list.id)?.[1]).toBeLessThan(-100)

    const painted = renderer.getPaintedText()
    expect(painted).toContain("row-99")
    expect(painted).not.toContain("row-0")
  })

  it("keeps estimated heights for pixel scrolls after the first layout", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={40}
        style={{ width: 400, height: 160 }}
      >
        {Rows({ count: 100 })}
      </virtual-list>
    ))

    // GPUI drops size hints on its first layout; unmeasured rows must still
    // count at the estimated height, or the scroll clamps near the top.
    const list = renderer.findByType("virtual-list")[0]
    renderer.scrollTo(list.id, 0, -3000)
    expect(renderer.getScrollOffset(list.id)?.[1]).toBe(-3000)
    expect(renderer.getPaintedText()).toContain("row-75")
  })

  it("keeps estimated heights when existing rows become focusable", () => {
    const t = createSolidNativeTestRoot()
    const rows = (focusable: boolean) => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={40}
        style={{ width: 400, height: 160 }}
      >
        {Array.from({ length: 100 }, (_, index) => (
          <div style={{ height: 40, flexShrink: 0 }}>
            {focusable ? <input value={`row-${index}`} /> : <Text>{`row-${index}`}</Text>}
          </div>
        ))}
      </virtual-list>
    )
    t.render(() => rows(false))
    t.render(() => rows(true))

    const list = t.renderer.findByType("virtual-list")[0]
    t.renderer.scrollTo(list.id, 0, -3000)
    expect(t.renderer.getScrollOffset(list.id)?.[1]).toBe(-3000)
  })

  it("lazily builds custom elements inside rows", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={80}
        style={{ width: 400, height: 160 }}
      >
        {Array.from({ length: 30 }, (_, index) => (
          <div style={{ minHeight: 80, flexShrink: 0 }}>
            {index === 20 ? <markdown source="# Lazy markdown" /> : <Text>{`row-${index}`}</Text>}
          </div>
        ))}
      </virtual-list>
    ))

    expect(renderer.findByType("markdown")).toHaveLength(1)
    expect(renderer.getPaintedText()).not.toContain("Lazy markdown")

    const list = renderer.findByType("virtual-list")[0]
    renderer.scrollToItem(list.id, 20)
    expect(renderer.getPaintedText()).toContain("Lazy markdown")
  })

  it("keeps a focused row active when it scrolls offscreen", () => {
    const t = createSolidNativeTestRoot()
    t.render(() => <FocusableRows />)

    const input = t.renderer.findByType("input")[0]
    t.renderer.simulateKeystrokes("a")
    expect(t.getElement(input.id)?.customProps?.value).toBe("a")

    const list = t.renderer.findByType("virtual-list")[0]
    t.renderer.scrollToItem(list.id, 29)
    t.renderer.simulateKeystrokes("b")
    expect(t.getElement(input.id)?.customProps?.value).toBe("ab")
  })

  it("reveals an initially focused offscreen row", () => {
    const t = createSolidNativeTestRoot()
    t.render(() => <FocusableRows inputIndex={20} />)

    expect(t.renderer.getPaintedText()).toContain("focused-input")

    const input = t.renderer.findByType("input")[0]
    t.renderer.simulateKeystrokes("a")
    expect(t.getElement(input.id)?.customProps?.value).toBe("a")
  })

  it("updates focus retention when an existing row becomes focusable", () => {
    const t = createSolidNativeTestRoot()
    t.render(() => <DynamicFocusableRows enabled={false} />)
    t.render(() => <DynamicFocusableRows enabled />)

    const input = t.renderer.findByType("input")[0]
    const list = t.renderer.findByType("virtual-list")[0]
    t.renderer.scrollToItem(list.id, 29)
    t.renderer.simulateKeystrokes("a")

    expect(t.getElement(input.id)?.customProps?.value).toBe("a")
  })

  it("follows appended chat rows while tail following is active", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    const transcript = (count: number) => (
      <virtual-list
        alignment="bottom"
        followTail
        overdraw={0}
        estimatedItemHeight={40}
        style={{ width: 400, height: 160 }}
      >
        {Rows({ count })}
      </virtual-list>
    )

    render(() => transcript(20))
    expect(renderer.getPaintedText()).toContain("row-19")
    expect(renderer.getPaintedText()).not.toContain("row-0")

    render(() => transcript(21))
    expect(renderer.getPaintedText()).toContain("row-20")
    expect(renderer.getPaintedText()).not.toContain("row-0")
  })

  it("lets overflow-x inside a row pan without moving the list", () => {
    const { render, renderer } = createSolidNativeTestRoot()
    render(() => (
      <virtual-list
        overdraw={0}
        estimatedItemHeight={80}
        style={{ width: 240, height: 160 }}
      >
        <div style={{ width: "100%", height: 80, overflowX: "scroll" }}>
          <div style={{ width: 800, height: 80, flexShrink: 0 }}>
            <Text>wide row</Text>
          </div>
        </div>
        <div style={{ height: 80 }}>
          <Text>below</Text>
        </div>
      </virtual-list>
    ))

    const list = renderer.findByType("virtual-list")[0]
    const scroller = renderer
      .findByType("div")
      .find((d) => d.style?.overflowX === "scroll")!
    expect(renderer.getScrollOffset(scroller.id)?.[0] ?? 0).toBe(0)

    renderer.nativeSimulateScrollWheel(80, 40, -80, 0)
    const listOffset = renderer.getScrollOffset(list.id)
    const rowOffset = renderer.getScrollOffset(scroller.id)
    expect(listOffset?.[1] ?? 0, `list ${JSON.stringify(listOffset)}`).toBeCloseTo(0)
    expect(rowOffset?.[0], `row ${JSON.stringify(rowOffset)}`).toBeLessThan(0)
  })
})
