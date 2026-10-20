/// A live window draws on its next display frame, after the mutation that
/// changed it. Automation must still click and type into controls that the
/// previous click mounted, without waiting for that frame: before
/// `SoloRenderer.flush`, `click()` failed with "Element has no painted bounds"
/// and `fill()` focused nothing, so its keystrokes were lost.
import { execFile, spawn } from "node:child_process"
import { rmSync } from "node:fs"
import { basename, dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"
import { connectStdio } from "../automation.js"
import type { TreeNode } from "../automation.js"

describe.skipIf(process.platform !== "darwin")("packaged automation frames", () => {
  it("acts on controls mounted by the previous click", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "automation-frames"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    if (!executable) throw new Error("Packager did not return an executable")
    const output = dirname(dirname(dirname(dirname(executable))))
    const child = spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"] })
    let errors = ""
    child.stderr.on("data", (data) => { errors += data })
    const exited = new Promise<void>((resolve, reject) => {
      child.once("error", reject); child.once("exit", () => resolve())
    })
    void exited.catch(() => {})
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 45_000)
    try {
      const work = async () => {
        const app = await connectStdio({
          write: (chunk) => { child.stdin.write(chunk) },
          feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
          close: async () => { child.kill() },
        })
        const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
        const text = async (id: string) => flatten(await app.getByTestId(id).element())
        await expect.poll(() => text("presses")).toBe("Presses: 0")
        for (let round = 1; round <= 10; round++) {
          await app.getByTestId("toggle").click()
          // No waiting in between: each command must see the click's result.
          await app.getByTestId("mounted").click()
          await app.getByTestId("field").fill(`round ${round}`)
          await expect.poll(() => text("value")).toBe(`round ${round}`)
          await expect.poll(() => text("presses")).toBe(`Presses: ${round}`)
          await app.getByTestId("toggle").click()
          await expect.poll(() => app.getByTestId("field").count()).toBe(0)
        }
      }
      await Promise.race([work(), exited.then(() => { throw new Error(`Fixture exited: ${errors}`) })])
    } catch (error) {
      throw new Error(`Packaged automation frames failed:\n${errors}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
      await exited.catch(() => {})
      if (basename(output).startsWith("solo-commands-")) rmSync(output, { recursive: true, force: true })
    }
  }, 60_000)
})
