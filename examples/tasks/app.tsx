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
import { createContextMenu, View, Text, Button, getScreens, onOpenUrls, openWindow } from "@solo/solid"
import type { AppWindow, ContextMenuAnchor, StyleDesc } from "@solo/solid"

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
  onOpenMenu: (anchor: ContextMenuAnchor) => void
}) {
  return (
    <div
      style={{ ...ROW_STYLE, hover: { backgroundColor: "#181825" } }}
      testId={`task-${props.task().id}`}
      onMouseDown={(e) => {
        if (e.button === 2) {
          props.onOpenMenu(e)
        }
      }}
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
          marginRight: 4,
          hover: { backgroundColor: "#45475a" },
        }}
        onClick={(e) => props.onOpenMenu(e)}
        testId="task-menu-btn"
      >
        <Text style={{ color: "#cdd6f4", fontSize: 14 }}>⋮</Text>
      </div>
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

function StatsWindow(props: {
  tasks: () => Task[]
  windowRef: () => AppWindow | undefined
  closeRequested: () => boolean
  onConfirmedClose: () => void
}) {
  const [subCount, setSubCount] = createSignal(0)
  const [stateLog, setStateLog] = createSignal("")

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
      testId="secondary-window-root"
    >
      <Text style={{ fontSize: 18, fontWeight: "bold", color: "#cdd6f4", marginBottom: 8 }}>
        Task Statistics
      </Text>
      <Text testId="secondary-total-tasks" style={{ color: "#a6adc8", fontSize: 14 }}>
        {`Total tasks: ${props.tasks().length}`}
      </Text>
      <Text testId="secondary-counter" style={{ color: "#a6adc8", fontSize: 14 }}>
        {`Counter: ${subCount()}`}
      </Text>
      <Text testId="secondary-state-log" style={{ color: "#a6adc8", fontSize: 12 }}>
        {stateLog()}
      </Text>

      {/* Button to click inside the secondary window */}
      <div
        testId="secondary-increment-btn"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          marginTop: 8,
          paddingTop: 6,
          paddingBottom: 6,
          backgroundColor: "#89b4fa",
          borderRadius: 4,
          cursor: "pointer",
        }}
        onClick={() => setSubCount((c) => c + 1)}
      >
        <Text style={{ color: "#11111b", fontWeight: "bold" }}>Increment</Text>
      </div>

      {/* Button to test window state: resize, maximize, fullscreen, background */}
      <div
        testId="secondary-test-state-btn"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          marginTop: 8,
          paddingTop: 6,
          paddingBottom: 6,
          backgroundColor: "#b4befe",
          borderRadius: 4,
          cursor: "pointer",
        }}
        onClick={() => {
          const win = props.windowRef()
          if (!win) return
          const initialSize = win.size()
          win.resize({ width: 420, height: 320 })
          const resizedSize = win.size()

          win.setBackground("transparent")
          win.setBackground("opaque")

          const wasMax = win.isMaximized()
          win.maximize()
          const isMax = win.isMaximized()
          win.restore()
          const restored = !win.isMaximized()

          const beforeFs = win.isFullscreen()
          win.toggleFullscreen()
          const inFs = win.isFullscreen()
          win.toggleFullscreen()
          const outFs = win.isFullscreen()

          win.setDocumentEdited(true)
          const isWindows =
            (globalThis as { process?: { platform?: string } }).process?.platform === "win32"
          const dummyPath = isWindows ? "C:\\notes.txt" : "/tmp/notes.txt"
          win.setDocumentPath(dummyPath)
          win.setDocumentEdited(false)
          win.setDocumentPath(null)

          win.setTabbingIdentifier("stats-group")
          let tabOverviewThrew = false
          try {
            win.toggleTabOverview()
          } catch {
            tabOverviewThrew = true
          }

          let charPaletteThrew = false
          try {
            win.showCharacterPalette()
          } catch {
            charPaletteThrew = true
          }

          win.setTitle("Task Statistics (Active)")
          win.focus()

          setStateLog(
            `ok;initial:${initialSize.width}x${initialSize.height};resized:${resizedSize.width}x${resizedSize.height};wasMax:${wasMax};isMax:${isMax};restored:${restored};fs:${inFs};tabThrew:${tabOverviewThrew};charThrew:${charPaletteThrew}`
          )
        }}
      >
        <Text style={{ color: "#11111b" }}>Test Window State</Text>
      </div>

      {/* Guarded close request trigger */}
      <div
        testId="secondary-request-close-btn"
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          marginTop: 8,
          paddingTop: 6,
          paddingBottom: 6,
          backgroundColor: "#f9e2af",
          borderRadius: 4,
          cursor: "pointer",
        }}
        onClick={() => {
          const win = props.windowRef()
          win?.requestClose()
        }}
      >
        <Text style={{ color: "#11111b" }}>Request Close</Text>
      </div>

      {/* Close confirmation button, shown when close was requested */}
      <Show when={props.closeRequested()}>
        <div
          testId="secondary-confirm-close-btn"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            marginTop: 8,
            paddingTop: 6,
            paddingBottom: 6,
            backgroundColor: "#f38ba8",
            borderRadius: 4,
            cursor: "pointer",
          }}
          onClick={() => {
            props.onConfirmedClose()
            props.windowRef()?.close()
          }}
        >
          <Text style={{ color: "#11111b" }}>Confirm Close</Text>
        </div>
      </Show>
    </View>
  )
}

export function TasksApp() {
  // A store keeps every task object identity-stable: toggling mutates one
  // property in place so only that row's style/marker updates natively.
  const [tasks, setTasks] = createStore<Task[]>(makeTasks())
  const [draft, setDraft] = createSignal("")
  const [query, setQuery] = createSignal("SELECT title FROM tasks WHERE completed = false")
  const [querySelection, setQuerySelection] = createSignal("none")
  const [menuStatus, setMenuStatus] = createSignal("idle")

  const openMenu = (taskId: number, anchor: ContextMenuAnchor) => {
    const task = tasks.find((t) => t.id === taskId)
    if (!task) return
    const menu = createContextMenu(
      [
        { id: "toggle", label: "Toggle complete", checked: task.completed },
        { type: "separator" },
        { id: "inspect", label: "Inspect Task", disabled: true },
        { type: "separator" },
        { id: "priority", label: "Set Priority" },
        { id: "delete", label: "Delete task" },
      ],
      (id) => {
        setMenuStatus(id)
        if (id === "toggle") {
          setTasks((state) => {
            const row = state.find((t) => t.id === taskId)
            if (row) row.completed = !row.completed
          })
        } else if (id === "delete") {
          setTasks((state) => state.filter((t) => t.id !== taskId))
        }
      },
    )

    setMenuStatus("open")
    void menu.show(anchor).then(
      (id) => setMenuStatus(id ?? "cancelled"),
      (err) => setMenuStatus(`error: ${String(err)}`),
    )
  }

  let secondaryWindow: AppWindow | undefined
  const [closeRequested, setCloseRequested] = createSignal(false)
  const [secondaryClosed, setSecondaryClosed] = createSignal(false)

  const openSecondary = () => {
    if (secondaryWindow && !secondaryWindow.closed) {
      secondaryWindow.focus()
      return
    }
    setCloseRequested(false)
    setSecondaryClosed(false)

    const screens = getScreens()
    const primaryScreen = screens.find((s) => s.primary) ?? screens[0]

    secondaryWindow = openWindow(
      () => (
        <StatsWindow
          tasks={() => tasks}
          windowRef={() => secondaryWindow}
          closeRequested={closeRequested}
          onConfirmedClose={() => setSecondaryClosed(true)}
        />
      ),
      {
        title: "Task Statistics",
        width: 360,
        height: 300,
        screen: primaryScreen?.id,
        onCloseRequest: () => {
          setCloseRequested(true)
        },
        onClose: () => {
          setSecondaryClosed(true)
        },
      }
    )
  }

  onOpenUrls((urls) => {
    setTasks((state) => {
      for (const url of urls) {
        state.push({ id: nextTaskId++, title: `Link: ${url}`, completed: false })
      }
    })
  })

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
      <View
        style={{
          display: "flex",
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "space-between",
          marginBottom: 8,
        }}
      >
        <Text style={{ fontSize: 20, fontWeight: "bold", color: "#cdd6f4" }}>Tasks</Text>
        <Text testId="menu-status" style={{ fontSize: 12, color: "#a6adc8" }}>
          {menuStatus()}
        </Text>
        <div
          testId="open-secondary-window"
          style={{
            display: "flex",
            alignItems: "center",
            paddingLeft: 10,
            paddingRight: 10,
            paddingTop: 4,
            paddingBottom: 4,
            borderRadius: 5,
            backgroundColor: "#313244",
            cursor: "pointer",
            hover: { backgroundColor: "#45475a" },
          }}
          onClick={openSecondary}
        >
          <Text style={{ color: "#cdd6f4", fontSize: 13 }}>Stats</Text>
        </div>
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
                onOpenMenu={(anchor) => openMenu(task().id, anchor)}
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

      {/* Saved query: a SQL editor that reports its selection */}
      <textarea
        value={query()}
        language="sql"
        maxRows={3}
        onChange={(e) => setQuery(String(e.value ?? ""))}
        onSelectionChange={(e) => {
          const selection = e.selections?.[0]
          if (selection) setQuerySelection(`${selection.anchor}-${selection.head}`)
        }}
        testId="query-editor"
        style={{ marginTop: 8 }}
      />
      <Text testId="query-selection" style={{ fontSize: 12, color: "#a6adc8" }}>
        {`Selection ${querySelection()}`}
      </Text>
    </View>
  )
}
