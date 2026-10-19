# GPUI dependency

How GPUIX consumes GPUI, what it needs from it, and why the source is pinned
where it is.

## Dependency graph

```text
@solo/native (packages/native)
├── gpui           — rendering, elements, styles, windows   [all platforms]
├── gpui_platform  — `application()` platform constructor    [Linux/Windows run path]
└── gpui_macos     — MacPlatform::new_embedded / new         [macOS only]
        ↓
pinned git revision of remorses/zed (branch gpui-macos-embedded)
```

`gpui_platform` is kept because it is upstream's supported platform-selection
convenience (`application()` = `Application::with_platform(current_platform())`
with per-OS cfg inside). Dropping it would mean hand-rolling per-OS cfg in
GPUIX for no gain. `gpui_macos` is required directly because GPUIX drives the
macOS event loop itself from Node via `MacPlatform::new_embedded()`, which is
fork functionality that plain `gpui` does not expose.

## Source and revision

All three crates come from one source so Cargo resolves a single consistent
graph:

```toml
gpui          = { git = "https://github.com/remorses/zed", rev = "db0820f6756b9d789707a3de01cee72ff5251941", ... }
gpui_platform = { git = "https://github.com/remorses/zed", rev = "<same>", ... }
gpui_macos    = { git = "https://github.com/remorses/zed", rev = "<same>", ... }  # macOS only
```

The exact rev is `db0820f6756b9d789707a3de01cee72ff5251941`.

## Embedded pump wake boundary

Solo wraps the pinned `MacPlatform::pump_events()` in
`packages/native/src/macos_event_pump.rs`. The fork stops AppKit from an
`AfterWaiting` observer, but its pre-posted wake event can be consumed before
the run loop sleeps. With no further input or scheduled frame, a synchronous
`tick()` can then strand JavaScript inside `NSApplication.run` (issue #6).

The wrapper installs a scoped `BeforeWaiting` observer that queues a run-loop
wake at the actual sleep boundary. GPUI still handles the after-wake phase,
frame callbacks, termination, and its embedded lifecycle. The observer is
invalidated as soon as the pump returns; it never runs between JavaScript
ticks. Do not replace it with an early stop that prevents frame callbacks.

The Solid suite includes bounded child-process regressions for idle ticks,
repeated input, painting, and WKWebView creation/destruction. The watchdog is
outside the native process so a blocked `tick()` cannot freeze the test runner.

## Worker mode

macOS worker mode (see `AGENTS.md`) needs no fork API: it builds the platform
with upstream `MacPlatform::new(false)` and runs
`gpui::Application::with_platform(...).run(...)` on the main thread. Only the
embedded `tick` path depends on `new_embedded`.

## Fork vs upstream

The fork branch is upstream `zed-industries/zed@fd82517a11` plus four commits:

| commit       | change                                            | needed by GPUIX |
| ------------ | ------------------------------------------------- | --------------- |
| `ad78f63f65` | gpui_macos: embedded macOS event loop (`new_embedded`, pump-based AppKit pumping on Node's main thread) | yes — production window loop on macOS |
| `8a43731222` | gpui_macos: unregister embedded AppKit pointers on drop (leak fix for the above) | yes |
| `e38ae4948e` / `db0820f675` | README review-marker churn | no |

`crates/gpui` is **bit-identical** to upstream at `fd82517a11`; every fork-only
line lives in `crates/gpui_macos/src/platform.rs`. If upstream ever grows an
equivalent embedding API, the git rev can be moved to a plain upstream revision
and the fork retired.

## Toolchain

`rust-toolchain.toml` pins channel `1.97.1`, matching `rust-toolchain.toml` of
the pinned Zed revision. When bumping the GPUI rev, re-check the channel.

## Bumping the pin

1. Merge upstream Zed into the `gpui-macos-embedded` branch of `remorses/zed`
   and rebase the two `gpui_macos` embedding commits.
2. Update `rev = "..."` in `packages/native/Cargo.toml`.
3. Match this repo's `rust-toolchain.toml` to the new revision's.
4. `cargo check --all-targets && bun run build && bun run test` everywhere.
