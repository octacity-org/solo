// Internal runtime bridge used by the Vite plugin. Keeping this import inside
// @solo/solid lets isolated workspace installs resolve @solo/native through
// @solo/solid's own dependencies instead of leaking it into application code.
export {
  SoloRenderer,
  addRecentDocument,
  enableWorkerMainThread,
  moveToTrash,
  openExternalUrl,
  openPath,
  readClipboardText,
  registerUrlScheme,
  requestProcessExit,
  revealPath,
  runMainLoop,
  setDockMenu,
  writeClipboardText,
} from "@solo/native"
