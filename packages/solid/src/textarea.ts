/// Public types and controls for `<input>` and `<textarea>` text.
///
/// Every offset is in UTF-16 code units, the unit JavaScript strings index by.

import type { NativeRenderer } from "@solo/core"
import { requireCapability } from "./capability.js"

/** A replacement of `start..end` with `text`. */
export interface TextEdit {
  start: number
  end: number
  text: string
}

/** A selection. `head` is where the caret is; it equals `anchor` when nothing is selected. */
export interface TextSelection {
  anchor: number
  head: number
}

export type ApplyEditsResult =
  | { applied: true; revision: number }
  | { applied: false; reason: "conflict" | "expired"; revision: number }

/**
 * The `ref` of an `<input>` or `<textarea>`. Its methods act on the native
 * editor synchronously, after flushing pending prop changes, so a `value`
 * set just before a call is applied first.
 */
export interface TextareaController {
  readonly id: number
  /** The text's revision. Every edit, from any source, increments it. */
  revision(): number
  getText(): string
  /** The selections, primary last. There is one until multiple selections land. */
  getSelections(): TextSelection[]
  /**
   * Replace the selections. Only the last (primary) one is kept for now.
   * `reveal` scrolls its caret into view.
   */
  setSelections(selections: TextSelection[], options?: { reveal?: boolean }): void
  /**
   * Apply non-overlapping `edits`, made against `revision`, as one revision.
   * Edits from an older revision are moved past the edits made since; if one
   * of those reached into an edited range the call is rejected with
   * `conflict`, and if the editor no longer remembers `revision`, with
   * `expired`. Nothing is applied when a call is rejected.
   *
   * `selections` replaces the selection afterwards (offsets in the new text);
   * without it the selection moves with the edits. `undoable` (default true)
   * makes the call one undo step; otherwise undo history is kept and moved
   * past the edits.
   */
  applyEdits(
    revision: number,
    edits: TextEdit[],
    options?: { selections?: TextSelection[]; undoable?: boolean },
  ): ApplyEditsResult
}

function assertOffset(value: unknown, path: string): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) {
    throw new TypeError(`${path} must be a non-negative integer`)
  }
}

function checkSelections(selections: TextSelection[], path: string): TextSelection[] {
  if (!Array.isArray(selections) || selections.length === 0) {
    throw new TypeError(`${path} must be a non-empty array`)
  }
  return selections.map((selection, index) => {
    assertOffset(selection?.anchor, `${path}[${index}].anchor`)
    assertOffset(selection.head, `${path}[${index}].head`)
    return { anchor: selection.anchor, head: selection.head }
  })
}

function checkEdits(edits: TextEdit[]): TextEdit[] {
  if (!Array.isArray(edits)) throw new TypeError("edits must be an array")
  return edits.map((edit, index) => {
    assertOffset(edit?.start, `edits[${index}].start`)
    assertOffset(edit.end, `edits[${index}].end`)
    if (edit.end < edit.start) throw new RangeError(`edits[${index}] ends before it starts`)
    if (typeof edit.text !== "string") throw new TypeError(`edits[${index}].text must be a string`)
    return { start: edit.start, end: edit.end, text: edit.text }
  })
}

/**
 * The controller for the text editor element `id`. `flush` commits pending
 * mutations, so the native element has the latest props when a method runs.
 */
export function createTextareaController(
  id: number,
  renderer: NativeRenderer,
  flush: () => void,
): TextareaController {
  const call = (method: string, args: unknown = null): unknown => {
    const callElement = requireCapability(renderer, "callElement", "support text editor methods")
    flush()
    return JSON.parse(callElement(id, method, JSON.stringify(args)))
  }

  return {
    id,
    revision: () => call("revision") as number,
    getText: () => call("getText") as string,
    getSelections: () => call("getSelections") as TextSelection[],
    setSelections(selections, options) {
      call("setSelections", {
        selections: checkSelections(selections, "selections"),
        reveal: options?.reveal ?? false,
      })
    },
    applyEdits(revision, edits, options) {
      assertOffset(revision, "revision")
      return call("applyEdits", {
        revision,
        edits: checkEdits(edits),
        selections: options?.selections && checkSelections(options.selections, "options.selections"),
        undoable: options?.undoable ?? true,
      }) as ApplyEditsResult
    },
  }
}
