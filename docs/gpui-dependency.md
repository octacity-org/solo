# GPUI dependency

How GPUIX consumes GPUI, what it needs from it, and why the source is pinned
where it is.

## Dependency graph

```text
@solo/native (packages/native)
├── gpui           — rendering, elements, styles, windows   [all platforms]
├── gpui_platform  — `application()` platform constructor    [Linux/Windows run path]
└── gpui_macos     — MacPlatform::new                        [macOS only]
        ↓
pinned git revision of zed-industries/zed (upstream, no fork)
```

`gpui_platform` is kept because it is upstream's supported platform-selection
convenience (`application()` = `Application::with_platform(current_platform())`
with per-OS cfg inside). Dropping it would mean hand-rolling per-OS cfg in
GPUIX for no gain. `gpui_macos` is required directly because macOS worker mode
builds the platform itself with `MacPlatform::new(false)`, and the test
renderer uses the native Metal renderer.

## Source and revision

All three crates come from one source so Cargo resolves a single consistent
graph:

```toml
gpui          = { git = "https://github.com/zed-industries/zed", rev = "1d8a29e1e2f65b6be6c176f90af801286ac11cc4", ... }
gpui_platform = { git = "https://github.com/zed-industries/zed", rev = "<same>", ... }
gpui_macos    = { git = "https://github.com/zed-industries/zed", rev = "<same>", ... }  # macOS only
```

The exact rev is `1d8a29e1e2f65b6be6c176f90af801286ac11cc4`.

## Why no fork

Solo used to depend on `remorses/zed` (branch `gpui-macos-embedded`), whose
only change was `MacPlatform::new_embedded()` and `pump_events()`: an AppKit
loop that Node pumped from JavaScript with `tick()`. macOS worker mode (see
`AGENTS.md`) replaced that loop. The main thread runs
`gpui::Application::with_platform(...).run(...)`, GPUI's ordinary loop, and
the app runs in a Bun Worker. Nothing Solo uses is fork-only, so the crates
point at upstream. The embedded loop, and Node support on macOS with it, were
removed.

## Toolchain

`rust-toolchain.toml` pins channel `1.98.1`, matching `rust-toolchain.toml` of
the pinned Zed revision. When bumping the GPUI rev, re-check the channel.

## Accessibility is disabled on macOS

At this revision, closing a window aborts the process: `MacWindow::drop`
releases the AccessKit `SubclassingAdapter`, and restoring the view's class
corrupts the KVO state of AppKit's Touch Bar observers
([zed-industries/zed#64819](https://github.com/zed-industries/zed/issues/64819)).
Every `openWindow` close hits it. Until the fix
([zed-industries/zed#65186](https://github.com/zed-industries/zed/pull/65186))
is in the pinned revision, `run_main_loop` builds the app with
`Application::new_inaccessible`, so GPUI exposes no accessibility tree and
screen readers cannot read Solo windows. Switch back to
`Application::with_platform` when bumping past that fix.

## Bumping the pin

1. Update `rev = "..."` for all three crates in `packages/native/Cargo.toml`.
2. Match this repo's `rust-toolchain.toml` to the new revision's.
3. `cargo check --all-targets && bun run build && bun run test` everywhere.
