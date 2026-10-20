import { execFile, spawn } from "node:child_process"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"
import { connectStdio } from "../automation.js"
import type { TreeNode } from "../automation.js"

describe.skipIf(process.platform !== "darwin")("packaged application commands", () => {
  it("dispatches across native focus targets, respects enabled/disposal, and preserves editing", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager], { timeout: 30_000 })
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
        const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
        const text = async (id: string): Promise<string> => flatten(await app.getByTestId(id).element())
        let count = 0
        for (const target of ["focus-target", "native-input", "native-textarea"]) {
          // Focus handlers are installed during paint, not retained-tree creation.
          await expect.poll(async () => (await app.getByTestId(target).bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
          await app.getByTestId(target).press("cmd-r")
          await expect.poll(() => text("count")).toBe(`Refresh: ${++count}`)
        }
        expect(await text("local")).toBe("Local keys: 0")
        await app.getByTestId("native-input").fill("hello 世界")
        await expect.poll(() => text("value")).toBe("Input: hello 世界")
        await app.getByTestId("native-input").press("tab")
        await app.getByTestId("toggle-enabled").click()
        await expect.poll(() => text("enabled")).toBe("Enabled: false")
        await app.getByTestId("focus-target").press("cmd-r")
        await expect.poll(() => text("local")).toBe("Local keys: 1")
        expect(await text("count")).toBe(`Refresh: ${count}`)
        await app.getByTestId("toggle-enabled").click()
        await app.getByTestId("toggle-registration").click()
        await expect.poll(() => text("registered")).toBe("Registered: false")
        await app.getByTestId("focus-target").press("cmd-r")
        await expect.poll(() => text("local")).toBe("Local keys: 2")
        expect(await text("count")).toBe(`Refresh: ${count}`)
        await app.getByTestId("toggle-registration").click()
        await app.getByTestId("native-textarea").press("cmd-r")
        await expect.poll(() => text("count")).toBe(`Refresh: ${++count}`)
      })()])
    } catch (error) {
      throw new Error(`Packaged command fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(timeout)
      child.kill("SIGKILL")
    }
  }, 60_000)
})

describe.skipIf(process.platform !== "darwin")("packaged background lifecycle", () => {
  it("keeps timers alive across repeated close/reopen cycles and quits explicitly", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "background-lifecycle"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 20_000)
    const exit = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once("error", reject)
      child.once("exit", (code, signal) => resolve({ code, signal }))
    })
    try {
      const app = await connectStdio({
        write: (chunk) => { child.stdin.write(chunk) },
        feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
        close: async () => { child.kill() },
      })
      const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
      const ticks = async (): Promise<number> => Number(flatten(await app.getByTestId("ticks").element()).match(/\d+/)?.[0] ?? -1)
      let previous = await ticks()
      for (let cycle = 0; cycle < 2; cycle += 1) {
        await expect.poll(async () => (await app.getByTestId("close").bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
        const marker = stderr.length
        await app.getByTestId("close").click()
        await expect.poll(() => stderr.slice(marker), { timeout: 5_000 }).toContain("background:timer-progressed")
        await expect.poll(() => stderr.slice(marker), { timeout: 5_000 }).toContain("background:window-reopened")
        await expect.poll(ticks, { timeout: 5_000 }).toBeGreaterThan(previous)
        previous = await ticks()
      }
      await app.getByTestId("quit").click()
      await expect(exit).resolves.toEqual({ code: 0, signal: null })
    } catch (error) {
      throw new Error(`Background lifecycle fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)

  it("preserves default last-window-close termination", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "background-lifecycle"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { env: { ...process.env, SOLO_BACKGROUND: "0" }, stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const exit = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once("error", reject)
      child.once("exit", (code, signal) => resolve({ code, signal }))
    })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 10_000)
    try {
      const app = await connectStdio({
        write: (chunk) => { child.stdin.write(chunk) },
        feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
        close: async () => { child.kill() },
      })
      await expect.poll(async () => (await app.getByTestId("close").bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
      await app.getByTestId("close").click()
      const result = await exit
      if (result.code !== 0 || result.signal !== null) {
        throw new Error(`Default lifecycle fixture exited unexpectedly: ${result.code ?? result.signal}\n${stderr}`)
      }
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)
})

// Keep packaged fixtures in one test file: they build the same public dist.
describe.skipIf(process.platform !== "darwin")("packaged file dialogs", () => {
  it("keeps JavaScript and automation responsive while a native dialog is open", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "file-dialogs"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 20_000)
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
        await expect.poll(async () => (await app.getByTestId("open").bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
        const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
        const ticks = async (): Promise<number> => {
          const text = flatten(await app.getByTestId("ticks").element())
          return Number(text.match(/^Ticks: (\d+)$/)?.[1] ?? -1)
        }
        const before = await ticks()
        await app.getByTestId("open").click()
        await expect.poll(async () => flatten(await app.getByTestId("status").element())).toBe("Open pending")
        await expect.poll(ticks, { timeout: 5_000, interval: 50 }).toBeGreaterThan(before)
      })()])
    } catch (error) {
      throw new Error(`Dialog fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)
})

describe.skipIf(process.platform !== "darwin")("packaged system appearance", () => {
  it("reads native appearance and keeps explicit choices separate without remounting", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "system-appearance"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 20_000)
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
        const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
        const text = async (id: string): Promise<string> => flatten(await app.getByTestId(id).element())
        await expect.poll(() => text("system")).toMatch(/^System: (light|dark)$/)
        const initial = (await text("system")).slice("System: ".length)
        expect(await text("effective")).toBe(`Effective: ${initial}`)
        for (const choice of ["dark", "light", "follow"]) {
          await expect.poll(async () => (await app.getByTestId(choice).bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
          await app.getByTestId(choice).click()
          await expect.poll(() => text("effective")).toBe(`Effective: ${choice === "follow" ? initial : choice}`)
          expect(await text("preference")).toBe(`Preference: ${choice === "follow" ? "system" : choice}`)
          expect(await text("system")).toBe(`System: ${initial}`)
          expect(await text("mount")).toBe("Mount: 1")
        }
        await app.getByTestId("editor").fill("still responsive")
      })()])
    } catch (error) {
      throw new Error(`Appearance fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)
})

describe.skipIf(process.platform !== "darwin")("packaged desktop actions", () => {
  it("exposes the native bridge and catches invalid URLs without launching another app", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "desktop"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 20_000)
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
        await expect.poll(async () => (await app.getByTestId("invalid").bounds()).height, { timeout: 5_000 }).toBeGreaterThan(0)
        await app.getByTestId("invalid").click()
        const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
        await expect.poll(async () => flatten(await app.getByTestId("status").element())).toContain("absolute HTTP/HTTPS URL")
        // Validation failures leave the application responsive.
        await app.getByTestId("editor").fill("still responsive 世界")
      })()])
    } catch (error) {
      throw new Error(`Desktop fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)
})

describe.skipIf(process.platform !== "darwin")("packaged multiple windows", () => {
  it("routes input to a second window that shares app state and closes it cleanly", async () => {
    const packager = fileURLToPath(new URL("./fixtures/package-commands.ts", import.meta.url))
    const { stdout } = await promisify(execFile)("bun", [packager, "multi-window"], { timeout: 30_000 })
    const executable = stdout.match(/^commands-executable:(.+)$/m)?.[1]
    expect(executable).toBeTruthy()
    const child = spawn(executable!, [], { stdio: ["pipe", "pipe", "pipe"] })
    let stderr = ""
    child.stderr.on("data", (chunk) => { stderr += chunk })
    const watchdog = setTimeout(() => child.kill("SIGKILL"), 30_000)
    const exit = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>((resolve, reject) => {
      child.once("error", reject)
      child.once("exit", (code, signal) => resolve({ code, signal }))
    })
    try {
      const app = await connectStdio({
        write: (chunk) => { child.stdin.write(chunk) },
        feed: (listener) => { child.stdout.on("data", (chunk) => listener(String(chunk))) },
        close: async () => { child.kill() },
      })
      const flatten = (node: TreeNode): string => (node.text ?? "") + (node.children ?? []).map(flatten).join("")
      const text = async (id: string): Promise<string> => flatten(await app.getByTestId(id).element())
      const painted = (id: string) => async () => (await app.getByTestId(id).bounds()).height
      for (let round = 0; round < 2; round += 1) {
        await expect.poll(painted("open"), { timeout: 5_000 }).toBeGreaterThan(0)
        await app.getByTestId("open").click()
        // Clicks in the inspector land in its own window and update both.
        await expect.poll(painted("inspector-increment"), { timeout: 5_000 }).toBeGreaterThan(0)
        const before = Number((await text("count")).match(/\d+/)![0])
        await app.getByTestId("inspector-increment").click()
        await expect.poll(() => text("count")).toBe(`Count: ${before + 1}`)
        expect(await text("inspector-count")).toBe(`Inspector: ${before + 1}`)
        await app.getByTestId("increment").click()
        await expect.poll(() => text("inspector-count")).toBe(`Inspector: ${before + 2}`)
        // Closing disposes the window's tree; the main window keeps running.
        const marker = stderr.length
        await app.getByTestId("inspector-close").click()
        await expect.poll(() => stderr.slice(marker), { timeout: 5_000 }).toContain("windows:inspector-closed")
        expect(await app.getByTestId("inspector-count").count()).toBe(0)
      }
      await app.getByTestId("quit").click()
      await expect(exit).resolves.toEqual({ code: 0, signal: null })
    } catch (error) {
      throw new Error(`Multi-window fixture failed. Native stderr:\n${stderr}`, { cause: error })
    } finally {
      clearTimeout(watchdog)
      child.kill("SIGKILL")
    }
  }, 60_000)
})
