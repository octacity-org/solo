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
    <Button testId="inspector-request-close" onClick={() => inspector?.requestClose()}>Request close</Button>
  </View>
}

function App() {
  const open = () => {
    inspector = openWindow(() => <Inspector />, {
      title: "Solo Inspector",
      width: 480,
      height: 200,
      screen: primary?.id,
      onClose: () => console.error("windows:inspector-closed"),
      // Keeps the window open; the test then closes it with close().
      onCloseRequest: () => console.error("windows:inspector-close-requested"),
    })
  }
  return <View style={{ display: "flex", gap: 8, padding: 16 }}>
    <Text testId="count">Count: {count()}</Text>
    <Text testId="screens">Screens: {screens.length}, primary: {primary ? "yes" : "no"}</Text>
    <Button testId="open" onClick={open}>Open inspector</Button>
    <Button testId="increment" onClick={() => setCount((value) => value + 1)}>Increment</Button>
    <Button testId="request-close" onClick={() => root.requestCloseWindow()}>Request close</Button>
    <Button testId="quit" onClick={() => root.quitApplication()}>Quit</Button>
  </View>
}

root = render(() => <App />, {
  title: "Solo Windows",
  // Wide enough for the row of controls.
  width: 720,
  height: 300,
  screen: primary?.id,
  onCloseRequest: () => console.error("windows:main-close-requested"),
})
