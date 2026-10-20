import { createSignal } from "solid-js"
import { Button, openWindow, render, Text, View } from "@solo/solid"
import type { AppWindow, Root } from "@solo/solid"

let root: Root
const [count, setCount] = createSignal(0)
let inspector: AppWindow | undefined

function Inspector() {
  return <View style={{ display: "flex", gap: 8, padding: 16 }}>
    <Text testId="inspector-count">Inspector: {count()}</Text>
    <Button testId="inspector-increment" onClick={() => setCount((value) => value + 1)}>Increment</Button>
    <Button testId="inspector-close" onClick={() => inspector?.close()}>Close</Button>
  </View>
}

function App() {
  const open = () => {
    inspector = openWindow(() => <Inspector />, {
      title: "Solo Inspector",
      width: 320,
      height: 200,
      onClose: () => console.error("windows:inspector-closed"),
    })
  }
  return <View style={{ display: "flex", gap: 8, padding: 16 }}>
    <Text testId="count">Count: {count()}</Text>
    <Button testId="open" onClick={open}>Open inspector</Button>
    <Button testId="increment" onClick={() => setCount((value) => value + 1)}>Increment</Button>
    <Button testId="quit" onClick={() => root.quitApplication()}>Quit</Button>
  </View>
}

root = render(() => <App />, { title: "Solo Windows", width: 400, height: 300 })
