/**
 * Solo Tasks — entry point. See app.tsx for the application itself.
 *
 * Run: bun run build && bun dist/index.js
 */

import { render } from "@solo/solid"
import { TasksApp } from "./app.js"

render(() => <TasksApp />, {
  // Matches the .desktop entry's StartupWMClass so the window groups under
  // its launcher icon.
  appId: "dev.solo.tasks",
  title: "Solo Tasks",
  width: 480,
  height: 640,
})
