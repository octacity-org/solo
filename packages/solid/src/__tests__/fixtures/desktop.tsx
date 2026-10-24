import { writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { createSignal } from "solid-js"
import { Button, moveToTrash, openExternalUrl, openPath, registerApplicationCommand, render, revealPath, Text, View, writeClipboardText } from "@solo/solid"

const article = "https://example.com/世界?q=İstanbul#solo-desktop-check"

function App() {
  const [status, setStatus] = createSignal("Ready")
  const run = (action: () => void, success: string) => {
    try { action(); setStatus(success) }
    catch (error) { setStatus(`Error: ${error instanceof Error ? error.message : String(error)}`) }
  }
  const open = () => run(() => openExternalUrl("https://example.com/#solo-desktop-check"), "Browser request accepted")
  const copy = () => run(() => writeClipboardText(article), "Copied Unicode article URL")
  const invalid = () => run(() => openExternalUrl("javascript:alert(1)"), "Unexpected success")
  const sample = () => {
    const file = join(tmpdir(), "Solo Path Check.txt")
    writeFileSync(file, "Opened by Solo's openPath.\n")
    return file
  }
  const reveal = () => run(() => revealPath(sample()), "Showed Solo Path Check.txt in Finder")
  const openFile = () => run(() => openPath(sample()), "Opened Solo Path Check.txt")
  const trash = () => {
    const file = join(tmpdir(), "Solo Trash Check.txt")
    writeFileSync(file, "Put me back from the Trash.\n")
    moveToTrash([file]).then(
      ([result]) => setStatus(result?.ok ? "Moved Solo Trash Check.txt to the Trash" : `Error: ${result?.error}`),
      (error) => setStatus(`Error: ${error instanceof Error ? error.message : String(error)}`),
    )
  }
  registerApplicationCommand({ id: "open", label: "Open test URL", menu: "Desktop", shortcut: "cmd-shift-b", run: open })
  registerApplicationCommand({ id: "copy", label: "Copy Unicode URL", menu: "Desktop", shortcut: "cmd-shift-y", run: copy })
  registerApplicationCommand({ id: "invalid", label: "Reject invalid URL", menu: "Desktop", run: invalid })
  registerApplicationCommand({ id: "reveal", label: "Show test file in Finder", menu: "Desktop", run: reveal })
  registerApplicationCommand({ id: "open-file", label: "Open test file", menu: "Desktop", run: openFile })
  registerApplicationCommand({ id: "trash", label: "Move test file to Trash", menu: "Desktop", run: trash })
  return <View style={{ display: "flex", flexDirection: "column", padding: 24, gap: 16, width: "100%", height: "100%", backgroundColor: "#202433", color: "#ffffff" }}>
    <Text>Desktop integration verification</Text>
    <Text testId="status">{status()}</Text>
    <Text>{article}</Text>
    <Button testId="open" onClick={open}><Text>Open test URL</Text></Button>
    <Button testId="copy" onClick={copy}><Text>Copy Unicode URL</Text></Button>
    <Button testId="invalid" onClick={invalid}><Text>Reject invalid URL</Text></Button>
    <Button testId="reveal" onClick={reveal}><Text>Show test file in Finder</Text></Button>
    <Button testId="open-file" onClick={openFile}><Text>Open test file</Text></Button>
    <Button testId="trash" onClick={trash}><Text>Move test file to Trash</Text></Button>
    <input testId="editor" placeholder="Existing native copy/paste still works" style={{ width: 480, height: 44 }} />
  </View>
}

render(() => <App />, { title: "Solo Desktop", width: 560, height: 580 })
