# Text editors

`<input>` and `<textarea>` are native editors: GPUI owns the text, the caret,
the selection, IME composition and undo. JavaScript sees the text through
`change` events and drives it through a controller.

```tsx
import type { TextareaController } from "@solo/solid"

function QueryEditor() {
  let editor!: TextareaController
  const [query, setQuery] = createSignal("SELECT 1")

  return (
    <textarea
      ref={(controller) => (editor = controller)}
      value={query()}
      language="sql"
      minRows={6}
      onChange={(event) => setQuery(event.value!)}
      onSelectionChange={(event) => showCaret(event.selections![0])}
      style={{ flexGrow: 1, minHeight: 0 }}
    />
  )
}
```

Every offset is in UTF-16 code units, the unit JavaScript strings index by.

## Events

`onChange` fires after every change to the text, whatever caused it:

| Field | Meaning |
|---|---|
| `value` | The whole new text. |
| `revision` | The text's revision after the change. |
| `edits` | `{ start, end, text }` replacements that turn the previous text into `value`. Apply them **in order**: each one's offsets are on the text the previous one left. |
| `origin` | What caused the change: `"user"` (typing, paste, cut, IME), `"undo"`, `"redo"`, `"api"` (`applyEdits`) or `"value"` (a new `value` prop). |

`onSelectionChange` fires when the caret or selection moves, at most once per
frame, after layout:

| Field | Meaning |
|---|---|
| `selections` | `{ anchor, head }` pairs, primary last. `head` is the caret; it equals `anchor` when nothing is selected. |
| `revision` | The revision the offsets refer to. |
| `caret` | `{ x, y, height }` of the primary caret as drawn, in window coordinates. Absent before the first layout. |

## The controller

The `ref` of an `<input>` or `<textarea>` is a `TextareaController`. Its
methods run synchronously. Each one first flushes pending prop changes, so a
`value` set just before a call is applied first.

```ts
interface TextareaController {
  readonly id: number
  revision(): number
  getText(): string
  getSelections(): TextSelection[]
  setSelections(selections: TextSelection[], options?: { reveal?: boolean }): void
  applyEdits(
    revision: number,
    edits: TextEdit[],
    options?: { selections?: TextSelection[]; undoable?: boolean },
  ): ApplyEditsResult
}
```

- `setSelections` replaces the selection. `reveal: true` scrolls the caret into
  view; without it the scroll position stays where it is.
- `applyEdits` applies non-overlapping edits as **one** revision, and by
  default as one undo step. It returns `{ applied: true, revision }` or
  `{ applied: false, reason, revision }`, where `revision` is the current
  one. Nothing is applied when a call is rejected.
  - `selections` sets the selection afterwards, in offsets of the new text.
    Without it the selection moves with the edits.
  - `undoable: false` keeps the edits out of the undo history. The history
    stays, moved past them, so undo still reverts the user's own typing.

Malformed arguments (negative offsets, an edit that ends before it starts,
overlapping edits, an edit past the end of the text) throw instead of
returning a result.

### Edits from an older revision

An app that computes an edit asynchronously, such as a formatter or a
completion, can pass the revision it read the text at. The editor moves the
edit past everything that happened since:

```ts
const revision = editor.revision()
const formatted = await format(editor.getText())
const result = editor.applyEdits(revision, [{ start: 0, end: length, text: formatted }])
if (!result.applied) {
  // The user typed inside the range meanwhile, or it is too old.
}
```

The rules:

- An edit entirely before or after a later change moves with it.
- A later change that reaches into the edit's range rejects the call with
  `conflict`. A change that only touches its boundary does not.
- An insertion at the same point as a later insertion goes after it.
- The editor remembers the last 512 revisions. When the log fills, it keeps
  the newest 256. A call from a revision older than that is rejected with
  `expired`: read the text again and recompute.

## `value`

`value` is the text the app wants. When it changes, the editor replaces only
the part that differs, so the caret, the selection, the scroll position and
the undo history stay where they were, moved past the replaced text. The
change is reported with origin `"value"`, and undo does not revert it.

Writing back the `value` a `change` event carried is recognised as an echo
and changes nothing, so the usual controlled pattern
(`onChange={(event) => setValue(event.value!)}`) adds no revisions.

## Syntax highlighting

`language` highlights the text with a Tree-sitter grammar, for example
`language="sql"`. Highlighting runs off the main thread once typing pauses
for about 30 ms. Until a fresh result arrives, lines an edit did not touch
keep their colours and edited lines draw plain.

The grammar must be built into `@solo/native`; SQL is the `syntax-sql`
feature, which `syntax-all` leaves out because of its size (about 13 MB).
Without it the text stays plain. The colours come from the
`theme` prop's `syntax` palette, as in `code` blocks:

```tsx
<textarea language="sql" theme={{ syntax: { keyword: "#c678dd", string: "#98c379" } }} />
```

## Current boundaries

- One selection. The API takes and returns arrays so that multiple
  selections can land without a breaking change; for now only the last
  (primary) selection is kept.
- The text is one string, rehighlighted in full after each pause, and every
  line is laid out. Very large documents are not yet efficient.
- `change` events still carry the whole `value`, which costs a copy of the
  text per keystroke on large documents.
