import { execFile, spawn } from "node:child_process"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"
import { connectStdio } from "../automation.js"

describe.skipIf(process.platform !== "darwin")("packaged Keychain credentials", () => {
  it("writes, replaces, isolates, reads and deletes a Keychain item", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "credentials"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const timeout = setTimeout(() => child.kill("SIGKILL"), 20_000)
    const exited = new Promise<never>((_resolve, reject) => {
      child.once("error", reject)
      child.once("exit", (code, signal) => reject(new Error(`Fixture exited: ${code ?? signal}`)))
    })
    try {
      await Promise.race([exited, (async () => {
        const app = await connectStdio({
          write: (chunk) => { child.stdin.write(chunk) },
          feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
          close: async () => { child.kill() },
        })
        await expect.poll(() => app.getByTestId("result").textContent(), { timeout: 10_000 }).not.toBe("pending")
        expect(await app.getByTestId("result").textContent()).toBe(
          "missing=null written=alice:true replaced=bob:true isolated=null deleted=true gone=null again=false",
        )
      })()])
    } catch (error) {
      throw new Error(`Packaged credentials fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(timeout)
      child.kill("SIGKILL")
    }
  }, 60_000)
})
