# AGENTS.md — Solo Codebase Guide

Solo builds native GPU-accelerated desktop apps from **TypeScript/Solid**
over [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)
(Zed's rendering framework). Solid is the only supported frontend runtime.

```
TypeScript / TSX
      ↓
    Solid          ← custom renderer (@solidjs/babel-plugin, universal mode)
      ↓
@solo/core        ← private, framework-free mutation kernel
      ↓
@solo/native      ← napi-rs bridge, retained tree in Rust
      ↓
     GPUI
```

## Package roles

- **`packages/native`** (`@solo/native`): Rust + napi-rs. Owns the retained
  element tree (`RetainedTree`), GPUI element building (`build_element`,
  `apply_styles`), text selection registry, Tree-sitter syntax
  highlighting, markdown/diff rendering, native input editors, motion,
  automation host + GPU-backed test renderer (macOS), and the frame clock.
- **`packages/core`** (`@solo/core`): private, framework-free TypeScript
  implementation. The
  mutation protocol vocabulary (`StyleDesc`, `NativeRenderer`), event-handler
  registry, `wrapWithBatching` (queues ops → one `applyBatch` FFI call),
  frame loop, event-prop mapping, `MockNativeRenderer`, and the automation
  client/protocol. Application code must not import this package directly;
  it remains framework-free as an internal architectural boundary.
- **`packages/solid`** (`@solo/solid`): Solid custom renderer. `src/runtime.ts`
  is the `moduleName` target for `@solidjs/babel-plugin`
  (`generate: "universal"`);
  every op maps onto a native mutation. Ships `View`/`Text`/`Button`
  primitives, window `render()`, `@solo/solid/automation`,
  `@solo/solid/testing`, `@solo/solid/vite`, and a types-only jsx-runtime.
  This is the only `@solo/*` package application developers import.
- React support was removed; see git history.
- Naming: packages/product are `@solo/*` / Solo; the Rust crate
  (`gpuix-native`), napi binary name and internal symbols like `GpuixView`
  still carry legacy names — renaming them is internal-only churn,
  tracked as follow-up.

## Runtime & mutation model

Description-driven, like GPUI itself:

```
signal write → Solid effects → runtime ops → batching queue
    → microtask flush → applyBatch(json) → RetainedTree → cx.notify → paint
```

- Ops: `createElement`, `appendChild`, `insertBefore`, `removeChild`,
  `setText`, `setStyle`, `setEventListener`, `setCustomProp`,
  `destroyElement`. One `applyBatch` per reactive transaction (microtask).
- Queue raw style/prop objects — do not pre-stringify (double-parse cost).
- Removal detaches; still-detached nodes are destroyed at commit so keyed
  moves can re-insert subtrees within one batch.
- Fine-grained property: one signal write must produce exactly the affected
  op(s) (e.g. one `setText`), never a tree rebuild.

## macOS worker mode

On macOS under Bun, a production build (`@solo/solid/vite` with
`build.ssr`) emits two entries: `<name>.js`, a bootstrap, and `<name>.app.js`,
the app. The bootstrap (`packages/solid/src/worker.ts`) calls
`enableWorkerMainThread()`, starts the app in a Bun web `Worker`, and enters
`runMainLoop()`, GPUI's ordinary blocking loop on the process main thread.
Under Node, and in dev/test, the bootstrap imports the app directly and the
embedded loop (`tick`) runs as before. Compile both files with
`bun build --compile`.

- The main thread never waits synchronously on the Worker. Main → Worker
  traffic is events only.
- Forwarding rule: any napi method that touches GPUI thread-locals
  (`GPUI_APP`, `GPUI_WINDOW`, `MAC_APPLICATION`, context-menu state) or needs
  `MainThreadMarker` must start with
  `if worker_main::must_forward() { return on_main(move || …) }`
  (`on_main_async` when no result is needed). Forwarded closures own their
  data; never send raw pointers.
- Exit: every way the Worker ends calls `requestProcessExit(code)` from its
  `exit` handler, which exits the process on main. An AppKit quit emits the
  element-0 `applicationQuitRequested` event; the Worker calls
  `process.exit(0)`, with a 2 s fallback in Rust.
- Use the web `Worker`, not `node:worker_threads`: the latter relays stdio
  through the main thread's event loop, which stops in `runMainLoop`.

## Layout invariants

- gpui's default `display` is Block. Any element using flexDirection/
  flexGrow MUST set `display: "flex"` explicitly (Solid users: put it in
  style). build_div forces it nowhere else.
- Scroll containers get `min-height: 0` (both axes when both scroll) so
  Taffy's automatic minimum size doesn't pin them to content height.
- The retained-tree root div is created unstyled by JS; build_div gives it
  `size_full()` so percentage heights resolve against the window.
- Nested scrolling chains to ancestors by default. Set
  `overscrollBehavior: "contain"` (or its axis-specific variant) on an inner
  scroll container when wheel input must stop there, including at a boundary.
- Every `overflow-x: scroll` uses `restrict_scroll_to_axis()` so vertical
  wheels are not remapped horizontally.
- Filled in-flow children use `block_mouse_except_scroll`; plain `occlude`
  is only for absolute/fixed overlays.

## Text

All painted strings route through `crate::text` (`selectable_text` /
`chrome_text`) so selection and test assertions can see them. Never
`div().child(string)` in new elements.

## Automation

Apps serve a Playwright-style protocol over stdio whenever stdin is not a
TTY (`serveAutomationStdio`). Methods cover tree lookup (testId/text/type),
click/mouse/keyboard/wheel injection, programmatic scrolling, bounds,
screenshots, and clock control. The public client is
`@solo/solid/automation`;
see `examples/tasks/diagnose-scroll.mts`.

Two renderers sit behind the same protocol. `TestSoloRenderer` is the
GPU-backed native test harness; `SoloRenderer` is a live application, adapted
to the same interface by `liveRendererAsTest`. Keep them symmetrical — a
method added to one is expected on the other.

Keyboard has two dispatch paths and picking the wrong one silently does
nothing:

- `Window::dispatch_keystroke` fills in `key_char` via
  `Keystroke::with_simulated_ime` and routes it to the focused element's input
  handler. **This is the only path that inserts text.** `simulateKeystrokes`
  uses it.
- A bare `KeyDownEvent` dispatched with `window.dispatch_event` fires the
  element's `onKeyDown` listener and any matching key binding, but never
  reaches the text buffer. `simulateKeyDown`/`simulateKeyUp` use it, matching
  `TestSoloRenderer`.

Both need the target focused first: an element only installs its input handler
during paint, and only while focused. `liveRendererAsTest` focuses before
dispatching.

Keystrokes are parsed on the calling thread, before the platform split, so a
malformed key reaches the controller as `Unsupported` with the offending
token rather than being swallowed by the `UiCommand` channel.

## Testing

- Headless everywhere: `MockNativeRenderer` records each mutation op —
  assert protocol traffic directly (`lifecycle.test.tsx`).
- macOS GPU-backed: `createSolidNativeTestRoot()` from `@solo/solid/testing`.
  Gate with `hasNativeTestRenderer`.
- Layout ground-truth: `packages/native/tests/layout_probe.rs` draws the
  real builder chain headlessly and asserts ScrollHandle geometry.
- Run: `bun run test` inside packages/solid or packages/core;
  `cargo test --features test-support --lib --test layout_probe` in
  packages/native (plain `cargo test` skips the macOS GPU test-renderer
  tests). Use `bun run test`, not `bun test` (vitest).

## Native child views (`<webview>`)

`<webview>` is the one element GPUI cannot paint. It is a real WKWebView, an
AppKit NSView, so it lives in the window's NSView hierarchy and composites
*above* GPUI — GPUI clears its Metal drawable opaquely every frame, so
anything below it is invisible.

```text
<webview url=…>
  → WebviewElement        CustomElement: props, events, destroy
    → WebviewPlaceholder  gpui::Element: takes layout space
      → prepaint(bounds)  authoritative post-layout bounds
        → native_view registry → WKWebView.setFrame:
```

- GPUI owns layout, AppKit owns pixels and input. Nothing polls bounds from
  JS: `prepaint` is inside the redraw that a resize triggers, so reflow is
  automatic.
- The host NSView comes from the public `Window::window_handle()`
  (`raw-window-handle`), not from GPUI's private `MacWindow` — no fork change.
- GPUI measures from the top-left, AppKit from the bottom-left; both in
  logical points. Every frame goes through `to_native_frame`, which is unit
  tested in `packages/native/src/native_view.rs`.
- macOS only. There is no WebView2 or WebKitGTK implementation, so the factory
  is `cfg`-gated and off macOS the type is simply not registered.
- New child views implement `NativeViewInstance` and are keyed by Solo element
  ID, so `destroy` (unmount) and `destroy_all` (app quit) are the only two
  cleanup paths.

## Native layout gotchas (each cost us a bug)

- Percentage heights need a *definite* parent height chain up to the window;
  build_div sizes the retained root for this.
- Flex items cannot shrink below content without explicit min-size 0.
- `.on_scroll_wheel` listeners see every wheel event in the window — always
  position-check before consuming/stopping propagation.
- A zero-area frame still keeps a WKWebView's web process alive, so
  `update_frame` hides the view instead of leaving it mounted.

## Iterating on the Rust side

No hot reload: `require()` of a `.node` dlopens permanently; the UI thread,
GPU device and registries live in its thread-locals. Use
`bun scripts/dev.ts` to rebuild and rerun screenshot tests (~4 s to fresh
PNGs on macOS). After a native rebuild restart the app.

Cargo treats git-checkout dependencies as immutable — after editing a
checked-out crate (e.g. temporary gpui tracing) clear
`target/release/.fingerprint/<crate>-*` or nothing rebuilds.

## GPUI dependency

Pinned git revision of `remorses/zed` (branch `gpui-macos-embedded`),
same rev for `gpui`, `gpui_platform`, `gpui_macos`. The fork exists only
for the embedded macOS event loop. See docs/gpui-dependency.md.
`rust-toolchain.toml` must match the revision's channel.

## Auto-generated files

Never edit by hand (regenerated by napi builds):
`packages/native/index.d.ts`, `index.js`, `*.node`. Note: Linux/macOS
builds generate different declarations (TestGpuixRenderer is
macOS-gated); restore them with `git checkout` if a local Linux build
rewrites them.

## Releasing

There is no automated release path: CI builds and tests only, and the
packages are not published anywhere. Public package manifests carry a
`prepublishOnly` guard that aborts a local `npm publish`, while `@solo/core`
is explicitly private, so
releasing takes a deliberate decision rather than an accidental version
bump. When publishing returns, stand up a versioning workflow
(changesets or equivalent) in the same change — `CHANGELOG.md` is
frozen at 0.4.0 and still documents the removed `@solo/react`
package.

## Examples

- `examples/tasks` — dogfood task manager: store state, keyed list,
  scrolling, input composer, automation regression suite, diagnose driver.
- `examples/solid-counter` — minimal counter window.
- `examples/webview-preview` — macOS-only embedded WKWebView beside a fully
  native pane, driven by the three `<webview>` events.

## Contributing

1. Rust changes: `packages/native/src/`.
2. TS changes: `packages/core` (private kernel) or `packages/solid` (public
   Solid API and renderer).
3. Keep `@solo/core` free of framework imports and out of application code.
