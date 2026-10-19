# Solo

Build native GPU-accelerated desktop apps with **Solid** and TypeScript.
Your components render directly to the GPU via Metal, DirectX, or Vulkan —
no Electron, no web views.

Solid is Solo's only supported frontend. Application code imports
`@solo/solid`; `@solo/core` is private and `@solo/native` is an internal
transitive dependency, not an application API.

```tsx
import { createSignal } from "solid-js"
import { render, View, Text, Button } from "@solo/solid"

function Counter() {
  const [count, setCount] = createSignal(0)
  return (
    <View style={{ display: "flex", gap: 12 }}>
      <Text>{count()}</Text>
      <Button onClick={() => setCount((v) => v + 1)}>Increment</Button>
    </View>
  )
}

render(() => <Counter />, { title: "Counter", width: 480, height: 320 })
```

## Architecture

```
TypeScript / TSX
      ↓
    Solid          ← custom renderer (@solidjs/babel-plugin, universal mode)
      ↓
@solo/core        ← private mutation kernel: types, batching, events
      ↓
@solo/native      ← internal napi-rs bridge; retained element tree in Rust
      ↓
     GPUI           ← Zed's GPU UI framework (pinned revision)
```

Solo is **description-driven**, not DOM-like. Solid never mutates a
retained JS tree: each commit queues a minimal batch of native mutations
(`createElement`, `appendChild`, `setText`, `setStyle`, `setCustomProp`,
`setEventListener`) that is flushed to Rust through a single `applyBatch`
FFI call. GPUI's retained tree owns all real state.

## Packages

| package | role |
|---|---|
| [`@solo/solid`](packages/solid) | **The public application package.** Solid components and JSX types, window `render()`, Vite integration via `@solo/solid/vite`, automation, and testing. |
| [`@solo/core`](packages/core) | Private TypeScript implementation: protocol/style types, event registry, mutation batching, frame loop, automation implementation, and mock renderer. |
| [`@solo/native`](packages/native) | Internal Rust/napi implementation and transitive dependency: retained tree, GPUI element building, text, custom elements, automation host, and GPU-backed test renderer. |

## Runtime model

- Solid compiles JSX to universal renderer ops (`createElement`,
  `insertNode`, `insert`, `setProp`, …) against `@solo/solid/runtime`.
- Every op maps onto a native mutation and is queued through
  `wrapWithBatching`; exactly one microtask-scheduled `applyBatch` flushes
  per reactive transaction. A signal-driven text change becomes a single
  `setText`.
- Removal is a *detach*: nodes are only destroyed at commit time if they
  were not reattached, so keyed list moves preserve subtrees.
- Native events arrive by element ID on the JS thread and dispatch into a
  shared handler registry (`handleGpuixEvent`).

## Native elements

`div`, `text`, `img`, `svg`, `input`, `textarea`, `anchored`, `code`
(Tree-sitter highlighting), `diff` (unified patches), `markdown`
(GFM), `virtual-list` (windowed rows), `webview` (macOS only — a real
WKWebView). Styles are a CSS-like subset
(`StyleDesc`) applied natively, including hover/active pseudo-states,
overflow scrolling, opacity, borders, and text metrics.

Nested scroll containers chain wheel input to ancestors by default. Add
`overscrollBehavior: "contain"` to keep both axes in the inner container, or
use `overscrollBehaviorX` / `overscrollBehaviorY` for one axis. Containment is
handled synchronously by the native event path, including at scroll boundaries.

## Automation

Solo ships a Playwright-style automation protocol over stdio (SSE
framing). Any app whose stdin is not a TTY serves it automatically:
tree queries by testId/text/type, click/mouse/keyboard/wheel injection,
programmatic scrolling, bounds, screenshots, clock control. Import the client
from `@solo/solid/automation`; see `examples/tasks/diagnose-scroll.mts`.

## Testing

- Headless (all platforms): `MockNativeRenderer` records every mutation op
  for protocol-level assertions. See
  `packages/solid/src/__tests__/lifecycle.test.tsx`.
- GPU-backed (macOS): `createSolidNativeTestRoot()` from
  `@solo/solid/testing` drives real frames, hit testing, selection, and
  screenshots through `TestGpuixRenderer`.

Run tests:

```bash
cd packages/solid && bun run test   # vitest
cd packages/native && cargo test --lib && cargo test --test layout_probe
```

## Examples

| example | demonstrates |
|---|---|
| `examples/tasks` | full task manager: store state, keyed list, scroll, input, automation regression suite |
| `examples/solid-counter` | minimal counter window |
| `examples/webview-preview` | macOS-only embedded WKWebView beside a fully native pane |

## Build

Requirements: Bun, Rust 1.97.1 (see `rust-toolchain.toml`), plus Xcode's
Metal Toolchain on macOS (`xcodebuild -downloadComponent MetalToolchain`).

```bash
bun install
bun run build:native        # napi release build (~minutes)
cd examples/tasks && bun run build && bun dist/index.js
```

GPUI comes from a pinned git revision of upstream `zed-industries/zed`; see
[docs/gpui-dependency.md](docs/gpui-dependency.md).

## Platform status

| platform | windows | input | notes |
|---|---|---|---|
| macOS | ✅ | ✅ incl. synthetic input & GPU test renderer | GPUI on the main thread, app in a Bun Worker (Bun required); only platform with `<webview>` |
| Linux (Wayland/X11) | ✅ | ✅ physical input; synthetic wheel via automation | blocking UI thread |
| Windows | ✅ builds | ⚠️ untested | same blocking-thread model |

## Acknowledgements

Solo originated as a fork of GPUIX and has since diverged into an independent
Solid-first native TypeScript desktop framework built on GPUI.

Inherited license notices are preserved in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) and were not replaced by the
transfer to independent ownership.
