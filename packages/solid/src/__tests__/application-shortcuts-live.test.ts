import { execFile, spawn } from "node:child_process"
import { rmSync } from "node:fs"
import { basename, dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"
import { connectStdio } from "../automation.js"

describe.skipIf(process.platform !== "darwin")("packaged focus-aware shortcuts", () => {
  it("dispatches scoped chords, protects editing and controls, and handles repeats and disposal", async () => {
    const systemUiTimeout = 10_000
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "application-shortcuts"], { timeout: 30_000 })
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
        const text = (id: string) => app.getByTestId(id).textContent()
        await expect.poll(() => text("validation")).toBe("Rejected: 2")
        await app.call("keyDown", { key: "cmd-r" })
        const expected: string[] = ["global"]
        await expect.poll(() => text("log")).toBe(expected.join(","))
        for (const [key, result] of [["space", "space"], ["s", "s"], ["h", "h"], ["o", "o"], ["alt-cmd-p", "cmd-option-p"]]) {
          await app.getByTestId("article").press(key)
          expected.push(result)
          await expect.poll(() => text("log")).toBe(expected.join(","))
        }
        const article = (await app.getByTestId("article").element()).id
        await app.call("keyDown", { elementId: article, key: "s", isHeld: true })
        await app.call("keyDown", { elementId: article, key: "cmd-r", isHeld: true })
        await app.call("keyDown", { elementId: article, key: "j", isHeld: true })
        expected.push("j")
        await expect.poll(() => text("log")).toBe(expected.join(","))
        await app.getByTestId("outside").press("s alt-cmd-p")
        await app.getByTestId("outside").press("u")
        expected.push("u")
        await expect.poll(() => text("log")).toBe(expected.join(","))
        await app.getByTestId("input").fill("s h o u 世界")
        await expect.poll(() => text("value")).toBe("s h o u 世界")
        await app.getByTestId("textarea").fill("s h o\n世界")
        await app.getByTestId("input").press("alt-cmd-p")
        expect(await text("log")).toBe(expected.join(","))
        const button = (await app.getByTestId("button").element()).id
        await app.call("keyDown", { elementId: button, key: "space" })
        await app.call("keyDown", { elementId: button, key: "space", isHeld: true })
        await app.call("keyUp", { elementId: button, key: "space" })
        expected.push("button")
        await expect.poll(() => text("log")).toBe(expected.join(","))
        for (const toggle of ["enabled", "registered"]) {
          await app.getByTestId(toggle).click()
          await expect.poll(() => text(toggle)).toBe("false")
          await app.getByTestId("article").press("s")
          expect(await text("log")).toBe(expected.join(","))
          await app.getByTestId(toggle).click()
          await expect.poll(() => text(toggle)).toBe("true")
        }
        await app.getByTestId("article").press("s")
        expected.push("s")
        await expect.poll(() => text("log")).toBe(expected.join(","))
        for (const editor of ["INPUT", "DIV"]) {
          await app.getByTestId("web-focus").click()
          await expect.poll(() => text("web-status"), { timeout: systemUiTimeout }).toBe(editor)
          // No elementId: retain AppKit's actual WebView first responder. The
          // last GPUI focus remains inside the article scope deliberately.
          await app.call("keyDown", { key: "s" })
          await app.call("keyDown", { key: "u" })
          await app.call("keyDown", { key: "alt-cmd-p" })
          expect(await text("log")).toBe(expected.join(","))
        }
      }
      await Promise.race([work(), exited.then(() => { throw new Error(`Fixture exited: ${errors}`) })])
    } catch (error) {
      throw new Error(`Packaged shortcuts failed:\n${errors}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
      await exited.catch(() => {})
      if (basename(output).startsWith("solo-commands-")) rmSync(output, { recursive: true, force: true })
    }
  }, 60_000)
})
