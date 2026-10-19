/**
 * Solo Tasks — entry point. See app.tsx for the application itself.
 *
 * Run: bun run build && bun dist/index.js
 */

import { render } from "@solo/solid"
import { TasksApp } from "./app.js"

render(() => <TasksApp />, { title: "Solo Tasks", width: 480, height: 640 })
