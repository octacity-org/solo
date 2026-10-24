// Solo — Solid bindings for Solo.

export { openWindow, render } from "./root.js"
export type { AppWindow, OpenWindowOptions, Root, RenderOptions } from "./root.js"
export { registerApplicationCommand } from "./commands.js"
export { configureMenuBar } from "./menu-bar.js"
export type { MenuBarConfiguration, MenuBarItem } from "./menu-bar.js"
export { createContextMenu } from "./context-menu.js"
export type { ContextMenu, ContextMenuAnchor, ContextMenuItem } from "./context-menu.js"
export type { ApplicationCommand } from "./commands.js"
export { moveToTrash, openExternalUrl, openPath, revealPath, writeClipboardText } from "./desktop.js"
export type { TrashResult } from "./desktop.js"
export { selectFiles, selectSavePath } from "./file-dialogs.js"
export type { SelectFilesOptions, SelectSavePathOptions } from "./file-dialogs.js"
export { createSystemAppearance } from "./system-appearance.js"
export type { SystemAppearance } from "./system-appearance.js"
export { View, Text, Button } from "./components.js"
export type { ViewProps, TextProps, ButtonProps } from "./components.js"
export {
  resetIdCounter,
  flushMutations,
  setSoloRenderer,
  getSoloRenderer,
} from "./runtime.js"
export type { SoloSolidNode } from "./runtime.js"
export type { StyleDesc } from "@solo/core"
export type { EventPayload } from "@solo/native"
export type {
  WebviewController,
  WebviewNavigationRequestEvent,
  WebviewValue,
} from "./webview.js"

export {
  createSolidNativeTestRoot,
  hasNativeTestRenderer,
} from "./testing.js"
export type { SolidNativeTestRoot } from "./testing.js"
