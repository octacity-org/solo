// Packaged Dock check (macOS, manual): a background app with a Dock menu,
// a reopen listener and one recent document. Events are appended to
// dock.log beside the .app. The window closes itself after a second so a
// Dock click (or `open -a`) can reopen it.
import { appendFileSync, writeFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { createSignal } from "solid-js"
import {
  addRecentDocument, configureDockMenu, onApplicationReopen, onOpenUrls, render, Text, View,
} from "@solo/solid"
import type { Root } from "@solo/solid"

const beside = resolve(dirname(process.execPath), "../../..")
const write = (line: string) => appendFileSync(resolve(beside, "dock.log"), `${line}\n`)
write(`launch pid=${process.pid}`)
let root: Root

function App() {
  const [count, setCount] = createSignal(0)
  configureDockMenu(() => [
    { id: "count", label: `Count (${count()})`, run: () => { setCount((n) => n + 1); write(`dock count ${count()}`) } },
    { type: "separator" },
    { type: "status", label: "Solo Dock fixture" },
    { id: "off", label: "Disabled item", enabled: false, run: () => write("dock disabled ran") },
  ])
  return <View style={{ padding: 16 }}><Text testId="count">Count: {count()}</Text></View>
}

onOpenUrls((urls) => { for (const url of urls) write(`url ${url}`) })
onApplicationReopen(() => { write("reopen"); root.showWindow() })
root = render(() => <App />, {
  title: "Solo Dock",
  menuBar: { iconPath: resolve(dirname(process.execPath), "../Resources/menu.png"), tooltip: "Solo Dock" },
})
const recent = resolve(beside, "recent note.txt")
writeFileSync(recent, "recent\n")
try { addRecentDocument(recent); write("recent added") } catch (error) { write(`recent error ${(error as Error).message}`) }
setTimeout(() => { root.closeWindow(); write("window closed") }, 1000)
