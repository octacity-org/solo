/// JSX types for @solo/solid.
///
/// @solidjs/babel-plugin compiles user JSX to universal runtime calls and never
/// imports this module at runtime — it exists so TypeScript can type-check
/// Solid JSX with `"jsxImportSource": "@solo/solid"` (see the
/// solid-js/universal README). Types are deliberately permissive: the native
/// protocol accepts style objects, event handlers and custom props.

import type { Element as SolidElement } from "solid-js"
import type { StyleDesc } from "@solo/core"
import type { EventPayload } from "@solo/native"
import type {
  WebviewController,
  WebviewNavigationRequestEvent,
} from "./webview.js"

export interface SoloIntrinsicProps {
  style?: StyleDesc
  children?: unknown
  onClick?: (event: EventPayload) => void
  onMouseDown?: (event: EventPayload) => void
  onMouseUp?: (event: EventPayload) => void
  onMouseEnter?: (event: EventPayload) => void
  onMouseLeave?: (event: EventPayload) => void
  onMouseMove?: (event: EventPayload) => void
  onMouseDownOutside?: (event: EventPayload) => void
  onKeyDown?: (event: EventPayload) => void
  onKeyUp?: (event: EventPayload) => void
  onFocus?: (event: EventPayload) => void
  onBlur?: (event: EventPayload) => void
  onScroll?: (event: EventPayload) => void
  /** `<input>`/`<textarea>`: the text changed. `value` is the new text. */
  onChange?: (event: EventPayload) => void
  /** `<input>`/`<textarea>`: the text was submitted. `value` is the text. */
  onSubmit?: (event: EventPayload) => void
  onToggleFile?: (event: EventPayload) => void
  onShowMore?: (event: EventPayload) => void
  onLineClick?: (event: EventPayload) => void
  onLinkClick?: (event: EventPayload) => void
  /**
   * `<virtual-list itemCount>`: the items to mount changed, from `rangeStart`
   * up to `rangeEnd`, plus `focusedIndex` while a row has focus.
   */
  onRangeChange?: (event: EventPayload) => void
  testId?: string
  [key: string]: unknown
}

/// `<webview>` embeds a real WKWebView. macOS only — there is no WebView2 or
/// WebKitGTK implementation, so on other platforms the element renders
/// nothing. The web view is a native child view, not painted content: it
/// composites above GPUI, takes part in normal layout, and owns its own input.
export interface WebviewProps extends SoloIntrinsicProps {
  /** Absolute URL to load. Changing it navigates the existing view. */
  url?: string
  /** Generated HTML to load. Cannot be supplied together with `url`. */
  html?: string
  /** Base URL used to resolve relative links and resources in `html`. */
  baseUrl?: string
  /** Overrides WebKit's default user agent string. */
  userAgent?: string
  /** Fired when the main frame finishes loading. `value` is the URL. */
  onLoad?: (event: EventPayload) => void
  /** Fired when a navigation starts, including link clicks and redirects. */
  onNavigation?: (event: EventPayload) => void
  /**
   * Fired before a link navigation is committed. Use the controller to allow
   * or cancel it. Fragment-only requests are allowed automatically; requests
   * for a new window are cancelled automatically.
   */
  onNavigationRequest?: (event: WebviewNavigationRequestEvent) => void
  /** Fired when a load fails. `value` is `"<url> — <reason>"`. */
  onLoadError?: (event: EventPayload) => void
  /** Stable controller for the mounted WebView. */
  ref?: (controller: WebviewController) => void
}

export namespace JSX {
  // Solid's own element type, so <Show>, <For> and render() accept Solo JSX.
  export type Element = SolidElement

  export interface IntrinsicElements {
    div: SoloIntrinsicProps
    text: SoloIntrinsicProps
    img: SoloIntrinsicProps
    svg: SoloIntrinsicProps
    canvas: SoloIntrinsicProps
    input: SoloIntrinsicProps
    textarea: SoloIntrinsicProps
    anchored: SoloIntrinsicProps
    code: SoloIntrinsicProps
    diff: SoloIntrinsicProps
    markdown: SoloIntrinsicProps
    "virtual-list": SoloIntrinsicProps
    webview: WebviewProps
  }
}

// Never called at runtime — @solidjs/babel-plugin emits universal ops instead.
export function jsx(_type: string, _props: Record<string, unknown>, _key?: string): JSX.Element {
  throw new Error("jsx-runtime is types-only; compile JSX with @solidjs/babel-plugin")
}

export const jsxs = jsx

export function Fragment(_props: { children?: unknown }): JSX.Element {
  throw new Error("jsx-runtime is types-only; compile JSX with @solidjs/babel-plugin")
}
