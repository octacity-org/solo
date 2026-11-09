# Linux production builds

A Solo app on Linux is one compiled executable plus its native addon. There is
no worker split (that exists only on macOS), so `@solo/solid/vite` emits a
single server-bundle entry and the addon loads in-process.

## Build

```sh
# The native addon. `automation` is on by default; pass --no-default-features
# to leave the automation server out of a shipped build.
cd packages/native
bun run build:release        # or build:release:full for automation

# The app. Vite writes one JS entry, which Bun embeds in the executable.
cd ../..
cd examples/tasks
bun run build
bun build dist/index.js --compile --outfile SoloTasks
```

`bun build --compile` embeds the `.node` addon and Bun's runtime, so the
result is a single file that can be installed anywhere. `dist/index.js` is
still needed while testing without `--compile`.

Production builds leave the stdio automation server out unless
`solidUniversal({ automation: true })` or `SOLO_AUTOMATION=1` was set: it
keeps Node's stdio streams alive, several megabytes of a compiled binary. The
addon binary is imported directly (not through napi-rs's loader) to avoid
pulling in Bun's `fs`; the generated `index.js`/`index.d.ts` are macOS
artifacts and should be restored with `git checkout` after a local Linux build.

## Install layout

```
~/.local/bin/SoloTasks                     the compiled binary
~/.local/share/applications/dev.solo.tasks.desktop
~/.local/share/icons/hicolor/256x256/apps/dev.solo.tasks.png
```

```ini
[Desktop Entry]
Type=Application
Name=Solo Tasks
Comment=Dogfood task manager
Exec=/home/me/.local/bin/SoloTasks
Icon=dev.solo.tasks
Terminal=false
Categories=Utility;
StartupWMClass=dev.solo.tasks
```

`StartupWMClass` is the window's `app_id`. GPUI's X11 backend writes it as
`WM_CLASS`, so it must match the `appId` the app passes to `render()` (or
`openWindow()`); otherwise the desktop shows a second, unnamed launcher entry.
Wayland uses the same identifier as `xdg_toplevel.app_id`. Keep it stable
across releases: desktops remember per-app data by it.

```tsx
render(() => <TasksApp />, { appId: "dev.solo.tasks", title: "Solo Tasks" })
```

Launcher actions (`.desktop` `Actions=`) are the closest Linux equivalent of
a Dock menu. Solo does not generate the file, and an action is just another
`Exec` line (for example `SoloTasks --new-task`); handling those arguments is
application code. That is deliberately left to the app rather than modelled
as a Solo API.

The executable must stay in one piece: `bun build --compile` output cannot be
split, and the addon must be the build for the target architecture
(`x86_64-unknown-linux-gnu` or `aarch64-unknown-linux-gnu`).

## Web views

A Linux app that uses `<webview>` links against GTK 3 and WebKitGTK
(libwebkit2gtk-4.1, which also pulls in JavaScriptCoreGTK). Host machines
need those runtime libraries installed; development machines need the
`-devel` packages (`webkit2gtk4.1-devel` on Fedora, `libwebkit2gtk-4.1-dev`
on Debian/Ubuntu, `webkit2gtk4.1` on Arch). See
[webview.md](webview.md#platform-support) for the X11/Wayland boundary.

## Status item

`menuBar` in `render()` needs no extra library: the item is a
StatusNotifierItem published on the session bus by `ksni`, a pure-Rust D-Bus
implementation. No GTK, libappindicator or other system package is required
on top of D-Bus itself, which a desktop session already runs.

The item can only appear through a host: KDE Plasma shows it in its panel,
XFCE through the status notifier plugin, GNOME through the AppIndicator and
KStatusNotifierItem extensions. A session without one makes `render()` throw,
because a background app whose only entry point is a hidden icon could never
be reached again. See [menu-bar-apps.md](menu-bar-apps.md).

## Recent documents

`addRecentDocument(path)` validates the absolute path and adds a bookmark to
`$XDG_DATA_HOME/recently-used.xbel` (`~/.local/share/recently-used.xbel` when
the variable is unset), following the freedesktop recent-files spec. A new
entry carries the file's MIME type and Solo's `bookmark:application`. When
another app already has a bookmark for the file, that entry is kept whole —
including its metadata for the other apps — and only its `modified` and
`visited` timestamps are bumped. The file is rewritten atomically because
desktop menus may read it at any time. A file that is not XBEL is left
untouched and the call throws.

Picking a recent document from the desktop menu launches the app with a
`file://` URL, which arrives through `onOpenUrls` — see
[deep-links.md](deep-links.md) for the single-instance and argument handling.

## Checking a build

```sh
ls -lh SoloTasks
./SoloTasks                                  # window opens
gtk-launch dev.solo.tasks                    # from the desktop entry
```

The window should group under its launcher icon, and `StartupWMClass` should
match the `WM_CLASS` reported by `xprop WM_CLASS`. Screenshots need X11;
Wayland sessions are supported for the app itself.
