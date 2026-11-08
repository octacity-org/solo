// Packaged deep-link check: the bundle declares the solo-deeplink-test scheme
// (package-commands.ts). Each received URL is appended to deep-links.log
// beside the .app, so a cold `open` and a warm one can both be checked.
import { appendFileSync } from "node:fs"
import { dirname, resolve } from "node:path"
import { createSignal } from "solid-js"
import { onOpenUrls, registerUrlScheme, render, Text, View } from "@solo/solid"

const log = resolve(dirname(process.execPath), "../../../deep-links.log")
const write = (line: string) => appendFileSync(log, `${line}\n`)
write(`launch pid=${process.pid}`)

function App() {
  const [urls, setUrls] = createSignal<string[]>([])
  onOpenUrls((received) => {
    // Untrusted input: only parse and display it.
    for (const url of received) write(`url pid=${process.pid} ${url}`)
    setUrls((all) => [...all, ...received])
  })
  return <View style={{ display: "flex", flexDirection: "column", gap: 8, padding: 16 }}>
    <Text testId="count">URLs: {urls().length}</Text>
    {urls().map((url) => <Text>{url}</Text>)}
  </View>
}

render(() => <App />, { title: "Solo Deep Links" })
registerUrlScheme(process.env.SOLO_REGISTER_SCHEME ?? "solo-deeplink-test").then(
  () => write("register ok"),
  (error: Error) => write(`register error ${error.message}`),
)
