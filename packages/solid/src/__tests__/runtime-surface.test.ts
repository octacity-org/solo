/// Contract test: @solidjs/babel-plugin compiles JSX to named imports from
/// "@solo/solid/runtime" (see scripts/solid-universal-plugin.ts). If any of
/// these exports disappears, every compiled app fails at import time —
/// as found by the tasks dogfood example, which resolves this module
/// through the package instead of an alias to src.
import { describe, expect, it } from "vitest"
import * as runtime from "../runtime.js"

describe("runtime module compiler surface", () => {
  it("exports every function @solidjs/babel-plugin can emit", () => {
    const required = [
      // ops emitted by { generate: "universal" }
      "createElement",
      "createTextNode",
      "insertNode",
      "insert",
      "spread",
      "setProp",
      "effect",
      "memo",
      "createComponent",
      // emitted for JSX spreads and refs
      "mergeProps",
      "ref",
      "applyRef",
      // control flow forwarded from solid-js
      "Show",
      "For",
      "Switch",
      "Match",
    ]
    for (const name of required) {
      expect(runtime, name).toHaveProperty(name)
    }
  })
})
