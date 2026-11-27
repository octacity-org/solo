/// End-to-end tests for the native GPUI text editor host elements.
/// macOS-only: requires the GPU-backed TestSoloRenderer.

import { createSignal } from "solid-js"
import { beforeEach, describe, expect, it } from "vitest"
import type { EventPayload } from "@solo/native"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"
import type { SolidNativeTestRoot } from "../testing.js"
import type { TextareaController } from "../textarea.js"
import { Text, View } from "../components.js"
import path from "path"
import { readPng, SHOTS_DIR } from "./test-utils.js"

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

  it("copies and pastes inside an input through GPUI's clipboard", () => {
    testRoot.render(() => <TextInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "h i cmd-a cmd-c right cmd-v")

    expect(testRoot.getAllText()).toEqual(["Value: hihi"])
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

  it("undoes a run of typing as one step and redoes it", () => {
    function EditableInput() {
      const [text, setText] = createSignal("")
      return (
        <View style={{ width: 400, height: 100 }}>
          <input
            value={text()}
            style={{ width: 300, height: 40 }}
            onChange={(event: EventPayload) => setText(event.value ?? "")}
          />
          <Text>{`Value: [${text()}]`}</Text>
        </View>
      )
    }

    testRoot.render(() => <EditableInput />)
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "a b c left d")
    expect(testRoot.getAllText()).toContain("Value: [abdc]")

    // Moving the caret closed the "abc" run, so "d" undoes on its own.
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "cmd-z")
    expect(testRoot.getAllText()).toContain("Value: [abc]")
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "cmd-z")
    expect(testRoot.getAllText()).toContain("Value: []")

    testRoot.renderer.nativeSimulateKeystrokes(input.id, "cmd-shift-z")
    expect(testRoot.getAllText()).toContain("Value: [abc]")
  })

  it("reports edits that rebuild the text from the previous value", () => {
    const changes: EventPayload[] = []
    testRoot.render(() => (
      <textarea
        value="select 1"
        style={{ width: 300, height: 80 }}
        onChange={(event: EventPayload) => changes.push(event)}
      />
    ))
    const textarea = testRoot.findByType("textarea")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(
      textarea.id,
      "x y shift-left shift-left 🙂 shift-enter left backspace cmd-z cmd-shift-z"
    )

    expect(changes.map((event) => event.origin)).toEqual([
      "user", "user", "user", "user", "user", "undo", "redo",
    ])
    let text = "select 1"
    let revision = 0
    for (const event of changes) {
      for (const edit of event.edits ?? []) {
        text = text.slice(0, edit.start) + edit.text + text.slice(edit.end)
      }
      expect(text).toBe(event.value)
      expect(event.revision!).toBeGreaterThan(revision)
      revision = event.revision!
    }
    expect(text).toBe("select 1\n")
    expect(changes[2]!.edits).toEqual([{ start: 8, end: 10, text: "🙂" }])
  })

  it("reports selection moves in UTF-16 offsets", () => {
    const selections: EventPayload[] = []
    testRoot.render(() => (
      <input
        value="a🙂b"
        style={{ width: 300, height: 40 }}
        onSelectionChange={(event: EventPayload) => selections.push(event)}
      />
    ))
    const input = testRoot.findByType("input")[0]!
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "left")
    testRoot.renderer.nativeSimulateKeystrokes(input.id, "left left shift-right")

    // The test window draws after each keystroke, so each move is a frame.
    expect(selections.map((event) => event.selections)).toEqual([
      [{ anchor: 3, head: 3 }],
      [{ anchor: 1, head: 1 }],
      [{ anchor: 0, head: 0 }],
      [{ anchor: 0, head: 1 }],
    ])
    const [x, y] = testRoot.getElementBounds(input.id)!
    const [beforeB, beforeEmoji, start, selected] = selections.map((event) => event.caret!)
    expect(start!.x).toBeCloseTo(x!, 0)
    expect(beforeEmoji!.x).toBeGreaterThan(start!.x)
    expect(beforeB!.x).toBeGreaterThan(beforeEmoji!.x)
    expect(selected!.x).toBeCloseTo(beforeEmoji!.x)
    expect(start!.y).toBeGreaterThanOrEqual(y!)
    expect(start!.height).toBeGreaterThan(0)

    // A frame without a move reports nothing.
    testRoot.getPaintedText()
    expect(selections).toHaveLength(4)
  })

  describe("controller", () => {
    function mountEditor(value: string, maxRows?: number) {
      let controller!: TextareaController
      const changes: EventPayload[] = []
      const selections: EventPayload[] = []
      testRoot.render(() => (
        <textarea
          ref={(c: TextareaController) => (controller = c)}
          value={value}
          maxRows={maxRows}
          style={{ width: 300 }}
          onChange={(event: EventPayload) => changes.push(event)}
          onSelectionChange={(event: EventPayload) => selections.push(event)}
        />
      ))
      const textarea = testRoot.findByType("textarea")[0]!
      return { controller, changes, selections, id: textarea.id }
    }

    it("reads the text, selections and revision", () => {
      const { controller, id } = mountEditor("a🙂b")
      expect(controller.getText()).toBe("a🙂b")
      expect(controller.getSelections()).toEqual([{ anchor: 4, head: 4 }])
      const revision = controller.revision()

      testRoot.renderer.nativeSimulateKeystrokes(id, "left shift-left")
      expect(controller.getSelections()).toEqual([{ anchor: 3, head: 1 }])
      expect(controller.revision()).toBe(revision)
      testRoot.renderer.nativeSimulateKeystrokes(id, "x")
      expect(controller.getText()).toBe("axb")
      expect(controller.revision()).toBeGreaterThan(revision)
    })

    it("applies edits at the current revision as one undo step", () => {
      const { controller, changes, id } = mountEditor("select 1")
      const result = controller.applyEdits(controller.revision(), [
        { start: 0, end: 6, text: "SELECT" },
        { start: 7, end: 8, text: "2" },
      ])

      expect(result).toEqual({ applied: true, revision: controller.revision() })
      expect(controller.getText()).toBe("SELECT 2")
      expect(changes.at(-1)!.origin).toBe("api")
      expect(changes.at(-1)!.value).toBe("SELECT 2")
      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-z")
      expect(controller.getText()).toBe("select 1")
    })

    it("rebases edits made against an older revision", () => {
      const { controller, id } = mountEditor("select 1")
      const revision = controller.revision()
      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-left x y")

      const result = controller.applyEdits(revision, [{ start: 7, end: 8, text: "2" }])
      expect(result.applied).toBe(true)
      expect(controller.getText()).toBe("xyselect 2")
    })

    it("refuses edits whose range changed since their revision", () => {
      const { controller, id } = mountEditor("select 1")
      const revision = controller.revision()
      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-left right right x")

      const result = controller.applyEdits(revision, [{ start: 0, end: 6, text: "SELECT" }])
      expect(result).toEqual({
        applied: false,
        reason: "conflict",
        revision: controller.revision(),
      })
      expect(controller.getText()).toBe("sexlect 1")
    })

    it("refuses edits from a revision the log no longer holds", () => {
      const { controller, id } = mountEditor("")
      const revision = controller.revision()
      testRoot.renderer.nativeSimulateKeystrokes(id, Array(520).fill("a").join(" "))

      const result = controller.applyEdits(revision, [{ start: 0, end: 0, text: "x" }])
      expect(result.applied).toBe(false)
      expect(result.applied === false && result.reason).toBe("expired")
    })

    it("keeps the undo history across edits that are not undoable", () => {
      const { controller, id } = mountEditor("select 1")
      testRoot.renderer.nativeSimulateKeystrokes(id, "x")
      controller.applyEdits(controller.revision(), [{ start: 0, end: 0, text: "-- " }], {
        undoable: false,
      })
      expect(controller.getText()).toBe("-- select 1x")

      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-z")
      expect(controller.getText()).toBe("-- select 1")
    })

    it("rejects malformed edits", () => {
      const { controller } = mountEditor("select 1")
      const revision = controller.revision()
      expect(() =>
        controller.applyEdits(revision, [
          { start: 0, end: 4, text: "" },
          { start: 2, end: 6, text: "" },
        ])
      ).toThrow(/overlap/)
      expect(() =>
        controller.applyEdits(revision, [{ start: 0, end: 99, text: "" }])
      ).toThrow()
      expect(controller.getText()).toBe("select 1")
    })

    it("sets the selection and reveals it on request", () => {
      const lines = Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n")
      const { controller, selections, id } = mountEditor(lines, 3)
      const [, top, , height] = testRoot.getElementBounds(id)!

      controller.setSelections([{ anchor: 0, head: 4 }], { reveal: true })
      testRoot.getPaintedText()
      expect(controller.getSelections()).toEqual([{ anchor: 0, head: 4 }])
      expect(selections.at(-1)!.selections).toEqual([{ anchor: 0, head: 4 }])
      expect(selections.at(-1)!.caret!.y).toBeGreaterThanOrEqual(top!)

      // Without `reveal` the view stays where it is.
      const end = lines.length
      controller.setSelections([{ anchor: end, head: end }])
      testRoot.getPaintedText()
      expect(selections.at(-1)!.caret!.y).toBeGreaterThan(top! + height!)

      controller.setSelections([{ anchor: end - 1, head: end - 1 }], { reveal: true })
      testRoot.getPaintedText()
      const caret = selections.at(-1)!.caret!
      expect(selections.at(-1)!.selections).toEqual([{ anchor: end - 1, head: end - 1 }])
      expect(caret.y).toBeGreaterThanOrEqual(top!)
      expect(caret.y + caret.height).toBeLessThanOrEqual(top! + height! + 1)
    })
  })

  describe("syntax highlighting", () => {
    // Red pixels inside the editor: only a keyword painted red makes them.
    function redPixels(name: string, language?: string) {
      testRoot.render(() => (
        <View style={{ width: "100%", height: "100%", backgroundColor: "#000000" }}>
          <textarea
            value={"SELECT name\nFROM users"}
            language={language}
            theme={{ syntax: { keyword: "#ff0000" } }}
            style={{ width: 300, color: "#ffffff" }}
          />
        </View>
      ))
      // Highlighting runs after a short debounce on GPUI's clock.
      testRoot.renderer.clockFastForward(100)
      const textarea = testRoot.findByType("textarea")[0]!
      const [x, y, width, height] = testRoot.getElementBounds(textarea.id)!
      const file = path.join(SHOTS_DIR, `${name}.png`)
      testRoot.captureScreenshot(file)
      const png = readPng(file)
      const scale = png.width / testRoot.renderer.getWindowSize().width
      let red = 0
      for (let py = y!; py < y! + height!; py++) {
        for (let px = x!; px < x! + width!; px++) {
          const [r, g, b] = png.rgb(px * scale, py * scale)
          if (r > 200 && g < 80 && b < 80) red++
        }
      }
      return red
    }

    it("paints SQL keywords in the keyword colour", () => {
      expect(redPixels("textarea-sql-plain")).toBe(0)
      expect(redPixels("textarea-sql", "sql")).toBeGreaterThan(20)
    })
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

  describe("external value", () => {
    function mountControlled(initial: string) {
      let controller!: TextareaController
      const [text, setText] = createSignal(initial)
      const changes: EventPayload[] = []
      testRoot.render(() => (
        <textarea
          ref={(c: TextareaController) => (controller = c)}
          value={text()}
          style={{ width: 300, height: 80 }}
          onChange={(event: EventPayload) => {
            changes.push(event)
            setText(event.value ?? "")
          }}
        />
      ))
      const id = testRoot.findByType("textarea")[0]!.id
      return { controller: () => controller, setText, changes, id }
    }

    it("keeps the caret where it was", () => {
      const { controller, setText, changes, id } = mountControlled("select 1")
      testRoot.renderer.nativeSimulateKeystrokes(id, "left left shift-left")
      expect(controller().getSelections()).toEqual([{ anchor: 6, head: 5 }])

      setText("-- select 1")
      testRoot.getPaintedText()
      expect(controller().getText()).toBe("-- select 1")
      expect(controller().getSelections()).toEqual([{ anchor: 9, head: 8 }])
      expect(changes.at(-1)).toMatchObject({
        origin: "value",
        value: "-- select 1",
        edits: [{ start: 0, end: 0, text: "-- " }],
      })
    })

    it("keeps the undo history and does not undo the new value", () => {
      const { controller, setText, id } = mountControlled("select 1")
      testRoot.renderer.nativeSimulateKeystrokes(id, "x")
      setText("-- select 1x")
      testRoot.getPaintedText()

      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-z")
      expect(controller().getText()).toBe("-- select 1")
      testRoot.renderer.nativeSimulateKeystrokes(id, "cmd-z")
      expect(controller().getText()).toBe("-- select 1")
    })

    it("adds no revision when the value echoes a change", () => {
      const { controller, changes, id } = mountControlled("")
      const revision = controller().revision()
      testRoot.renderer.nativeSimulateKeystrokes(id, "a b c")

      expect(controller().getText()).toBe("abc")
      expect(controller().revision()).toBe(revision + 3)
      expect(changes.map((event) => event.origin)).toEqual(["user", "user", "user"])
    })

    it("applies a value set just before applyEdits first", () => {
      const { controller, setText } = mountControlled("select 1")
      const revision = controller().revision()
      setText("select 2")
      const result = controller().applyEdits(revision, [{ start: 0, end: 6, text: "SELECT" }])

      expect(result.applied).toBe(true)
      expect(controller().getText()).toBe("SELECT 2")
    })
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
