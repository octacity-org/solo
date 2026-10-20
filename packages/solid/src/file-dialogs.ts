import { isAbsolute } from "node:path"
import { getSoloRenderer } from "./runtime.js"

export interface SelectFilesOptions {
  /** Allow more than one file. Defaults to false. */
  multiple?: boolean
  /** Select folders instead of files. Defaults to false. */
  directories?: boolean
  /** Native confirmation-button label, such as "Import". */
  prompt?: string
}

export interface SelectSavePathOptions {
  /** Suggested filename only, without directory components. */
  suggestedName?: string
  /** Absolute directory initially shown by the save dialog. */
  initialDirectory?: string
}

function optionalText(value: unknown, name: string): string | undefined {
  if (value === undefined) return undefined
  if (typeof value !== "string" || !value.trim()) {
    throw new TypeError(`${name} must be a non-empty string`)
  }
  return value
}

/** Select one or more existing files, or folders with `directories: true`. `null` means the user cancelled. */
export async function selectFiles(options: SelectFilesOptions = {}): Promise<string[] | null> {
  if (typeof options.multiple !== "undefined" && typeof options.multiple !== "boolean") {
    throw new TypeError("File dialog multiple must be boolean")
  }
  if (typeof options.directories !== "undefined" && typeof options.directories !== "boolean") {
    throw new TypeError("File dialog directories must be boolean")
  }
  const prompt = optionalText(options.prompt, "File dialog prompt")
  const renderer = getSoloRenderer()
  const select = renderer.selectFiles?.bind(renderer)
  if (!select) throw new Error("This renderer does not support file dialogs")
  return select(JSON.stringify({
    ...(options.multiple !== undefined ? { multiple: options.multiple } : {}),
    ...(options.directories !== undefined ? { directories: options.directories } : {}),
    ...(prompt !== undefined ? { prompt } : {}),
  }))
}

/** Choose a destination path without creating or writing it. `null` means cancellation. */
export async function selectSavePath(options: SelectSavePathOptions = {}): Promise<string | null> {
  const suggestedName = optionalText(options.suggestedName, "Suggested filename")
  if (suggestedName?.includes("/") || suggestedName?.includes("\\")) {
    throw new TypeError("Suggested filename must not contain directory components")
  }
  const initialDirectory = optionalText(options.initialDirectory, "Initial directory")
  if (initialDirectory !== undefined && !isAbsolute(initialDirectory)) {
    throw new TypeError("Initial directory must be an absolute path")
  }
  const renderer = getSoloRenderer()
  const select = renderer.selectSavePath?.bind(renderer)
  if (!select) throw new Error("This renderer does not support file dialogs")
  return select(JSON.stringify({
    ...(suggestedName !== undefined ? { suggestedName } : {}),
    ...(initialDirectory !== undefined ? { initialDirectory } : {}),
  }))
}
