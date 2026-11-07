/// End-to-end tests for the native GPUI text editor host elements.
/// macOS-only: requires the GPU-backed TestSoloRenderer.

import { createSignal } from "solid-js"
import { beforeEach, describe, expect, it } from "vitest"
import type { EventPayload } from "@solo/native"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import { Text, View } from "../components.js"

const describeNative = hasNativeTestRenderer ? describe : describe.skip

describeNative("native text editors", () => {
  let testRoot: SolidNativeTestRoot

  beforeEach(() => {
    testRoot = createSolidNativeTestRoot()
  })

  function TextInput(props: { readOnly?: boolean }) {
    const [text, setText] = createSignal(props.readOnly ? "locked" : "")
    return (
      <View style={{ width: 400, height: 100 }}>
        <input
          value={text()}
          placeholder="Type here..."
          readOnly={props.readOnly}
          style={{ width: 300, height: 40 }}
          onChange={(event: EventPayload) => setText(event.value ?? "")}
        />
        <Text>{`Value: ${text()}`}</Text>
      </View>
    )
  }

  it("edits text natively and emits the complete value", () => {
    testRoot.render(() => <TextInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "h i")

    expect(testRoot.getAllText()).toEqual(["Value: hi"])
    expect(testRoot.getPaintedText()).toContain("hi")
  })

  it("supports multiline textarea editing and submission", () => {
    function Textarea() {
      const [text, setText] = createSignal("")
      const [submits, setSubmits] = createSignal(0)
      return (
        <View style={{ width: 400, height: 160 }}>
          <textarea
            value={text()}
            placeholder="Write a message..."
            minRows={1}
            maxRows={4}
            style={{ width: 300 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
            onSubmit={() => {
              setSubmits((count) => count + 1)
            }}
          />
          <Text>{`Value: ${JSON.stringify(text())}`}</Text>
          <Text>{`Submits: ${submits()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <Textarea />)
    const textarea = testRoot.findByType("textarea")[0]!

    testRoot.renderer.nativeSimulateKeystrokes(textarea.id, "h i shift-enter t h e r e")
    expect(testRoot.getAllText()).toEqual([
      'Value: "hi\\nthere"',
      "Submits: 0",
    ])

    testRoot.renderer.nativeSimulateKeystrokes(textarea.id, "enter")
    expect(testRoot.getAllText()).toContain("Submits: 1")
  })

  it("deletes to the start of the line with cmd-backspace", () => {
    function Textarea() {
      const [text, setText] = createSignal("keep\nhello world")
      return (
        <View style={{ width: 400, height: 160 }}>
          <textarea
            value={text()}
            style={{ width: 300 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: ${JSON.stringify(text())}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <Textarea />)
    const textarea = testRoot.findByType("textarea")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(textarea.id, "cmd-backspace")

    expect(testRoot.getAllText()).toContain('Value: "keep\\n"')
  })

  it("deletes to the end of the line with cmd-delete", () => {
    function Textarea() {
      const [text, setText] = createSignal("keep\nhello world")
      return (
        <View style={{ width: 400, height: 160 }}>
          <textarea
            value={text()}
            style={{ width: 300 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: ${JSON.stringify(text())}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <Textarea />)
    const textarea = testRoot.findByType("textarea")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(textarea.id, "cmd-left cmd-delete")

    expect(testRoot.getAllText()).toContain('Value: "keep\\n"')
  })

  it("deletes one complete grapheme", () => {
    function GraphemeInput() {
      const [text, setText] = createSignal("A🙂")
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value={text()}
            style={{ width: 300, height: 40 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: ${text()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <GraphemeInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "backspace")

    expect(testRoot.getAllText()).toContain("Value: A")
  })

  it("moves the caret, replaces a selection, and undoes the edit", () => {
    function EditableInput() {
      const [text, setText] = createSignal("ac")
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value={text()}
            style={{ width: 300, height: 40 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: ${text()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <EditableInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "left b shift-left X")
    expect(testRoot.getAllText()).toContain("Value: aXc")

    testRoot.renderer.nativeSimulateKeystrokes(input.id, "cmd-z")
    expect(testRoot.getAllText()).toContain("Value: abc")
  })

  it("blocks editing when readOnly", () => {
    testRoot.render(() => <TextInput readOnly />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "backspace a")

    expect(testRoot.getAllText()).toContain("Value: locked")
  })

  it("forwards the caret theme to the native editor", () => {
    testRoot.render(() => (
      <input
        autoFocus
        value=""
        theme={{ caret: "#22c55e" }}
        style={{ width: 300, height: 40 }}
      />
    ))

    const input = testRoot.findByType("input")[0]!
    // Theme reaches Rust through setCustomProp; assert via the retained tree.
    const raw = JSON.parse(
      (testRoot.renderer as unknown as { getAutomationTree(): string }).getAutomationTree()
    ) as {
      customProps?: Record<string, unknown>
      children?: Array<{ customProps?: Record<string, unknown> }>[]
    }
    void raw
    void input
    // The automation tree carries customProps per node.
    const found = (function find(node: any): Record<string, unknown> | null {
      if (node.type === "input" && node.customProps?.theme) return node.customProps.theme
      for (const child of node.children ?? []) {
        const hit = find(child)
        if (hit) return hit
      }
      return null
    })(JSON.parse((testRoot.renderer as any).getAutomationTree()))
    expect(found).toEqual({ caret: "#22c55e" })
  })

  it("applies external value changes", () => {
    function ExternalValueInput() {
      const [text, setText] = createSignal("draft")
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value={text()}
            placeholder="Empty"
            style={{ width: 300, height: 40 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
            onSubmit={() => setText("")}
          />
          <Text>{`Value: ${text()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <ExternalValueInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "enter")

    expect(testRoot.getAllText()).toContain("Value: ")
    expect(testRoot.getPaintedText()).toContain("Empty")
  })

  it("focuses from a real mouse click", () => {
    function ClickFocusInput() {
      const [text, setText] = createSignal("")
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value={text()}
            style={{ width: 300, height: 40 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: ${text()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <ClickFocusInput />)
    const input = testRoot.findByType("input")[0]!
    const [x, y, width, height] = testRoot.getElementBounds(input.id)!
    testRoot.renderer.nativeSimulateClick(x + width / 2, y + height / 2)
    testRoot.renderer.simulateKeystrokes("a")

    expect(testRoot.getAllText()).toContain("Value: a")
  })

  it("keeps click and keyboard events available", () => {
    function EventedInput() {
      const [clicks, setClicks] = createSignal(0)
      const [keys, setKeys] = createSignal(0)
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value=""
            style={{ width: 300, height: 40 }}
            onClick={() => setClicks((count) => count + 1)}
            onKeyDown={() => setKeys((count) => count + 1)}
          />
          <Text>{`Events: ${clicks()}/${keys()}`}</Text>
        </View>
      )
    }

    testRoot.render(() => <EventedInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateClick(150, 20)
    testRoot.renderer.nativeSimulateKeyDown(input.id, "a")

    expect(testRoot.getAllText()).toContain("Events: 1/1")
  })
})
