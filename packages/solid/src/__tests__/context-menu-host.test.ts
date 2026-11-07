import { spawn } from "node:child_process"
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"
import { interactiveMenus } from "./test-utils.js"

const executable = process.env.SOLO_CONTEXT_MENU_HOST ?? fileURLToPath(new URL(
  `../../../native/solo-context-menu.darwin-${process.arch}`, import.meta.url,
))

describe.skipIf(process.platform !== "darwin")("native menu tracking", () => {
  // Scripted menus are built but never opened. On-screen menus are
  // system-wide modals that the person at the machine can dismiss or steer.
  for (const scripted of [true, false]) {
    const describeMode = scripted ? describe : describe.skipIf(!interactiveMenus)
    describeMode(scripted ? "scripted" : "on screen", () => {
      it.each([false, true])("cleans its private image after parent pipe loss (aliased TMPDIR: %s)", async (aliased) => {
        const root = mkdtempSync(join(tmpdir(), "solo-host-exit-"))
        const realTmp = join(root, "real")
        mkdirSync(realTmp)
        const runtimeTmp = aliased ? join(root, "alias") : realTmp
        if (aliased) symlinkSync(realTmp, runtimeTmp, "dir")
        const directory = mkdtempSync(join(runtimeTmp, "solo-context-menu-"))
        const staged = join(directory, "solo-context-menu")
        copyFileSync(executable, staged)
        chmodSync(staged, 0o755)
        const child = spawn(staged, [], { stdio: ["pipe", "pipe", "pipe"], env: { ...process.env, TMPDIR: runtimeTmp } })
        let output = "", errors = ""
        child.stdout.on("data", (chunk) => { output += chunk })
        child.stderr.on("data", (chunk) => { errors += chunk })
        const exited = new Promise<number | null>((resolve, reject) => {
          child.once("error", reject); child.once("exit", resolve)
        })
        void exited.catch(() => {})
        const watchdog = setTimeout(() => child.kill("SIGKILL"), 8000)
        try {
          child.stdin.write(JSON.stringify({ x: 300, y: 400, items: [{ id: "read", label: "Read" }], scripted }) + "\n")
          await expect.poll(() => output, { timeout: 5000 }).toContain('"ready":true')
          // Parent death closes both pipes, not only the child's stdin.
          child.stdout.destroy()
          child.stdin.end()
          const code = await exited
          expect(existsSync(directory)).toBe(false)
          expect(code, errors).toBe(0)
          expect(errors).not.toContain("Broken pipe")
        } finally {
          clearTimeout(watchdog)
          child.kill("SIGKILL")
          await exited.catch(() => {})
          rmSync(root, { recursive: true, force: true })
        }
      }, 10_000)

      it("navigates by keyboard, skipping disabled items and separators", async () => {
        const child = spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"] })
        let output = "", errors = ""
        child.stdout.on("data", (chunk) => { output += chunk })
        child.stderr.on("data", (chunk) => { errors += chunk })
        const exited = new Promise<number | null>((resolve, reject) => {
          child.once("error", reject); child.once("exit", resolve)
        })
        void exited.catch(() => {})
        const watchdog = setTimeout(() => child.kill("SIGKILL"), 8000)
        try {
          child.stdin.write(JSON.stringify({ x: 300, y: 400, items: [
            { id: "disabled", label: "Unavailable", disabled: true },
            { type: "separator" }, { id: "read", label: "Mark read", checked: true },
          ], scripted }) + "\n")
          await expect.poll(() => output, { timeout: 5000 }).toContain('"ready":true')
          child.stdin.write('{"key":"down"}\n{"key":"enter"}\n')
          expect(await exited, errors).toBe(0)
          expect(output.trim().split("\n").at(-1)).toBe('"read"')
        } finally { clearTimeout(watchdog); child.kill("SIGKILL") }
      }, 10_000)

      it("keeps the application event loop running and cancels when its owner disconnects", async () => {
        const child = spawn(executable, [], { stdio: ["pipe", "pipe", "pipe"] })
        let output = "", errors = "", ticks = 0
        child.stdout.on("data", (chunk) => { output += chunk })
        child.stderr.on("data", (chunk) => { errors += chunk })
        const exited = new Promise<number | null>((resolve, reject) => {
          child.once("error", reject)
          child.once("exit", resolve)
        })
        // Mark rejection handled even if startup fails before the assertions.
        void exited.catch(() => {})
        const timer = setInterval(() => { ticks++ }, 20)
        const watchdog = setTimeout(() => child.kill("SIGKILL"), 8000)
        try {
          child.stdin.write(JSON.stringify({ x: 300, y: 400, items: [
            { id: "read", label: "Mark read", checked: true },
            { type: "separator" },
            { id: "delete", label: "Delete", disabled: true },
          ], scripted }) + "\n")
          await expect.poll(() => output, { timeout: 5000 }).toContain('"ready":true')
          const before = ticks
          await expect.poll(() => ticks).toBeGreaterThan(before + 5)
          expect(child.exitCode).toBeNull()
          child.stdin.end()
          expect(await exited, errors).toBe(0)
          expect(output.trim().split("\n").at(-1)).toBe("null")
        } finally {
          clearInterval(timer)
          clearTimeout(watchdog)
          child.kill("SIGKILL")
        }
      }, 10_000)
    })
  }
})
