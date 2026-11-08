import { createSignal } from "solid-js"
import { Button, getScreens, openWindow, render, Text, View } from "@solo/solid"
import type { AppWindow, Root } from "@solo/solid"

let root: Root
const [count, setCount] = createSignal(0)
let inspector: AppWindow | undefined
// Listed before render(): the GPUI loop already runs on the main thread.
const screens = getScreens()
const primary = screens.find((screen) => screen.primary)

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
      screen: primary?.id,
      onClose: () => console.error("windows:inspector-closed"),
    })
  }
  return <View style={{ display: "flex", gap: 8, padding: 16 }}>
    <Text testId="count">Count: {count()}</Text>
    <Text testId="screens">Screens: {screens.length}, primary: {primary ? "yes" : "no"}</Text>
    <Button testId="open" onClick={open}>Open inspector</Button>
    <Button testId="increment" onClick={() => setCount((value) => value + 1)}>Increment</Button>
    <Button testId="quit" onClick={() => root.quitApplication()}>Quit</Button>
  </View>
}

root = render(() => <App />, {
  title: "Solo Windows",
  // Wide enough for the row of controls.
  width: 560,
  height: 300,
  screen: primary?.id,
})
