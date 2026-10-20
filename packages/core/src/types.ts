/// Internal protocol and style types used by the Solid renderer.
///
/// These stay independent of Solid because they describe the native mutation
/// protocol and the style vocabulary that Rust's `apply_styles` understands.

import type { WindowOptions } from "@solo/native"

export type DimensionValue = number | string

export interface MotionStyle {
  width?: number
  height?: number
  opacity?: number
  top?: number
  right?: number
  bottom?: number
  left?: number
  borderRadius?: number
}

export type MotionEase =
  | "linear"
  | "ease"
  | "easeIn"
  | "easeOut"
  | "easeInOut"
  | [number, number, number, number]

export interface MotionTransition {
  /** Duration in seconds. */
  duration?: number
  /** Delay in seconds. */
  delay?: number
  ease?: MotionEase
}

export interface MotionProps {
  initial?: MotionStyle | false
  animate: MotionStyle
  transition?: MotionTransition
}

export interface StyleDesc {
  display?: string
  visibility?: string
  flexDirection?: string
  flexWrap?: string
  flexGrow?: number
  flexShrink?: number
  flexBasis?: number
  alignItems?: string
  alignSelf?: string
  alignContent?: string
  justifyContent?: string
  gap?: number
  rowGap?: number
  columnGap?: number
  gridTemplateColumns?: number
  gridTemplateRows?: number
  gridColumnMin?: "zero" | "min-content" | "max-content"
  gridRowMin?: "zero" | "min-content" | "max-content"

  width?: DimensionValue
  height?: DimensionValue
  minWidth?: DimensionValue
  minHeight?: DimensionValue
  maxWidth?: DimensionValue
  maxHeight?: DimensionValue

  padding?: number
  paddingTop?: number
  paddingRight?: number
  paddingBottom?: number
  paddingLeft?: number

  margin?: number
  marginTop?: number
  marginRight?: number
  marginBottom?: number
  marginLeft?: number

  position?: string
  top?: number
  right?: number
  bottom?: number
  left?: number

  background?: string
  backgroundColor?: string
  color?: string
  opacity?: number

  borderWidth?: number
  borderColor?: string
  borderRadius?: number
  borderTopLeftRadius?: number
  borderTopRightRadius?: number
  borderBottomLeftRadius?: number
  borderBottomRightRadius?: number

  fontSize?: number
  fontFamily?: string
  fontWeight?: string | number
  textAlign?: string
  lineHeight?: number
  whiteSpace?: "normal" | "nowrap"
  textOverflow?: "ellipsis" | "ellipsis-start"
  lineClamp?: number

  overflow?: string
  overflowX?: string
  overflowY?: string
  /** Controls whether wheel input may continue to an ancestor scroller. */
  overscrollBehavior?: "auto" | "contain"
  overscrollBehaviorX?: "auto" | "contain"
  overscrollBehaviorY?: "auto" | "contain"

  cursor?:
    | "default" | "pointer" | "text" | "vertical-text" | "crosshair" | "grab" | "grabbing"
    | "w-resize" | "e-resize" | "ew-resize" | "n-resize" | "s-resize" | "ns-resize"
    | "nwse-resize" | "nesw-resize" | "col-resize" | "row-resize"
    | "not-allowed" | "alias" | "copy" | "context-menu"
  /** `"auto"` blocks hits behind this element. `"none"` never does. Unset blocks when the element paints a fill or is absolutely positioned. */
  pointerEvents?: "auto" | "none"

  /** "none" opts this element and its subtree out of text selection.
   *  Inherited like the CSS property, so a toolbar can disable it once. */
  userSelect?: "text" | "none" | "auto"
  /** Selection wash colour for this subtree. Defaults to the theme accent at 35%. */
  selectionColor?: string

  // Pseudo-selector styles — applied by GPUI natively (no JS round-trip).
  // Nesting is one level deep: hover/active cannot contain hover/active.
  hover?: Omit<StyleDesc, "hover" | "active">
  active?: Omit<StyleDesc, "hover" | "active">
}

// Element types supported by Solo
export type ElementType =
  | "div"
  | "text"
  | "img"
  | "svg"
  | "canvas"
  | "input"
  | "textarea"
  | "anchored"
  | "code"
  | "diff"
  | "markdown"
  | "virtual-list"

// ── Theme ────────────────────────────────────────────────────────────

/** Colours for one syntax capture class each. Every field is a CSS colour. */
export interface SyntaxTheme {
  comment?: string
  keyword?: string
  string?: string
  stringSpecial?: string
  escape?: string
  number?: string
  boolean?: string
  typeName?: string
  typeBuiltin?: string
  constructor?: string
  function?: string
  functionBuiltin?: string
  macroName?: string
  property?: string
  constant?: string
  variable?: string
  variableSpecial?: string
  parameter?: string
  operator?: string
  punctuation?: string
  tag?: string
  attribute?: string
  label?: string
  invalid?: string
}

/**
 * Every number that decides layout in the native text components.
 *
 * These live in the theme, not in Rust constants, so tuning a row height or a
 * heading scale is a re-render and needs no native rebuild.
 */
export interface SoloMetrics {
  // Code blocks
  codeTextSize?: number
  codeLineHeight?: number
  codePaddingX?: number
  codePaddingY?: number
  codeRadius?: number
  codeHeaderPaddingY?: number
  codeHeaderTextSize?: number
  codeGutterDigitWidth?: number
  codeGutterPaddingRight?: number
  codeGutterMinWidth?: number

  // Diffs
  diffTextSize?: number
  diffLineHeight?: number
  diffFileHeaderHeight?: number
  diffHunkHeaderHeight?: number
  diffNoticeHeight?: number
  diffBodyBottomPad?: number
  diffGutterWidth?: number
  diffMarkerWidth?: number
  diffAccentBarWidth?: number
  diffRowPaddingX?: number

  // Markdown
  mdTextSize?: number
  mdLineHeight?: number
  mdBlockGap?: number
  /** `[h1, h2, h3, h4to6]`. A shorter array leaves the rest at their defaults. */
  mdHeadingSizes?: number[]
  mdHeadingLineHeights?: number[]
  mdTableCellPadding?: number
  mdTableMinColumnWidth?: number
  mdTableMinColumnContent?: number
  mdInlineCodeRadius?: number
}

/**
 * Theme tokens for the native text components. Every field is optional and
 * layers on top of the built-in dark theme (or light, via `appearance`).
 */
export interface SoloTheme {
  appearance?: "dark" | "light"
  bg?: string
  border?: string
  text?: string
  textMuted?: string
  textFaint?: string
  textDim?: string
  accent?: string
  caret?: string
  codeText?: string
  codeWash?: string
  diffAdd?: string
  diffDel?: string
  diffHunkBg?: string
  fontSans?: string
  fontMono?: string
  syntax?: SyntaxTheme
  metrics?: SoloMetrics
}

export type DebugFrameOverlayMode = "hidden" | "minimal" | "full"

/// Interface for the renderer that receives mutations from a framework
/// reconciler. Implemented by the real napi SoloRenderer and by TestRenderer
/// (which delegates to native TestSoloRenderer for tests).
export interface NativeRenderer {
  /** Evaluate JavaScript in a mounted WebView and resolve serialized JSON. */
  evaluateWebviewJavaScript?(elementId: number, script: string): Promise<string>
  /** Resolve when the intended WebView document has finished loading. */
  waitWebviewReady?(elementId: number): Promise<number>
  /** Resolve a native WebView navigation request. */
  allowWebviewNavigation?(elementId: number, navigationId: number): void
  /** Cancel a native WebView navigation request. */
  cancelWebviewNavigation?(elementId: number, navigationId: number): void
  /** Show the native file-open dialog. Resolves null only when cancelled. */
  selectFiles?(optionsJson: string): Promise<string[] | null>
  /** Native context menu session; null is normal cancellation. */
  showContextMenu?(optionsJson: string): Promise<string | null>
  cancelContextMenu?(requestId: number): void
  /** Show the native save-destination dialog. Resolves null only when cancelled. */
  selectSavePath?(optionsJson: string): Promise<string | null>
  /** Replace/drop the native appearance observer and return its current snapshot. */
  setSystemAppearanceSubscription?(token: string | null): string
  /** Replace the application command descriptors atomically (native capability). */
  setApplicationCommands?(json: string): void
  /** Replace the macOS status-item presentation, or restore its defaults. */
  setMenuBar?(json: string | null): void
  createElement(id: number, elementType: string): void
  destroyElement(id: number): Array<number>
  appendChild(parentId: number, childId: number): void
  removeChild(parentId: number, childId: number): void
  insertBefore(parentId: number, childId: number, beforeId: number): void
  setStyle(id: number, styleJson: string | object): void
  setText(id: number, content: string): void
  setEventListener(id: number, eventType: string, hasHandler: boolean): void
  setRoot(id: number): void
  commitMutations(): void
  setCustomProp(id: number, key: string, valueJson: string | object | number | boolean | null): void
  /** Apply a batch of mutations in a single FFI call. Returns destroyed IDs. */
  applyBatch?(json: string): Array<number>

  // ── Focus API ──────────────────────────────────────────────────
  focusElement?(elementId: number): void
  blur?(): void

  // ── Scroll API ─────────────────────────────────────────────────
  /** Set the scroll offset of a scrollable element (overflow: "scroll").
   *  x and y are negative pixel values (scroll down = more negative y). */
  scrollTo?(elementId: number, x: number, y: number): void
  /** Scroll a child into view by its index in the children list. */
  scrollToItem?(elementId: number, index: number): void
  /** Get the current scroll offset [x, y] or null if element is not scrollable. */
  getScrollOffset?(elementId: number): Array<number> | null

  // ── Selection API ──────────────────────────────────────────────
  /** The current text selection joined in document order, or null. */
  getSelectedText?(): string | null
  /** Drop the current selection. */
  clearSelection?(): void

  // ── Window API ─────────────────────────────────────────────────
  // `windowId` is the root element of a window opened with openWindow;
  // omitted, a call acts on the main window.
  /** Open a window painting the parentless subtree under `rootId` (macOS). */
  openWindow?(rootId: number, options?: WindowOptions): void
  showWindow?(windowId?: number): void
  closeWindow?(windowId?: number): void
  setWindowMaximized?(maximized: boolean, windowId?: number): void
  isWindowMaximized?(windowId?: number): boolean
  quitApplication?(): void
  /** Content size in logical pixels. */
  getWindowSize?(windowId?: number): { width: number; height: number }
  setWindowTitle?(title: string, windowId?: number): void
  setDebugFrameOverlay?(mode: DebugFrameOverlayMode): string
  getDebugFrameOverlay?(): string
  cycleDebugFrameOverlay?(): string
  resetDebugFrameOverlayStats?(): void
}

// Container holds the renderer reference.
// Mutations go directly via napi calls.
export interface Container {
  renderer: NativeRenderer
}

// Deprecated aliases — remove in next major
export type GpuixMetrics = SoloMetrics
export type GpuixTheme = SoloTheme
export type GpuixRenderer = NativeRenderer
