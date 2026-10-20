import { createSignal, Show } from "solid-js"
import { Button, render, Text } from "@solo/solid"

// Every click mounts or unmounts controls that automation uses at once,
// before the window's next display frame.
function App() {
  const [open, setOpen] = createSignal(false)
  const [value, setValue] = createSignal("")
  const [presses, setPresses] = createSignal(0)
  return <div style={{ display: "flex", flexDirection: "column", gap: 8, padding: 12 }}>
    <Button testId="toggle" onClick={() => setOpen((open) => !open)}>Toggle editor</Button>
    <Show when={open()}>
      <input testId="field" value={value()} onChange={(event) => setValue(event.value ?? "")} style={{ height: 32 }} />
      <Button testId="mounted" onClick={() => setPresses((count) => count + 1)}>Mounted button</Button>
    </Show>
    <Text testId="value">{value() || "Empty"}</Text>
    <Text testId="presses">{`Presses: ${presses()}`}</Text>
  </div>
}

render(() => <App />, { title: "Solo Frames", width: 400, height: 300 })
