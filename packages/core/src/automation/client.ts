/// Playwright-like automation client for Solo.
///
/// In-process tests talk to TestRenderer through the same typed method catalog
/// as a live app on SSE stdin/stdout. Locators query the retained tree.

import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process"
import { mkdir } from "node:fs/promises"
import path from "node:path"
import {
  AutomationError,
  createSseDecoder,
  encodeSse,
  methods,
  parseResponse,
  parseWireMessage,
  PROTOCOL_VERSION,
  type AutomationRequest,
  type AutomationResponse,
  type AutomationServerEvent,
  type ElementBounds,
  type MethodName,
  type ParamsOf,
  type ResultOf,
  type TreeNode,
} from "./protocol.js"

export interface AutomationBackend {
  call<M extends MethodName>(method: M, params: ParamsOf<M>): Promise<ResultOf<M>>
  close(): Promise<void>
}

/**
 * Coordinate input is window-local. `elementId`, when given, picks the window
 * that element is painted in; renderers with one window ignore it.
 */
export interface TestAutomationRenderer {
  nativeSimulateClick(x: number, y: number, elementId?: number): void
  nativeSimulateMouseDown(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseUp(x: number, y: number, button?: number, elementId?: number): void
  nativeSimulateMouseMove(x: number, y: number, pressedButton?: number, elementId?: number): void
  nativeSimulateScrollWheel(
    x: number,
    y: number,
    deltaX: number,
    deltaY: number,
    elementId?: number
  ): void
  simulateKeystrokes(keystrokes: string): void
  nativeSimulateKeystrokes(elementId: number, keystrokes: string): void
  nativeSimulateKeyDown(
    elementId: number,
    keystroke: string,
    isHeld?: boolean
  ): void
  nativeSimulateKeyUp(elementId: number, keystroke: string): void
  scrollTo(elementId: number, x: number, y: number): void
  getScrollOffset(elementId: number): [number, number] | null
  getAllText(): string[]
  getPaintedText(): string[]
  getSelectedText(): string | null
  clearSelection(): void
  captureScreenshot(path: string): void
  getAutomationTree(): string
  /** JSON array of the openWindow windows' trees, where supported. */
  getWindowAutomationTrees?(): string
  getElementBounds(elementId: number): number[] | null
  clockPause(): number
  clockSet(nowMs: number): number
  clockFastForward(deltaMs: number): number
  clockResume(): number
  focusElement?(elementId: number): void
  blur?(): void
}

type HandlerMap = {
  [M in MethodName]: (params: ParamsOf<M>) => ResultOf<M> | Promise<ResultOf<M>>
}

export class InProcessBackend implements AutomationBackend {
  constructor(private readonly renderer: TestAutomationRenderer) {}

  async call<M extends MethodName>(
    method: M,
    params: ParamsOf<M>
  ): Promise<ResultOf<M>> {
    methods[method].params.parse(params)
    const result = await this.handlers[method](params as never)
    return methods[method].result.parse(result) as ResultOf<M>
  }

  async close(): Promise<void> {}

  private readonly handlers: HandlerMap = {
    initialize: () => ({
      protocolVersion: PROTOCOL_VERSION,
      pid: process.pid,
      capabilities: ["input", "screenshot", "clock", "tree"],
      window: { width: 800, height: 600 },
    }),
    cancel: () => ({ ok: true as const }),
    click: (params) => {
      this.renderer.nativeSimulateClick(params.x, params.y, params.elementId)
      return { ok: true as const }
    },
    mouseDown: (params) => {
      this.renderer.nativeSimulateMouseDown(
        params.x,
        params.y,
        params.button,
        params.elementId
      )
      return { ok: true as const }
    },
    mouseUp: (params) => {
      this.renderer.nativeSimulateMouseUp(
        params.x,
        params.y,
        params.button,
        params.elementId
      )
      return { ok: true as const }
    },
    mouseMove: (params) => {
      this.renderer.nativeSimulateMouseMove(
        params.x,
        params.y,
        params.pressedButton,
        params.elementId
      )
      return { ok: true as const }
    },
    scrollWheel: (params) => {
      this.renderer.nativeSimulateScrollWheel(
        params.x,
        params.y,
        params.deltaX,
        params.deltaY,
        params.elementId
      )
      return { ok: true as const }
    },
    keystrokes: (params) => {
      if (params.elementId == null) {
        this.renderer.simulateKeystrokes(params.keys)
      } else {
        this.renderer.nativeSimulateKeystrokes(params.elementId, params.keys)
      }
      return { ok: true as const }
    },
    keyDown: (params) => {
      this.renderer.nativeSimulateKeyDown(
        params.elementId ?? 0,
        params.key,
        params.isHeld
      )
      return { ok: true as const }
    },
    keyUp: (params) => {
      this.renderer.nativeSimulateKeyUp(params.elementId ?? 0, params.key)
      return { ok: true as const }
    },
    focus: (params) => {
      this.renderer.focusElement?.(params.elementId)
      return { ok: true as const }
    },
    blur: () => {
      this.renderer.blur?.()
      return { ok: true as const }
    },
    scrollTo: (params) => {
      this.renderer.scrollTo(params.elementId, params.x, params.y)
      return { ok: true as const }
    },
    getScrollOffset: (params) => ({
      offset: this.renderer.getScrollOffset(params.elementId),
    }),
    getTree: () => {
      const raw = JSON.parse(this.renderer.getAutomationTree()) as unknown
      const windows = this.renderer.getWindowAutomationTrees
        ? (JSON.parse(this.renderer.getWindowAutomationTrees()) as TreeNode[])
        : []
      return {
        tree: raw === null ? null : (raw as TreeNode),
        ...(windows.length > 0 ? { windows } : {}),
      }
    },
    getPaintedText: () => ({ text: this.renderer.getPaintedText() }),
    getAllText: () => ({ text: this.renderer.getAllText() }),
    getBounds: (params) => {
      const rect = this.renderer.getElementBounds(params.elementId)
      if (!rect) return { bounds: null }
      return {
        bounds: { x: rect[0], y: rect[1], width: rect[2], height: rect[3] },
      }
    },
    getSelectedText: () => ({ text: this.renderer.getSelectedText() }),
    clearSelection: () => {
      this.renderer.clearSelection()
      return { ok: true as const }
    },
    screenshot: (params) => {
      this.renderer.captureScreenshot(params.path)
      return { path: params.path }
    },
    clockPause: () => ({ nowMs: this.renderer.clockPause() }),
    clockSet: (params) => ({ nowMs: this.renderer.clockSet(params.nowMs) }),
    clockFastForward: (params) => ({
      nowMs: this.renderer.clockFastForward(params.deltaMs),
    }),
    clockResume: () => ({ nowMs: this.renderer.clockResume() }),
  }
}

export class SseBackend implements AutomationBackend {
  private nextId = 1
  private readonly pending = new Map<
    number,
    {
      resolve: (response: AutomationResponse) => void
      reject: (error: unknown) => void
    }
  >()

  constructor(
    private readonly write: (chunk: string) => void,
    feed: (listener: (chunk: string) => void) => void,
    private readonly onClose?: () => Promise<void>
  ) {
    const decoder = createSseDecoder((message) => {
      if ("method" in message) return
      if ("event" in message) return
      const waiter = this.pending.get(message.id)
      if (!waiter) return
      this.pending.delete(message.id)
      waiter.resolve(message)
    })
    feed((chunk) => decoder.feed(chunk))
  }

  async call<M extends MethodName>(
    method: M,
    params: ParamsOf<M>
  ): Promise<ResultOf<M>> {
    methods[method].params.parse(params)
    const id = this.nextId++
    const request = { id, method, params } as AutomationRequest
    const response = await new Promise<AutomationResponse>((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      this.write(encodeSse(request))
    })
    if ("error" in response) {
      throw new AutomationError(
        response.error.code,
        response.error.message,
        response.error.data
      )
    }
    return methods[method].result.parse(response.result) as ResultOf<M>
  }

  async close(): Promise<void> {
    for (const [id, waiter] of this.pending) {
      waiter.reject(new AutomationError("Closed", `Request ${id} cancelled`))
    }
    this.pending.clear()
    await this.onClose?.()
  }
}

function toKeystrokes(text: string): string {
  return [...text]
    .map((ch) => {
      if (ch === " ") return "space"
      if (ch === "\n") return "enter"
      if (ch === "\t") return "tab"
      return ch
    })
    .join(" ")
}

interface Selector {
  testId?: string
  text?: string
  type?: string
  parent?: Selector
}

function matches(node: TreeNode, selector: Selector): boolean {
  if (selector.testId != null && node.testId !== selector.testId) return false
  if (selector.type != null && node.type !== selector.type) return false
  if (selector.text != null && !(node.text ?? "").includes(selector.text)) {
    return false
  }
  return true
}

function collect(node: TreeNode | null, selector: Selector): TreeNode[] {
  if (!node) return []
  const roots = selector.parent
    ? collect(node, selector.parent)
    : [node]
  const found: TreeNode[] = []
  const walk = (current: TreeNode) => {
    if (matches(current, selector)) found.push(current)
    for (const child of current.children ?? []) walk(child)
  }
  for (const root of roots) {
    if (selector.parent) {
      for (const child of root.children ?? []) walk(child)
    } else {
      walk(root)
    }
  }
  return found
}

export class Locator {
  constructor(
    private readonly app: App,
    private readonly selector: Selector
  ) {}

  getByTestId(testId: string): Locator {
    return new Locator(this.app, { testId, parent: this.selector })
  }

  getByText(text: string): Locator {
    return new Locator(this.app, { text, parent: this.selector })
  }

  getByType(type: string): Locator {
    return new Locator(this.app, { type, parent: this.selector })
  }

  async all(): Promise<TreeNode[]> {
    const { tree, windows = [] } = await this.app.call("getTree", {})
    return [tree, ...windows].flatMap((root) => collect(root, this.selector))
  }

  async count(): Promise<number> {
    return (await this.all()).length
  }

  async element(): Promise<TreeNode> {
    const found = await this.all()
    if (found.length === 0) {
      throw new AutomationError("NotFound", "Locator did not match any element")
    }
    if (found.length > 1) {
      throw new AutomationError(
        "Ambiguous",
        `Locator matched ${found.length} elements`
      )
    }
    return found[0]
  }

  async bounds(): Promise<ElementBounds> {
    const node = await this.element()
    if (node.bounds) return node.bounds
    const { bounds } = await this.app.call("getBounds", { elementId: node.id })
    if (!bounds) {
      throw new AutomationError("NotFound", "Element has no painted bounds")
    }
    return bounds
  }

  async click(): Promise<void> {
    const bounds = await this.bounds()
    const node = await this.element()
    await this.app.call("click", {
      x: bounds.x + bounds.width / 2,
      y: bounds.y + bounds.height / 2,
      elementId: node.id,
    })
  }

  async fill(text: string): Promise<void> {
    const node = await this.element()
    const selectAll = process.platform === "darwin" ? "cmd-a" : "ctrl-a"
    const replacement = text.length === 0 ? "backspace" : toKeystrokes(text)
    await this.app.call("keystrokes", {
      elementId: node.id,
      keys: `${selectAll} ${replacement}`,
    })
  }

  async press(key: string): Promise<void> {
    const node = await this.element()
    await this.app.call("keystrokes", {
      elementId: node.id,
      keys: key,
    })
  }

  /** The node's own text followed by its descendants' text, in document
   *  order, as DOM and Playwright `textContent` do. */
  async textContent(): Promise<string> {
    const text = (node: TreeNode): string =>
      (node.text ?? "") + (node.children ?? []).map(text).join("")
    return text(await this.element())
  }

  async waitFor(options: { timeoutMs?: number } = {}): Promise<TreeNode> {
    const timeoutMs = options.timeoutMs ?? 5000
    const started = Date.now()
    for (;;) {
      const found = await this.all()
      if (found.length === 1) return found[0]
      if (Date.now() - started >= timeoutMs) {
        throw new AutomationError(
          found.length === 0 ? "Timeout" : "Ambiguous",
          `waitFor timed out after ${timeoutMs}ms`
        )
      }
      await new Promise((resolve) => setTimeout(resolve, 16))
    }
  }
}

export class App {
  readonly clock: {
    pause: () => Promise<number>
    set: (nowMs: number) => Promise<number>
    fastForward: (deltaMs: number) => Promise<number>
    resume: () => Promise<number>
  }

  constructor(private readonly backend: AutomationBackend) {
    this.clock = {
      pause: async () => (await this.call("clockPause", {})).nowMs,
      set: async (nowMs) => (await this.call("clockSet", { nowMs })).nowMs,
      fastForward: async (deltaMs) =>
        (await this.call("clockFastForward", { deltaMs })).nowMs,
      resume: async () => (await this.call("clockResume", {})).nowMs,
    }
  }

  call<M extends MethodName>(
    method: M,
    params: ParamsOf<M>
  ): Promise<ResultOf<M>> {
    return this.backend.call(method, params)
  }

  getByTestId(testId: string): Locator {
    return new Locator(this, { testId })
  }

  getByText(text: string): Locator {
    return new Locator(this, { text })
  }

  getByType(type: string): Locator {
    return new Locator(this, { type })
  }

  async screenshot(options: { path: string }): Promise<string> {
    const { path: saved } = await this.call("screenshot", { path: options.path })
    return saved
  }

  async captureFrames(
    dir: string,
    timesMs: readonly number[]
  ): Promise<string[]> {
    await mkdir(dir, { recursive: true })
    await this.clock.pause()
    const paths: string[] = []
    for (const nowMs of timesMs) {
      await this.clock.set(nowMs)
      const file = path.join(dir, `t${nowMs}.png`)
      await this.screenshot({ path: file })
      paths.push(file)
    }
    return paths
  }

  async close(): Promise<void> {
    await this.backend.close()
  }
}

/** `elementId` arguments pick the window that element is painted in. */
export interface LiveAutomationRenderer {
  simulateClick(x: number, y: number, button?: number, elementId?: number): void
  simulateMouseDown(x: number, y: number, button?: number, elementId?: number): void
  simulateMouseUp(x: number, y: number, button?: number, elementId?: number): void
  simulateMouseMove(x: number, y: number, pressedButton?: number, elementId?: number): void
  /** Inject a wheel event; supported where the native runtime implements it. */
  simulateScrollWheel?(
    x: number,
    y: number,
    deltaX: number,
    deltaY: number,
    elementId?: number
  ): void
  /**
   * Inject key events; supported where the native runtime implements them.
   * Optional so a renderer built without them degrades to `Unsupported`
   * instead of failing to satisfy the interface.
   */
  simulateKeystrokes?(keystrokes: string, elementId?: number): void
  simulateKeyDown?(keystroke: string, isHeld?: boolean, elementId?: number): void
  simulateKeyUp?(keystroke: string, elementId?: number): void
  focusElement(elementId: number): void
  blur(): void
  scrollTo(elementId: number, x: number, y: number): void
  getScrollOffset(elementId: number): number[] | null
  getAllText(): string[]
  getPaintedText(): string[]
  getSelectedText(): string | null
  clearSelection(): void
  captureScreenshot(path: string): void
  getAutomationTree(): string
  getWindowAutomationTrees?(): string
  getElementBounds(elementId: number): number[] | null
  clockPause(): number
  clockSet(nowMs: number): number
  clockFastForward(deltaMs: number): number
  clockResume(): number
  /** Draw pending changes now; see `liveRendererAsTest`. */
  flush?(): void
}

export function liveRendererAsTest(
  renderer: LiveAutomationRenderer
): TestAutomationRenderer {
  /** A renderer built without keyboard support at all. */
  const noKeyboard = (): AutomationError =>
    new AutomationError(
      "Unsupported",
      "keyboard is not implemented by this platform's renderer"
    )

  /**
   * A keyboard op that exists but failed in the native layer.
   *
   * On macOS that is real news — the method is compiled in, so a throw is a
   * genuine failure and is passed through untouched. Off macOS the renderer is
   * built without these methods and the plain Error would otherwise reach the
   * controller as `Protocol` noise instead of a capability gap.
   */
  const keyboardFailure = (op: string, error: unknown): AutomationError => {
    if (process.platform === "darwin") throw error
    return new AutomationError(
      "Unsupported",
      `${op}: ${error instanceof Error ? error.message : String(error)}`
    )
  }

  /**
   * Draw the latest committed tree before acting on or reading the window.
   * Input is hit-tested and bounds are read against the last drawn frame,
   * and focus handles exist only after a render, while a live window draws on
   * its next display frame. Mirrors `native.flush()` in
   * packages/solid/src/testing.ts.
   */
  const flush = (): void => renderer.flush?.()

  return {
    nativeSimulateClick(x, y, elementId) {
      flush()
      renderer.simulateClick(x, y, undefined, elementId)
    },
    nativeSimulateMouseDown(x, y, button, elementId) {
      flush()
      renderer.simulateMouseDown(x, y, button, elementId)
    },
    nativeSimulateMouseUp(x, y, button, elementId) {
      flush()
      renderer.simulateMouseUp(x, y, button, elementId)
    },
    nativeSimulateMouseMove(x, y, pressedButton, elementId) {
      flush()
      renderer.simulateMouseMove(x, y, pressedButton, elementId)
    },
    nativeSimulateScrollWheel(x, y, deltaX, deltaY, elementId) {
      flush()
      if (!renderer.simulateScrollWheel) {
        throw new AutomationError(
          "Unsupported",
          "scrollWheel is not implemented by this platform's renderer"
        )
      }
      try {
        renderer.simulateScrollWheel(x, y, deltaX, deltaY, elementId)
      } catch (error) {
        // The native gate is per-platform (renderer.rs compiles simulate_*
        // only for macOS); surface that as Unsupported, not Protocol noise.
        if (process.platform === "darwin") throw error
        throw new AutomationError(
          "Unsupported",
          error instanceof Error ? error.message : String(error)
        )
      }
    },
    simulateKeystrokes(keystrokes) {
      flush()
      if (!renderer.simulateKeystrokes) throw noKeyboard()
      try {
        renderer.simulateKeystrokes(keystrokes)
      } catch (error) {
        throw keyboardFailure("keystrokes", error)
      }
    },
    nativeSimulateKeystrokes(elementId, keystrokes) {
      flush()
      if (!renderer.simulateKeystrokes) throw noKeyboard()
      try {
        // Focus before typing. Keystrokes are dispatched to whatever has
        // focus, and an element only exposes its text input handler while
        // focused, so without this the keys go nowhere. Mirrors the native
        // test harness in packages/solid/src/testing.ts.
        renderer.focusElement(elementId)
        renderer.simulateKeystrokes(keystrokes, elementId)
      } catch (error) {
        throw keyboardFailure("keystrokes", error)
      }
    },
    nativeSimulateKeyDown(elementId, keystroke, isHeld) {
      flush()
      if (!renderer.simulateKeyDown) throw noKeyboard()
      try {
        renderer.focusElement(elementId)
        renderer.simulateKeyDown(keystroke, isHeld, elementId)
      } catch (error) {
        throw keyboardFailure("keyDown", error)
      }
    },
    nativeSimulateKeyUp(elementId, keystroke) {
      flush()
      if (!renderer.simulateKeyUp) throw noKeyboard()
      try {
        renderer.focusElement(elementId)
        renderer.simulateKeyUp(keystroke, elementId)
      } catch (error) {
        throw keyboardFailure("keyUp", error)
      }
    },
    scrollTo: (id, x, y) => {
      flush()
      renderer.scrollTo(id, x, y)
    },
    getScrollOffset: (id) => {
      flush()
      const offset = renderer.getScrollOffset(id)
      return offset ? [offset[0], offset[1]] : null
    },
    getAllText: () => renderer.getAllText(),
    getPaintedText: () => {
      flush()
      return renderer.getPaintedText()
    },
    getSelectedText: () => {
      flush()
      return renderer.getSelectedText()
    },
    clearSelection: () => renderer.clearSelection(),
    captureScreenshot: (file) => {
      flush()
      return renderer.captureScreenshot(file)
    },
    // Draws every window, so the window trees `getTree` reads next are
    // current too.
    getAutomationTree: () => {
      flush()
      return renderer.getAutomationTree()
    },
    getWindowAutomationTrees: renderer.getWindowAutomationTrees
      ? () => renderer.getWindowAutomationTrees!()
      : undefined,
    getElementBounds: (id) => {
      flush()
      return renderer.getElementBounds(id)
    },
    clockPause: () => renderer.clockPause(),
    clockSet: (nowMs) => renderer.clockSet(nowMs),
    clockFastForward: (deltaMs) => renderer.clockFastForward(deltaMs),
    clockResume: () => renderer.clockResume(),
    focusElement: (id) => {
      flush()
      return renderer.focusElement(id)
    },
    blur: () => renderer.blur(),
  }
}

export async function connectTest(
  renderer: TestAutomationRenderer
): Promise<App> {
  const app = new App(new InProcessBackend(renderer))
  await app.call("initialize", {
    protocolVersion: PROTOCOL_VERSION,
    client: "@solo/solid/automation",
  })
  return app
}

export async function connectStdio(options: {
  write: (chunk: string) => void
  feed: (listener: (chunk: string) => void) => void
  close?: () => Promise<void>
}): Promise<App> {
  const app = new App(
    new SseBackend(options.write, options.feed, options.close)
  )
  await app.call("initialize", {
    protocolVersion: PROTOCOL_VERSION,
    client: "@solo/solid/automation",
  })
  return app
}

export async function launch(options: {
  command: string
  args?: string[]
  cwd?: string
  env?: NodeJS.ProcessEnv
}): Promise<App> {
  const child: ChildProcessWithoutNullStreams = spawn(
    options.command,
    options.args ?? [],
    {
      cwd: options.cwd,
      env: {
        ...process.env,
        ...options.env,
      },
      stdio: ["pipe", "pipe", "pipe"],
    }
  )
  const app = await connectStdio({
    write: (chunk) => {
      child.stdin.write(chunk)
    },
    feed: (listener) => {
      child.stdout.on("data", (buf: Buffer) => listener(buf.toString("utf8")))
    },
    close: async () => {
      child.kill()
    },
  })
  return app
}

export function handleAutomationRequest(
  raw: unknown,
  backend: AutomationBackend
): Promise<string> {
  const request = parseWireMessage(raw)
  if (!("method" in request)) {
    throw new AutomationError("Protocol", "Server expected a request")
  }
  return backend.call(request.method, request.params as never).then(
    (result) => encodeSse({ id: request.id, result } as AutomationResponse),
    (error) => {
      const failure =
        error instanceof AutomationError
          ? error
          : new AutomationError("Protocol", String(error))
      return encodeSse({
        id: request.id,
        error: {
          code: failure.code,
          message: failure.message,
          data: failure.data,
        },
      })
    }
  )
}

export function serveAutomationStdio(backend: AutomationBackend): void {
  const decoder = createSseDecoder((message) => {
    if (!("method" in message)) return
    void handleAutomationRequest(message, backend).then((reply) => {
      process.stdout.write(reply)
    })
  })
  process.stdin.setEncoding("utf8")
  process.stdin.on("data", (chunk: string) => {
    decoder.feed(chunk)
  })
}

export function isServerEvent(
  message: unknown
): message is AutomationServerEvent {
  return (
    typeof message === "object" &&
    message !== null &&
    "event" in message &&
    !("method" in message)
  )
}
