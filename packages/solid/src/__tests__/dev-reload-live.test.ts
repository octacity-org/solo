import { execFileSync, spawn } from "node:child_process"
import { mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join, resolve } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import { connectStdio } from "../automation.js"

const root = fileURLToPath(new URL("../..", import.meta.url))

const APP = `import { createSignal } from "solid-js"
import { Button, registerApplicationCommand, render, Text, View } from "@solo/solid"

const LABEL = "v1"
function App() {
  const [count, setCount] = createSignal(0)
  registerApplicationCommand({ id: "bump", label: "Bump", shortcut: "cmd-r", menu: "Dev", run: () => setCount((n) => n + 1) })
  return <View style={{ display: "flex", flexDirection: "column", width: "100%", height: "100%" }}>
    <Text>{\`\${LABEL}:\${count()}\`}</Text>
    <Button testId="quit" onClick={() => app.quitApplication()}><Text>Quit</Text></Button>
  </View>
}
const app = render(() => <App />, { title: "Dev reload", width: 400, height: 300 })
`

/** A throwaway Vite project that resolves @solo/solid to this package. */
function createProject(): string {
  const dir = mkdtempSync(join(tmpdir(), "solo-dev-"))
  const modules = join(dir, "node_modules")
  mkdirSync(join(modules, "@solo"), { recursive: true })
  for (const name of readdirSync(join(root, "node_modules"))) {
    if (name === "@solo" || name.startsWith(".")) continue
    symlinkSync(join(root, "node_modules", name), join(modules, name))
  }
  for (const name of readdirSync(join(root, "node_modules", "@solo"))) {
    symlinkSync(join(root, "node_modules", "@solo", name), join(modules, "@solo", name))
  }
  symlinkSync(root, join(modules, "@solo", "solid"), "dir")
  writeFileSync(join(dir, "package.json"), JSON.stringify({ type: "module" }))
  writeFileSync(join(dir, "vite.config.ts"), `import { solidUniversal } from "@solo/solid/vite"
export default { plugins: [solidUniversal()], build: { ssr: "index.tsx", target: "node20" } }
`)
  writeFileSync(join(dir, "index.tsx"), APP)
  return dir
}

describe.skipIf(process.platform !== "darwin")("solo dev", () => {
  it("reloads the app Worker in the running process and survives build errors and crashes", async () => {
    // CI runs Solid tests before its package build; the CLI runs from dist.
    execFileSync("bun", ["run", "build"], { cwd: root, stdio: "inherit" })
    const dir = createProject()
    const edit = (text: string) => writeFileSync(join(dir, "index.tsx"), text)
    const child = spawn("bun", [resolve(root, "dist/cli.js"), "dev", dir], { stdio: ["pipe", "pipe", "pipe"] })
    let errors = ""
    child.stderr.on("data", (data) => { errors += data })
    const exited = new Promise<number | null>((done) => child.once("exit", (code) => done(code)))
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 60_000)

    // Each step waits for its own log line, after the previous one.
    let seen = 0
    const logged = async (line: string) => {
      await expect.poll(() => errors.indexOf(line, seen), { timeout: 20_000 }).toBeGreaterThanOrEqual(0)
      seen = errors.indexOf(line, seen) + line.length
    }
    // A reload replaces the app's automation server, so reconnect after one.
    const connect = () => connectStdio({
      write: (chunk) => { child.stdin.write(chunk) },
      feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
    })
    try {
      await logged("[solo] built in")
      let app = await connect()
      const label = async () => (await app.call("getPaintedText", {})).text[0]
      await expect.poll(label).toBe("v1:0")
      await app.call("keyDown", { key: "cmd-r" })
      await expect.poll(label).toBe("v1:1")

      // An edit restarts only the app: fresh state, and commands reach the new app.
      edit(APP.replace(`"v1"`, `"v2"`))
      await logged("[solo] reloaded in")
      app = await connect()
      await expect.poll(label).toBe("v2:0")
      await app.call("keyDown", { key: "cmd-r" })
      await expect.poll(label).toBe("v2:1")

      // A build error keeps the running app.
      edit(APP.replace("function App() {", "function App() { const = "))
      await logged("[solo] build failed; the app keeps running")
      await app.call("keyDown", { key: "cmd-r" })
      await expect.poll(label).toBe("v2:2")

      // A crash keeps the process and window until the next good build.
      edit(APP.replace("const app = render(", `throw new Error("boom")\nconst app = render(`))
      await logged("[solo] app exited with code 1; waiting for changes")
      expect(child.exitCode).toBe(null)
      edit(APP.replace(`"v1"`, `"v3"`))
      await logged("[solo] reloaded in")
      app = await connect()
      await expect.poll(label).toBe("v3:0")

      // Quitting the app ends the session.
      // The process may end before the click is acknowledged.
      void app.getByTestId("quit").click().catch(() => {})
      expect(await exited).toBe(0)
    } catch (error) {
      throw new Error(`solo dev failed:\n${errors}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      if (child.exitCode === null) child.kill("SIGKILL")
      rmSync(dir, { recursive: true, force: true })
    }
  }, 90_000)
})
