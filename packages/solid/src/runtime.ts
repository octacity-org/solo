/// Solid 2 runtime for Solo.
///
/// This is the `moduleName` target for @solidjs/babel-plugin compiled with
/// `{ generate: "universal", moduleName: "@solo/solid/runtime" }`. The
/// compiler emits calls against the functions re-exported here, and each one
/// maps onto the native mutation protocol (createElement / appendChild /
/// insertBefore / setText / setStyle / setEventListener / setCustomProp /
/// commitMutations) through @solo/core's NativeRenderer interface.
///
/// Fine-grained updates stay fine-grained: a signal-driven text change only
/// produces a single `setText` op in the next batch, never a tree rebuild.

import { createRenderer } from "@solidjs/universal"
import { flush } from "solid-js"
import { For, Show, Switch, Match, Repeat, Reveal, Loading } from "solid-js"
import type { Element as SolidElement } from "solid-js"
import type { EventPayload } from "@solo/native"
import {
  attachEventHandler,
  clearEventHandlers,
  soloEventTypeForProp,
  wrapWithBatching,
} from "@solo/core"
import type { NativeRenderer, StyleDesc } from "@solo/core"
import {
  createWebviewController,
  type InternalWebviewController,
} from "./webview.js"

// ── Node bookkeeping ─────────────────────────────────────────────────

export interface SoloSolidNode {
  id: number
  /** Native element type ("div", "text", "img", ...). */
  type: string
  isText: boolean
  parent: SoloSolidNode | null
  // Children as a linked list, so sibling inserts, removals and lookups
  // are O(1) even in long lists.
  firstChild: SoloSolidNode | null
  lastChild: SoloSolidNode | null
  prev: SoloSolidNode | null
  next: SoloSolidNode | null
  /** The last style object sent to native, to skip resending equal ones. */
  style?: StyleDesc
  /** Source values used to validate URL/HTML exclusivity. */
  webviewSource?: {
    url?: string
    html?: string
    baseUrl?: string
  }
}

let idCounter = 0
const nodesById = new Map<number, SoloSolidNode>()
const webviewControllers = new Map<number, InternalWebviewController>()

/** Reset element IDs so tests are deterministic. */
export function resetIdCounter(): void {
  idCounter = 0
}

function nextId(): number {
  return ++idCounter
}

// ── Active renderer ──────────────────────────────────────────────────

let activeRenderer: NativeRenderer | null = null
let flushNow: (() => void) | null = null

/**
 * Set the renderer all ops go to. Called by render()/mount().
 * Mutations are buffered by wrapWithBatching() and flushed with one
 * applyBatch() per microtask: every queued mutation arms exactly one commit,
 * so everything a reactive transaction mutates lands in a single batch.
 */
export function setSoloRenderer(renderer: NativeRenderer): void {
  let scheduled = false
  // Both commit paths drain reactions, tear down still-detached subtrees,
  // then flush the queue in a single applyBatch.
  const runCommit = (): void => {
    // Solid 2 may defer reaction re-runs to a microtask; drain them first
    // so the batch contains every op from this transaction.
    flush()
    validateWebviewSources()
    destroyStillDetached()
    batched.commitMutations()
  }
  const batched = wrapWithBatching(renderer, {
    onQueue() {
      if (scheduled) return
      scheduled = true
      queueMicrotask(() => {
        // A flushNow since this was queued already committed.
        if (!scheduled) return
        scheduled = false
        runCommit()
      })
    },
    onDestroy: disposeNodeById,
  })
  activeRenderer = batched

  // Immediate flush bypasses (and cancels) the microtask schedule.
  flushNow = () => {
    scheduled = false
    runCommit()
  }
}

export function getSoloRenderer(): NativeRenderer {
  if (!activeRenderer) throw new Error("Solo renderer not set. Call render() first.")
  return activeRenderer
}

/** Run pending Solid reactions and flush queued mutations immediately. */
export function flushMutations(): void {
  flushNow?.()
}

// ── Renderer ops → native mutations ──────────────────────────────────

const NATIVE_TYPES = new Set([
  "div",
  "text",
  "img",
  "svg",
  "canvas",
  "input",
  "textarea",
  "anchored",
  "code",
  "diff",
  "markdown",
  "virtual-list",
  "webview",
])

function applyProps(node: SoloSolidNode, props: Record<string, unknown>): void {
  validateWebviewProps(node, props)
  for (const name of Object.keys(props)) {
    setProperty(node, name, props[name])
  }
}

function validateWebviewProps(node: SoloSolidNode, props: Record<string, unknown>): void {
  if (node.type !== "webview") return
  const url = props.url
  const html = props.html
  const baseUrl = props.baseUrl
  if (url != null && typeof url !== "string") throw new TypeError("WebView url must be a string")
  if (html != null && typeof html !== "string") throw new TypeError("WebView html must be a string")
  if (baseUrl != null && typeof baseUrl !== "string") {
    throw new TypeError("WebView baseUrl must be a string")
  }
  if (url != null && html != null) {
    throw new TypeError("WebView url and html are mutually exclusive")
  }
  if (baseUrl != null && html == null) {
    throw new TypeError("WebView baseUrl requires html")
  }
}

function validateWebviewSources(): void {
  for (const id of webviewControllers.keys()) {
    const node = nodesById.get(id)
    if (node?.webviewSource) validateWebviewProps(node, node.webviewSource)
  }
}

function unlinked(): Pick<SoloSolidNode, "parent" | "firstChild" | "lastChild" | "prev" | "next"> {
  return { parent: null, firstChild: null, lastChild: null, prev: null, next: null }
}

/** Take `node` out of its parent's child list. */
function unlink(node: SoloSolidNode): void {
  const parent = node.parent
  if (!parent) return
  if (node.prev) node.prev.next = node.next
  else parent.firstChild = node.next
  if (node.next) node.next.prev = node.prev
  else parent.lastChild = node.prev
  node.parent = node.prev = node.next = null
}

/** Put `node` into `parent`'s child list before `anchor`, or at the end. */
function link(parent: SoloSolidNode, node: SoloSolidNode, anchor: SoloSolidNode | null): void {
  node.parent = parent
  node.next = anchor
  node.prev = anchor ? anchor.prev : parent.lastChild
  if (node.prev) node.prev.next = node
  else parent.firstChild = node
  if (anchor) anchor.prev = node
  else parent.lastChild = node
}

export function createElement(tag: string, staticProps?: Record<string, unknown>): SoloSolidNode {
  const r = getSoloRenderer()
  const node: SoloSolidNode = {
    id: nextId(),
    type: NATIVE_TYPES.has(tag) ? tag : "div",
    isText: false,
    ...unlinked(),
  }
  if (staticProps) validateWebviewProps(node, staticProps)
  nodesById.set(node.id, node)
  if (node.type === "webview") {
    const controller = createWebviewController(node.id, r)
    webviewControllers.set(node.id, controller)
    // The universal Solid renderer passes the raw host node to `ref`. Keep
    // that node stable while giving WebView refs the public controller shape.
    Object.assign(node, {
      isMounted: controller.isMounted,
      ready: controller.ready,
      evaluateJavaScript: controller.evaluateJavaScript,
      allowNavigation: controller.allowNavigation,
      cancelNavigation: controller.cancelNavigation,
    })
  }
  r.createElement(node.id, node.type)
  // Solid 2 passes static (non-reactive) props at creation instead of via
  // setProp calls afterwards.
  if (staticProps) applyProps(node, staticProps)
  return node
}

export function createTextNode(value: string | number): SoloSolidNode {
  const r = getSoloRenderer()
  const node: SoloSolidNode = {
    id: nextId(),
    type: "text",
    isText: true,
    ...unlinked(),
  }
  r.createElement(node.id, "text")
  // Solid 2 passes numbers straight through; native wants strings.
  r.setText(node.id, String(value))
  return node
}

function replaceText(node: SoloSolidNode, value: string | number): void {
  getSoloRenderer().setText(node.id, String(value))
}

/**
 * Insert `node` into `parent` before `anchor` (or appended when omitted).
 * Maps to appendChild / insertBefore on the retained tree.
 */
export function insertNode(parent: SoloSolidNode, node: SoloSolidNode, anchor?: SoloSolidNode): void {
  const r = getSoloRenderer()
  detached.delete(node)
  // Detach from any previous parent first (the retained tree requires it).
  const oldParent = node.parent
  if (oldParent) {
    unlink(node)
    if (oldParent.id !== parent.id || anchor) {
      r.removeChild(oldParent.id, node.id)
    }
  }
  if (anchor != null && anchor.parent === parent) {
    link(parent, node, anchor)
    r.insertBefore(parent.id, node.id, anchor.id)
  } else {
    link(parent, node, null)
    r.appendChild(parent.id, node.id)
  }
}

/**
 * Nodes detached but not yet destroyed. The universal reconciler treats
 * removed nodes as reusable — it may re-insert the same node later within
 * the same update (e.g. keyed-list moves). Destroying eagerly would lose
 * such subtrees, so removal only detaches; nodes still detached when a
 * batch commits are destroyed just before applyBatch.
 */
const detached = new Set<SoloSolidNode>()

function removeNode(parent: SoloSolidNode, node: SoloSolidNode): void {
  const r = getSoloRenderer()
  if (node.parent === parent) unlink(node)
  r.removeChild(parent.id, node.id)
  detached.add(node)
}

function disposeNode(node: SoloSolidNode): void {
  for (let child = node.firstChild; child; child = child.next) disposeNode(child)
  webviewControllers.get(node.id)?.dispose()
  webviewControllers.delete(node.id)
  nodesById.delete(node.id)
}

function disposeNodeById(id: number): void {
  const node = nodesById.get(id)
  if (node) disposeNode(node)
}

/** Destroy every node still detached at commit time. */
function destroyStillDetached(): void {
  if (detached.size === 0) return
  const r = getSoloRenderer()
  for (const node of detached) {
    if (node.parent == null) {
      // destroyElement recurses over descendants; descendants that were
      // themselves detached are covered by their own entry or by this walk.
      r.destroyElement(node.id)
    }
  }
  detached.clear()
}

function setProperty(
  node: SoloSolidNode,
  name: string,
  value: unknown,
  prev?: unknown
): void {
  const r = getSoloRenderer()
  // `ref` is consumed by Solid's universal reconciler. It is not native
  // content and must never become a custom property on the retained element.
  if (name === "ref") return
  if (name === "style") {
    const style = value != null && typeof value === "object" ? (value as StyleDesc) : undefined
    // A new object with the same contents changes nothing natively.
    if (style && node.style && sameStyle(style, node.style)) return
    if (style) r.setStyle(node.id, style)
    else if (node.style) r.setStyle(node.id, {})
    node.style = style
    return
  }
  const eventType = soloEventTypeForProp(name)
  if (eventType) {
    attachEventHandler(r, node.id, eventType, value as ((e: EventPayload) => void) | null)
    return
  }
  if (node.type === "webview" && (name === "url" || name === "html" || name === "baseUrl")) {
    const source = node.webviewSource ?? (node.webviewSource = {})
    const normalized = value == null ? undefined : value
    if (normalized !== undefined && typeof normalized !== "string") {
      throw new TypeError(`WebView ${name} must be a string`)
    }
    const previous = source[name]
    if (normalized === undefined) delete source[name]
    else source[name] = normalized
    if (previous !== normalized) {
      webviewControllers.get(node.id)?.invalidateDocument("WebView document changed")
    }
  }
  if (value === prev) return
  r.setCustomProp(
    node.id,
    name,
    (value === undefined ? null : value) as string | object | number | boolean | null
  )
}

/** Equal keys with identical values; `hover`/`active` compare one level down. */
function sameStyle(a: StyleDesc, b: StyleDesc): boolean {
  const aKeys = Object.keys(a) as (keyof StyleDesc)[]
  if (aKeys.length !== Object.keys(b).length) return false
  for (const key of aKeys) {
    const x = a[key]
    const y = b[key]
    if (x === y) continue
    if ((key !== "hover" && key !== "active") || !x || !y) return false
    if (!sameStyle(x as StyleDesc, y as StyleDesc)) return false
  }
  return true
}

const getParentNode = (node: SoloSolidNode): SoloSolidNode | undefined => node.parent ?? undefined
const getFirstChild = (node: SoloSolidNode): SoloSolidNode | undefined => node.firstChild ?? undefined
const getNextSibling = (node: SoloSolidNode): SoloSolidNode | undefined => node.next ?? undefined

// ── Universal renderer ───────────────────────────────────────────────

export const universal = createRenderer<SoloSolidNode>({
  createElement,
  createTextNode,
  replaceText,
  isTextNode: (node) => node.isText,
  setProperty,
  insertNode,
  removeNode,
  getParentNode,
  getFirstChild,
  getNextSibling,
})

// Re-exported under the exact names @solidjs/babel-plugin emits. Compiled JSX
// does `import { createElement, insertNode, ... } from "@solo/solid/runtime"`
// — these must be named exports of this module.
export const insert = universal.insert
export const spread = universal.spread
export const setProp = universal.setProp
export const effect = universal.effect
export const memo = universal.memo
export const applyRef = universal.applyRef
export const ref = universal.ref
export const mergeProps = universal.mergeProps


export function createComponent<T>(comp: (props: T) => unknown, props: T): unknown {
  return universal.createComponent(comp as never, props)
}

// Control flow lives in core solid-js; the compiled output imports it from
// this module.
export { For, Show, Switch, Match, Repeat, Reveal, Loading }

// ── Root mounting (container-style render, no window ownership) ──────

/**
 * Mount `code` into a fresh root div on the given renderer and return a
 * disposer. Used by the public render() and by tests. The initial batch is
 * committed synchronously before returning.
 */
export function mount(
  code: () => SolidElement,
  renderer: NativeRenderer
): () => void {
  setSoloRenderer(renderer)
  const rootNode = createElement("div")
  renderer.setRoot(rootNode.id)
  const solidDispose = universal.render(code as unknown as () => SoloSolidNode, rootNode)
  const dispose = (): void => {
    // The universal disposer removes mounted nodes through our removeNode op.
    solidDispose()
    getSoloRenderer().destroyElement(rootNode.id)
    flushNow?.()
    clearEventHandlers()
  }
  flushNow?.()
  return dispose
}

/**
 * Mount `code` into a fresh parentless root div on the active renderer, for a
 * window opened with openWindow. Unlike mount(), it shares the running app's
 * renderer, IDs and event handlers. The initial batch is committed before
 * returning, so the native side can open a window on `rootId`.
 */
export function mountWindowRoot(code: () => SolidElement): {
  rootId: number
  dispose: () => void
} {
  const rootNode = createElement("div")
  const solidDispose = universal.render(code as unknown as () => SoloSolidNode, rootNode)
  flushNow?.()
  return {
    rootId: rootNode.id,
    dispose: () => {
      solidDispose()
      getSoloRenderer().destroyElement(rootNode.id)
      flushNow?.()
    },
  }
}
