/**
 * Solo Solid Counter
 *
 * Same app as examples/counter.tsx, written with Solid 2 against the native
 * Solo renderer. No Rust application code — just this file plus a build
 * config that compiles Solid JSX to the native mutation protocol.
 *
 * Run:
 *   bun run build && bun dist/index.js   (or: bun run start)
 */

import { createSignal } from "solid-js"
import { render, View, Text, Button } from "@solo/solid"

function Counter() {
  const [count, setCount] = createSignal(0)
  return (
    <View
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 16,
        width: "100%",
        height: "100%",
        backgroundColor: "#1e1e2e",
      }}
    >
      <Text style={{ fontSize: 48, fontWeight: "bold", color: "#cdd6f4" }} testId="count">
        {count()}
      </Text>
      <Button onClick={() => setCount((v) => v + 1)}>
        <Text style={{ fontSize: 16, fontWeight: "bold", color: "#1e1e2e" }}>Increment</Text>
      </Button>
      <Button
        onClick={() => setCount(0)}
        style={{ backgroundColor: "#313244" }}
      >
        <Text style={{ fontSize: 14, color: "#bac2de" }}>Reset</Text>
      </Button>
    </View>
  )
}

render(
  () => (
    <View
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        width: "100%",
        height: "100%",
        backgroundColor: "#11111b",
      }}
    >
      <Counter />
    </View>
  ),
  { title: "Solo Solid Counter", width: 800, height: 600 }
)
