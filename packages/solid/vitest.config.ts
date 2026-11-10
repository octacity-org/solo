import { defineConfig } from "vitest/config"
import { dirname } from "node:path"
import { fileURLToPath } from "node:url"
import { createRequire } from "node:module"
import { solidUniversal } from "./src/vite.js"

const req = createRequire(import.meta.url)
// Resolve each package's dist directory, then point at the client dev
// build explicitly (avoids both the SSR builds and bun's isolated
// node_modules layout).
//
// @solidjs/signals is a transitive dep that may not be requirable from this
// package, so resolve it from within solid-js itself.
const solidJsEntry = req.resolve("solid-js")
const signalsReq = createRequire(solidJsEntry)
const distDir = (p: string): string => dirname(p)

export default defineConfig({
  plugins: [solidUniversal()],
  resolve: {
    // Tests run against src, not dist.
    alias: [
      {
        find: /^@solo\/solid\/runtime$/,
        replacement: fileURLToPath(new URL("./src/runtime.ts", import.meta.url)),
      },
      {
        find: /^@solo\/solid$/,
        replacement: fileURLToPath(new URL("./src/index.ts", import.meta.url)),
      },
    ],
  },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    // The mock renderer needs no DOM environment.
    environment: "node",
    // macOS suites create Metal-backed test windows and standalone .app
    // processes. Running those files concurrently starves GPUI's event pump
    // and makes otherwise unrelated hooks cross their timeout in CI.
    fileParallelism: process.platform !== "darwin",
    // Linux screenshots go through wgpu (Vulkan, lavapipe in CI): the first
    // capture compiles pipelines and takes several seconds, past vitest's 5 s
    // default.
    testTimeout: process.platform === "linux" ? 60_000 : 5_000,
    // Inline the Solid packages so solidUniversal's resolveId pinning
    // also applies to their INTERNAL imports. Externalized, they would load
    // solid-js's server build natively and lose all reactivity.
    server: {
      deps: { inline: ["solid-js", "@solidjs/signals", "@solidjs/universal"] },
    },
  },
})
