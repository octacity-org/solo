/**
 * Solo Tasks — a small native task manager built entirely from Solid 2.
 *
 * Dogfood app for the @solo/solid runtime: list, add, toggle and delete
 * tasks. In-memory mock data only. All state is ordinary Solid primitives;
 * every interaction must reach GPUI as fine-grained native mutations.
 *
 * Layout notes (learned by dogfooding):
 * - Every flex container sets `display: "flex"` explicitly. GPUI's default
 *   display is Block, which ignores flexDirection/flexGrow entirely.
 * - The scrollable list relies on build_div's min-height 0 so it can shrink
 *   below its content inside a flex column (Taffy's automatic minimum size
 *   would otherwise pin it to content height → nothing to scroll).
 */

import { createSignal, For, Show } from "solid-js"
import { createStore } from "solid-js"
import { View, Text, Button } from "@solo/solid"
import type { StyleDesc } from "@solo/solid"

interface Task {
  id: number
  title: string
  completed: boolean
}

let nextTaskId = 1

function makeTasks(): Task[] {
  const seedTitles = [
    "Finish renderer cleanup",
    "Decouple GPUI from Zed",
    "Add lifecycle tests",
    "Test native scrolling",
    "Wire up text input focus",
    "Try hover styling on rows",
    "Check retained-tree reuse",
    "Profile applyBatch size",
    "Read GPUI paint docs",
    "Compare with opentui reconciler",
    "Clean up mutation queue",
    "Write task app README",
    "Verify window resize",
    "Try keyboard navigation",
    "Review event payload shape",
    "Delete-task lifecycle test",
    "Reorder regression test",
    "Dispose-root regression test",
    "Style completed rows dimly",
    "Add empty state",
    "Ship milestone report",
    "Plan next milestone",
    "Take a break",
    "Stretch goals triage",
    "Water the plants",
  ]
  return seedTitles.map((title) => ({
    id: nextTaskId++,
    title,
    completed: false,
  }))
}

const ROW_STYLE: StyleDesc = {
  display: "flex",
  flexDirection: "row",
  alignItems: "center",
  paddingLeft: 12,
  paddingRight: 8,
  paddingTop: 6,
  paddingBottom: 6,
}

const CHECKBOX_STYLE: StyleDesc = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: 22,
  height: 22,
  marginRight: 10,
  borderRadius: 5,
  borderWidth: 1,
  borderColor: "#45475a",
  cursor: "pointer",
}

const CHECKBOX_HOVER: StyleDesc = {
  ...CHECKBOX_STYLE,
  borderColor: "#89b4fa",
  backgroundColor: "#313244",
}

function Checkbox(props: { checked: () => boolean; onToggle: () => void }) {
  return (
    <div
      style={props.checked()
        ? { ...CHECKBOX_HOVER, backgroundColor: "#a6e3a1", borderColor: "#a6e3a1" }
        : { ...CHECKBOX_STYLE, hover: CHECKBOX_HOVER }}
      onClick={() => props.onToggle()}
      testId={`checkbox-${props.checked() ? "on" : "off"}`}
    >
      <Show when={props.checked()}>
        <Text style={{ color: "#1e1e2e", fontSize: 14 }}>✓</Text>
      </Show>
    </div>
  )
}

function TaskRow(props: {
  task: () => Task
  onToggle: () => void
  onDelete: () => void
}) {
  return (
    <div
      style={{ ...ROW_STYLE, hover: { backgroundColor: "#181825" } }}
      testId={`task-${props.task().id}`}
    >
      <Checkbox checked={() => props.task().completed} onToggle={props.onToggle} />
      <Text
        style={{
          flexGrow: 1,
          fontSize: 15,
          lineHeight: 20,
          color: props.task().completed ? "#585b70" : "#cdd6f4",
        }}
      >
        {props.task().title}
      </Text>
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          width: 24,
          height: 24,
          borderRadius: 5,
          cursor: "pointer",
          hover: { backgroundColor: "#45475a" },
        }}
        onClick={() => props.onDelete()}
        testId="delete"
      >
        <Text style={{ color: "#f38ba8", fontSize: 14 }}>×</Text>
      </div>
    </div>
  )
}

export function TasksApp() {
  // A store keeps every task object identity-stable: toggling mutates one
  // property in place so only that row's style/marker updates natively.
  const [tasks, setTasks] = createStore<Task[]>(makeTasks())
  const [draft, setDraft] = createSignal("")

  const addTask = () => {
    const title = draft().trim()
    if (!title) return
    setTasks((state) => {
      state.push({ id: nextTaskId++, title, completed: false })
    })
    setDraft("")
  }

  return (
    <View
      style={{
        display: "flex",
        flexDirection: "column",
        width: "100%",
        height: "100%",
        backgroundColor: "#1e1e2e",
        paddingLeft: 16,
        paddingRight: 16,
        paddingTop: 12,
        paddingBottom: 12,
      }}
    >
      {/* Header */}
      <View style={{ marginBottom: 8 }}>
        <Text style={{ fontSize: 20, fontWeight: "bold", color: "#cdd6f4" }}>Tasks</Text>
      </View>

      {/* Scrollable task list */}
      <View
        style={{
          display: "flex",
          flexGrow: 1,
          overflow: "scroll",
          flexDirection: "column",
        }}
        testId="task-list"
      >
        {/* Keyed by id: reordering moves native nodes instead of rebuilding.
            Key-function mode hands the children an accessor. */}
        <For each={tasks} keyed={(t) => (t as Task).id}>
          {(item) => {
            // Key-function mode hands us an accessor; read it lazily so every
            // property read lands in a tracking scope.
            const task = item as unknown as () => Task
            return (
              <TaskRow
                task={task}
                onToggle={() =>
                  setTasks((state) => {
                    const row = state.find((t) => t.id === task().id)
                    if (row) row.completed = !row.completed
                  })
                }
                onDelete={() =>
                  setTasks((state) => state.filter((t) => t.id !== task().id))
                }
              />
            )
          }}
        </For>
      </View>

      {/* Composer */}
      <View
        style={{
          display: "flex",
          flexDirection: "row",
          alignItems: "center",
          gap: 8,
          paddingTop: 10,
          borderTopWidth: 1,
          borderColor: "#313244",
        }}
      >
        <input
          value={draft()}
          placeholder="Describe the task…"
          onChange={(e) => setDraft(String(e.value ?? ""))}
          onSubmit={addTask}
          testId="new-task-input"
          style={{ flexGrow: 1, height: 32 }}
        />
        <Button onClick={addTask} testId="add">
          <Text style={{ fontSize: 14, fontWeight: "bold", color: "#1e1e2e" }}>+ Add task</Text>
        </Button>
      </View>
    </View>
  )
}
