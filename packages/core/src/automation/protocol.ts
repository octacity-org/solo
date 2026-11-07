/// Typed Solo automation protocol and SSE codec.
///
/// Wire format is SSE `data:` lines so `eventsource-parser` can extract
/// messages from noisy stdout. JSON carries every field. Do not emit SSE
/// `id:` or `event:` fields; process logs that start with those prefixes
/// would otherwise leak into the next message.

import { createParser, type EventSourceMessage } from "eventsource-parser"
import { SchemaError, v, type Infer, type Schema } from "./schema.js"

export const PROTOCOL_VERSION = 1 as const

export const automationErrorCodes = [
  "Timeout",
  "NotFound",
  "Ambiguous",
  "Protocol",
  "Closed",
  "Unsupported",
  "Security",
  "Cancelled",
] as const

export type AutomationErrorCode = (typeof automationErrorCodes)[number]

export class AutomationError extends Error {
  readonly code: AutomationErrorCode
  readonly data?: unknown

  constructor(code: AutomationErrorCode, message: string, data?: unknown) {
    super(message)
    this.name = "AutomationError"
    this.code = code
    this.data = data
  }
}

const pointSchema = v.object({
  x: v.number(),
  y: v.number(),
  /** Window-local coordinates in the window of this element; default main. */
  elementId: v.number().optional(),
})

const buttonSchema = v.number().int().min(0).max(2).optional()

export const boundsSchema = v.object({
  x: v.number(),
  y: v.number(),
  width: v.number(),
  height: v.number(),
})

export type ElementBounds = Infer<typeof boundsSchema>

export const treeNodeSchema: Schema<TreeNode> = v.lazy(() =>
  v.object({
    id: v.number(),
    type: v.string(),
    text: v.string().optional(),
    testId: v.string().optional(),
    style: v.record(v.unknown()).optional(),
    events: v.array(v.string()).optional(),
    customProps: v.record(v.unknown()).optional(),
    bounds: boundsSchema.optional(),
    children: v.array(treeNodeSchema).optional(),
  })
)

export interface TreeNode {
  id: number
  type: string
  text?: string
  testId?: string
  style?: Record<string, unknown>
  events?: string[]
  customProps?: Record<string, unknown>
  bounds?: ElementBounds
  children?: TreeNode[]
}

const okSchema = v.object({ ok: v.literal(true) })

const capabilitiesSchema = v.array(
  v.enum(["input", "screenshot", "clock", "tree"])
)

/** Single source of truth for method names, params, and results. */
export const methods = {
  initialize: {
    params: v.object({
      protocolVersion: v.literal(PROTOCOL_VERSION),
      client: v.string(),
    }),
    result: v.object({
      protocolVersion: v.literal(PROTOCOL_VERSION),
      pid: v.number().int(),
      capabilities: capabilitiesSchema,
      window: v.object({
        width: v.number(),
        height: v.number(),
      }),
    }),
  },
  cancel: {
    params: v.object({ id: v.number().int() }),
    result: okSchema,
  },
  click: {
    params: pointSchema.extend({ button: buttonSchema }),
    result: okSchema,
  },
  mouseDown: {
    params: pointSchema.extend({ button: buttonSchema }),
    result: okSchema,
  },
  mouseUp: {
    params: pointSchema.extend({ button: buttonSchema }),
    result: okSchema,
  },
  mouseMove: {
    params: pointSchema.extend({ pressedButton: buttonSchema }),
    result: okSchema,
  },
  scrollWheel: {
    params: pointSchema.extend({
      deltaX: v.number(),
      deltaY: v.number(),
    }),
    result: okSchema,
  },
  keystrokes: {
    params: v.object({
      keys: v.string(),
      elementId: v.number().optional(),
    }),
    result: okSchema,
  },
  keyDown: {
    params: v.object({
      key: v.string(),
      isHeld: v.boolean().optional(),
      elementId: v.number().optional(),
    }),
    result: okSchema,
  },
  keyUp: {
    params: v.object({
      key: v.string(),
      elementId: v.number().optional(),
    }),
    result: okSchema,
  },
  focus: {
    params: v.object({ elementId: v.number() }),
    result: okSchema,
  },
  blur: {
    params: v.object({}),
    result: okSchema,
  },
  scrollTo: {
    params: v.object({
      elementId: v.number(),
      x: v.number(),
      y: v.number(),
    }),
    result: okSchema,
  },
  getScrollOffset: {
    params: v.object({ elementId: v.number() }),
    result: v.object({
      offset: v.tuple([v.number(), v.number()]).nullable(),
    }),
  },
  getTree: {
    params: v.object({}),
    result: v.object({
      tree: treeNodeSchema.nullable(),
      /** Windows opened with openWindow, in opening order. */
      windows: v.array(treeNodeSchema).optional(),
    }),
  },
  getPaintedText: {
    params: v.object({}),
    result: v.object({ text: v.array(v.string()) }),
  },
  getAllText: {
    params: v.object({}),
    result: v.object({ text: v.array(v.string()) }),
  },
  getBounds: {
    params: v.object({ elementId: v.number() }),
    result: v.object({ bounds: boundsSchema.nullable() }),
  },
  getSelectedText: {
    params: v.object({}),
    result: v.object({ text: v.string().nullable() }),
  },
  clearSelection: {
    params: v.object({}),
    result: okSchema,
  },
  screenshot: {
    params: v.object({ path: v.string() }),
    result: v.object({ path: v.string() }),
  },
  clockPause: {
    params: v.object({}),
    result: v.object({ nowMs: v.number() }),
  },
  clockSet: {
    params: v.object({ nowMs: v.number().nonnegative() }),
    result: v.object({ nowMs: v.number() }),
  },
  clockFastForward: {
    params: v.object({ deltaMs: v.number().nonnegative() }),
    result: v.object({ nowMs: v.number() }),
  },
  clockResume: {
    params: v.object({}),
    result: v.object({ nowMs: v.number() }),
  },
} as const

export type MethodName = keyof typeof methods
export type ParamsOf<M extends MethodName> = Infer<(typeof methods)[M]["params"]>
export type ResultOf<M extends MethodName> = Infer<(typeof methods)[M]["result"]>

export type AutomationRequest<M extends MethodName = MethodName> = {
  [K in M]: { id: number; method: K; params: ParamsOf<K> }
}[M]

export type AutomationSuccess<M extends MethodName = MethodName> = {
  [K in M]: { id: number; result: ResultOf<K> }
}[M]

export interface AutomationFailure {
  id: number
  error: {
    code: AutomationErrorCode
    message: string
    data?: unknown
  }
}

export type AutomationResponse<M extends MethodName = MethodName> =
  | AutomationSuccess<M>
  | AutomationFailure

export const serverEventNames = ["console", "frame", "closed"] as const
export type ServerEventName = (typeof serverEventNames)[number]

export type AutomationServerEvent =
  | { event: "console"; params: { text: string } }
  | { event: "frame"; params: { n: number; path?: string } }
  | { event: "closed"; params: { reason: string } }

export type WireMessage =
  | AutomationRequest
  | AutomationResponse
  | AutomationServerEvent

const methodNames = Object.keys(methods) as MethodName[]

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function parseMethodName(value: unknown): MethodName {
  if (typeof value !== "string" || !methodNames.includes(value as MethodName)) {
    throw new AutomationError(
      "Protocol",
      `Unknown automation method: ${String(value)}`
    )
  }
  return value as MethodName
}

export function parseRequest(value: unknown): AutomationRequest {
  if (!isObject(value)) {
    throw new AutomationError("Protocol", "Request must be an object")
  }
  if (typeof value.id !== "number" || !Number.isInteger(value.id)) {
    throw new AutomationError("Protocol", "Request id must be an integer")
  }
  const method = parseMethodName(value.method)
  let params: unknown
  try {
    params = methods[method].params.parse(value.params)
  } catch (error) {
    if (!(error instanceof SchemaError)) throw error
    throw new AutomationError("Protocol", `Invalid params for ${method}: ${error.message}`, { path: error.path })
  }
  return { id: value.id, method, params } as AutomationRequest
}

export function parseResponse(value: unknown): AutomationResponse {
  if (!isObject(value)) {
    throw new AutomationError("Protocol", "Response must be an object")
  }
  if (typeof value.id !== "number" || !Number.isInteger(value.id)) {
    throw new AutomationError("Protocol", "Response id must be an integer")
  }
  if ("error" in value) {
    if (!isObject(value.error) || typeof value.error.message !== "string") {
      throw new AutomationError("Protocol", "Invalid error payload")
    }
    const code = value.error.code
    if (typeof code !== "string" || !automationErrorCodes.includes(code as AutomationErrorCode)) {
      throw new AutomationError("Protocol", "Invalid error code")
    }
    return {
      id: value.id,
      error: {
        code: code as AutomationErrorCode,
        message: value.error.message,
        ...(value.error.data === undefined ? {} : { data: value.error.data }),
      },
    }
  }
  if (!("result" in value)) {
    throw new AutomationError("Protocol", "Response needs result or error")
  }
  return { id: value.id, result: value.result } as AutomationSuccess
}

export function parseServerEvent(value: unknown): AutomationServerEvent {
  if (!isObject(value) || typeof value.event !== "string") {
    throw new AutomationError("Protocol", "Event must have an event name")
  }
  if (value.event === "console") {
    const params = v.object({ text: v.string() }).parse(value.params)
    return { event: "console", params }
  }
  if (value.event === "frame") {
    const params = v
      .object({ n: v.number(), path: v.string().optional() })
      .parse(value.params)
    return { event: "frame", params }
  }
  if (value.event === "closed") {
    const params = v.object({ reason: v.string() }).parse(value.params)
    return { event: "closed", params }
  }
  throw new AutomationError(
    "Protocol",
    `Unknown server event: ${value.event}`
  )
}

export function parseWireMessage(value: unknown): WireMessage {
  if (!isObject(value)) {
    throw new AutomationError("Protocol", "Wire message must be an object")
  }
  if ("method" in value) return parseRequest(value)
  if ("event" in value) return parseServerEvent(value)
  if ("result" in value || "error" in value) return parseResponse(value)
  throw new AutomationError("Protocol", "Unrecognized wire message")
}

export function encodeSse(message: WireMessage): string {
  return `data: ${JSON.stringify(message)}\n\n`
}

export interface SseDecoder {
  feed(chunk: string): void
}

export function createSseDecoder(
  onMessage: (message: WireMessage) => void,
  onInvalid?: (raw: string, error: unknown) => void
): SseDecoder {
  const parser = createParser({
    onEvent(event: EventSourceMessage) {
      try {
        onMessage(parseWireMessage(JSON.parse(event.data)))
      } catch (error) {
        onInvalid?.(event.data, error)
      }
    },
  })
  return {
    feed(chunk: string): void {
      parser.feed(chunk)
    },
  }
}

export function decodeSseChunk(chunk: string): WireMessage[] {
  const messages: WireMessage[] = []
  const decoder = createSseDecoder((message) => {
    messages.push(message)
  })
  decoder.feed(chunk)
  return messages
}
