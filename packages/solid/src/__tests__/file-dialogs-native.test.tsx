import { describe, expect, it } from "vitest"
import { selectFiles, selectSavePath } from "@solo/solid"
import { createSolidNativeTestRoot, hasNativeTestRenderer } from "../testing.js"

describe.skipIf(!hasNativeTestRenderer)("native file dialogs", () => {
  // The test renderer has no portal responder on Linux, so a prompt queues
  // forever instead of resolving. The Linux dialogs are exercised by the
  // packaged-automation suite over a real session (#91).
  it.skipIf(process.platform === "linux")("exposes asynchronous cancellation through the GPU-backed renderer", async () => {
    const root = createSolidNativeTestRoot()
    try {
      root.render(() => null)
      await expect(selectFiles({ multiple: true, prompt: "Import" })).resolves.toBeNull()
      await expect(selectFiles({ directories: true, prompt: "Scan" })).resolves.toBeNull()
      await expect(selectSavePath({ suggestedName: "Notes 世界.md", initialDirectory: "/tmp" })).resolves.toBeNull()
    } finally {
      root.unmount()
    }
  })
})
