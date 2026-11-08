// Internal runtime bridge used by the Vite plugin. Keeping this import inside
// @solo/solid lets isolated workspace installs resolve @solo/native through
// @solo/solid's own dependencies instead of leaking it into application code.
export {
  SoloRenderer,
  addRecentDocument,
  enableWorkerMainThread,
  getKeyboardLayout,
  getScreens,
  getThermalState,
  hideApplication,
  hideOtherApplications,
  moveToTrash,
  openExternalUrl,
  openPath,
  readClipboardText,
  registerUrlScheme,
  requestProcessExit,
  restartApplication,
  revealPath,
  runMainLoop,
  setDockMenu,
  shouldAutoHideScrollbars,
  showAllApplications,
  writeClipboardText,
} from "@solo/native"
