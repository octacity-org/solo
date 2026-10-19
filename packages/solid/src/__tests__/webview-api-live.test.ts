import { execFile } from "node:child_process"
import { fileURLToPath } from "node:url"
import { promisify } from "node:util"
import { describe, expect, it } from "vitest"

const run = promisify(execFile)
const fixture = fileURLToPath(new URL("./fixtures/webview-api-live.mjs", import.meta.url))

describe.skipIf(process.platform !== "darwin")("live macOS WebView API", () => {
  it("loads generated HTML and safely evaluates JavaScript across lifecycle changes", async () => {
    const { stdout } = await run("bun", [fixture], {
      timeout: 20_000,
      killSignal: "SIGKILL",
    }).catch((error) => {
      throw new Error(`WebView API child failed; last output:\n${error.stdout?.slice(-1000)}`, {
        cause: error,
      })
    })
    expect(stdout).toContain("webview-api:passed")
  }, 25_000)
})
