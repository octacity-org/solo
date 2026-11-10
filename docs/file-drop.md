# Dropping files onto elements

Any element can accept files and folders dragged in from outside the app,
for example from Finder or Thunar. `onFileDrop` receives their absolute
paths, and the `fileDragOver` style highlights the element while a drag
hovers it.

```tsx
<div
  style={{
    padding: 24,
    borderWidth: 2,
    borderColor: "#45475a",
    fileDragOver: { borderColor: "#89b4fa", backgroundColor: "#1e1e2e" },
  }}
  onFileDrop={(event) => importFiles(event.paths ?? [])}
>
  Drop images here
</div>
```

## The event

- `event.paths` lists the absolute paths, in the order the platform reports
  them. Folders are included as paths; Solo does not expand, read or validate
  them. A path that is not valid UTF-8 is converted lossily.
- `event.x` and `event.y` are the drop point in window coordinates.
- The innermost element under the pointer with `onFileDrop` takes the drop.
  Its ancestors do not also receive it.
- A drop on an area without `onFileDrop` does nothing.

## The highlight

`fileDragOver` is a pseudo-style like `hover` and `active`: GPUI applies it
natively, with no JS round trip, while files hover the element, and removes it
when they are dropped or dragged away. It is applied at paint time, after
layout, so use properties that change how the element paints, such as
`backgroundColor`, `borderColor` or `opacity`. Size and spacing properties in
it have no effect.

## Platforms

macOS delivers drops through AppKit's dragging destination protocol. Linux
uses the windowing backend's own protocol: X11's XDND (GPUI's X11 client
handles `XdndEnter`/`XdndDrop` and turns the URIs into `ExternalPaths`), and
Wayland's `wl_data_device` drag-and-drop on the compositor's side. Both paths
produce the same `ExternalPaths` value, so `onFileDrop`, `event.paths` and
`fileDragOver` behave identically. Windows builds carry the same wiring but
are untested.

Drops from some file managers arrive as plain text rather than `text/uri-list`.
GPUI exposes only URI lists, so those drops are ignored; Solo does not guess
a path from arbitrary text. Dragging from the desktop's file manager is the
supported case.

## Current boundaries

- A `<webview>` is a native view composited above GPUI, so files dragged over
  it go to the web view, not to Solo. The elements underneath or around it
  receive no drop or highlight in that area.
- Only incoming drops are supported. Dragging files out of the app is not.

## Testing

`createSolidNativeTestRoot()` simulates the platform drag:
`renderer.nativeSimulateFileDragOver(paths, x, y)` holds files over a point,
and `renderer.nativeSimulateFileDrop(paths, x, y)` drops them. The live
`SoloRenderer` has the same `simulateFileDragOver` and `simulateFileDrop`
methods in automation builds, on every platform, so an automation suite can
assert the listener, paths and highlight without a real drag.

Real desktop delivery is a platform concern: verify it by dragging one file and
then several from the file manager onto a painted drop target. Under X11 the
window must accept XDND; under Wayland the compositor forwards the drag. Solo
adds no filter of its own, so a failure there is in GPUI's backend, not in the
Solid event wiring.
