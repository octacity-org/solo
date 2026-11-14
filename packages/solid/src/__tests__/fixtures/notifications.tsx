// Packaged notification check (package-commands.ts notifications). Every post,
// dismissal, error and response is appended to notifications.log beside the
// .app, so posting can be driven by automation and the clicks done by hand,
// including a click that launches the app after it quit.
import { appendFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { createSignal } from "solid-js"
import { Button, dismissNotification, onNotificationResponse, render, showNotification, Text, View } from "@solo/solid"
import type { ShowNotificationOptions } from "@solo/solid"

const log = resolve(dirname(process.execPath), "../../../notifications.log")
const write = (line: string) => appendFileSync(log, `${line}\n`)
write(`launch pid=${process.pid}`)

function attempt(name: string, call: () => void) {
  try {
    call()
    write(`${name} ok`)
  } catch (error) {
    write(`${name} error ${(error as Error).message}`)
  }
}

const post = (name: string, options: ShowNotificationOptions) => attempt(name, () => showNotification(options))

function App() {
  const [last, setLast] = createSignal("none")
  onNotificationResponse(({ tag, actionId }) => {
    write(`response pid=${process.pid} tag=${tag} action=${actionId ?? "body"}`)
    setLast(`${tag} / ${actionId ?? "body"}`)
  })
  const button = (testId: string, label: string, onClick: () => void) =>
    <Button testId={testId} onClick={onClick}><Text>{label}</Text></Button>
  return <View style={{ display: "flex", flexDirection: "column", gap: 8, padding: 16 }}>
    {button("post", "Post", () => post("post", { tag: "plain", title: "Solo notification", body: "Click me" }))}
    {button("replace", "Replace", () => post("replace", { tag: "plain", title: "Replaced notification", body: "Same tag" }))}
    {button("actions", "Post with actions", () => post("actions", {
      tag: "actions",
      title: "Download finished",
      body: "report.pdf",
      actions: [{ id: "open", label: "Open" }, { id: "later", label: "Later" }],
    }))}
    {button("dismiss", "Dismiss", () => attempt("dismiss", () => dismissNotification("plain")))}
    <Text testId="last">Last response: {last()}</Text>
  </View>
}

render(() => <App />, { title: "Solo Notifications" })
