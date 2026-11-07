import { execFile, spawn } from "node:child_process"
import { mkdirSync, readdirSync, realpathSync, rmSync } from "node:fs"
import { basename, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"
import { App, connectStdio, SseBackend } from "../automation.js"
import type { TreeNode } from "../automation.js"
import { interactiveMenus } from "./test-utils.js"

const cases = [
  { action: "close", stallHelper: false },
  { action: "close", stallHelper: true },
  { action: "quit", stallHelper: true },
]

describe.skipIf(process.platform !== "darwin")("packaged native menus", () => {
  it.each(cases)("keeps JS responsive and drains helper cleanup ($action, stalled: $stallHelper)",
    (options) => packagedMenus({ ...options, scripted: true }), 60_000)

  // An open AppKit menu is a system-wide modal that the person at the machine
  // can dismiss or steer, so these runs need an idle machine.
  describe.skipIf(!interactiveMenus)("on screen", () => {
    it.each(cases)("tracks a real menu by keyboard ($action, stalled: $stallHelper)",
      (options) => packagedMenus({ ...options, scripted: false }), 60_000)
  })
})

async function packagedMenus({ action, stallHelper, scripted }: { action: string, stallHelper: boolean, scripted: boolean }) {
  const systemUiTimeout = 10_000
  const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
  const { stdout } = await promisify(execFile)("bun", [packager, "context-menus"], { timeout: 30_000 })
  const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
  if (!executable) throw new Error("Packager did not return an executable")
  const output = dirname(dirname(dirname(dirname(executable))))
  const runtimeTmp = join(output, "runtime-tmp")
  mkdirSync(runtimeTmp)
  const child = spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, TMPDIR: runtimeTmp } })
  let errors = ""
  child.stderr.on("data", (data) => { errors += data })
  const exited = new Promise<number | null>((resolve, reject) => {
    child.once("error", reject); child.once("exit", resolve)
  })
  void exited.catch(() => {})
  const watchdog = setTimeout(() => child.kill("SIGKILL"), 45_000)
  let stoppedHelper: number | undefined
  try {
    const transport = {
      write: (chunk: string) => { child.stdin.write(chunk) },
      feed: (listener: (chunk: string) => void) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
      close: async () => { child.kill() },
    }
    // The `initialize` handshake attaches automation, which scripts the
    // app's menus. Without it, they open on screen and AppKit tracks them.
    const app = scripted
      ? await connectStdio(transport)
      : new App(new SseBackend(transport.write, transport.feed, transport.close))
    const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
    const text = async (id: string) => flatten(await app.getByTestId(id).element())
    const ticks = async () => Number((await text("ticks")).split(": ")[1])
    await expect.poll(async () => (await app.getByTestId("owner").bounds()).height).toBeGreaterThan(0)
    for (let cycle = 0; cycle < 2; cycle++) {
      await app.getByTestId("owner").click()
      await expect.poll(() => text("status")).toBe("Menu pending")
      const before = await ticks()
      await expect.poll(ticks).toBeGreaterThan(before + 10)
      expect(await text("status"), "menu must remain open before keyboard input").toBe("Menu pending")
      await app.getByTestId("owner").press("down enter")
      await expect.poll(() => text("status"), { timeout: systemUiTimeout }).toBe("read")
      expect(readdirSync(runtimeTmp).filter((name) => name.startsWith("solo-context-menu-"))).toEqual([])
    }
    await app.getByTestId("owner").click()
    await expect.poll(() => text("status")).toBe("Menu pending")
    await app.getByTestId("owner").press("escape")
    await expect.poll(() => text("status"), { timeout: systemUiTimeout }).toBe("Cancelled")
    await app.getByTestId("owner").click()
    await expect.poll(() => text("status")).toBe("Menu pending")
    const beforeUnmount = await ticks()
    await expect.poll(ticks).toBeGreaterThan(beforeUnmount + 10)
    await app.getByTestId("remove").click()
    await expect.poll(() => text("status"), { timeout: systemUiTimeout }).toBe("Cancelled")
    await app.getByTestId("restore").click()
    await app.getByTestId("owner").click()
    await expect.poll(() => text("status")).toBe("Menu pending")
    if (stallHelper) {
      // Suspend the real helper at the OS process boundary. Cancellation must
      // reach its bounded kill/reap path, not win by favorable AppKit timing.
      await expect.poll(async () => {
        const { stdout } = await promisify(execFile)("ps", ["-axo", "pid=,command="])
        for (const directory of readdirSync(runtimeTmp).filter((name) => name.startsWith("solo-context-menu-"))) {
          const executable = join(runtimeTmp, directory, "solo-context-menu")
          const physicalExecutable = realpathSync(executable)
          const match = stdout.split("\n").map((line) => line.trim().match(/^(\d+)\s+(.+)$/))
            .find((match) => match?.[2] === executable || match?.[2] === physicalExecutable)
          if (match) {
            stoppedHelper = Number(match[1])
            return stoppedHelper
          }
        }
        return 0
      }).toBeGreaterThan(0)
      process.kill(stoppedHelper!, "SIGSTOP")
    }
    await app.getByTestId(action).click()
    expect(await exited, errors).toBe(0)
    expect(readdirSync(runtimeTmp).filter((name) => name.startsWith("solo-context-menu-"))).toEqual([])
    if (stoppedHelper !== undefined) {
      expect(() => process.kill(stoppedHelper!, 0)).toThrow()
      expect(errors).toContain("shutdown-tick")
    }
  } catch (error) {
    throw new Error(`Packaged context menu failed:\n${errors}`, { cause: error })
  } finally {
    clearTimeout(watchdog)
    if (stoppedHelper !== undefined) {
      try { process.kill(stoppedHelper, "SIGKILL") } catch { /* already reaped */ }
    }
    child.kill("SIGKILL")
    await exited.catch(() => {})
    if (basename(output).startsWith("solo-commands-")) rmSync(output, { recursive: true, force: true })
  }
}
