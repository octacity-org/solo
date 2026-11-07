/// Batching renderer: buffers individual mutation calls and sends them to
/// native in one applyBatch() FFI call per commit.
///
/// Queue raw objects for setStyle / setCustomProp. Do not JSON.stringify them
/// first. The outer applyBatch stringify would escape that string again, and
/// Rust would parse twice. A 10k-row mount spent 626ms in applyBatch that way.
///
/// Mutation calls are captured as ["methodName", ...args] in a queue, and
/// commitMutations() sends the whole queue as applyBatch(json). Every other
/// method passes straight through to the inner renderer.
///
/// Adding a new mutation method to NativeRenderer requires adding it to
/// BATCHED_METHODS below — nothing else.
///
/// ## Batch timing
///
/// The batch boundary is the framework's commit phase:
///
///   state change → framework render → mutation callbacks → commit end
///                                ↓ each callback queues ops  ↓ flushes queue
///                                queue.push([name, ...args]) applyBatch(json)
///
/// Multiple updates batched by the framework into one render = one batch.
/// Multiple separate commits in the same event loop tick = multiple batches.

import type { NativeRenderer } from "./types.js"
import { unregisterEventHandlers } from "./event-registry.js"

type MutationTuple = (number | string | boolean | object | null)[]

/// Methods that are queued instead of called immediately. Any method NOT in
/// this list passes through to the inner renderer, so getters, queries and
/// future non-mutation methods never enqueue garbage ops.
const BATCHED_METHODS = [
  "createElement",
  "destroyElement",
  "appendChild",
  "removeChild",
  "insertBefore",
  "setStyle",
  "setText",
  "setEventListener",
  "setRoot",
  "setCustomProp",
] as const

export interface BatchingHooks {
  /** A mutation was queued. The host arms its commit here. */
  onQueue?(): void
  /** destroyElement was called for `id`, before its op is queued. */
  onDestroy?(id: number): void
}

/**
 * Wrap a NativeRenderer with batching.
 *
 * Mutations are queued and flushed in one applyBatch() per commitMutations().
 * setCustomProp is queued as setCustomPropValue so raw strings stay strings.
 * destroyElement returns [] at once; the destroyed IDs come from applyBatch,
 * and their event handlers are unregistered after the batch.
 */
export function wrapWithBatching(inner: NativeRenderer, hooks: BatchingHooks = {}): NativeRenderer {
  let queue: MutationTuple[] = []
  const { onQueue, onDestroy } = hooks

  // Built once, so a queued op allocates only its tuple.
  const methods = new Map<string | symbol, unknown>()
  for (const name of BATCHED_METHODS) {
    const op = name === "setCustomProp" ? "setCustomPropValue" : name
    methods.set(name, (...args: MutationTuple) => {
      if (name === "destroyElement") onDestroy?.(args[0] as number)
      queue.push([op, ...args])
      onQueue?.()
      if (name === "destroyElement") return []
    })
  }
  methods.set("commitMutations", () => {
    if (queue.length === 0) {
      inner.commitMutations()
      return
    }
    // applyBatch may throw on malformed ops. The queue is kept on failure so
    // JS and Rust state don't desync.
    const destroyedIds = inner.applyBatch(JSON.stringify(queue))
    for (const id of destroyedIds) unregisterEventHandlers(id)
    queue = []
  })

  return new Proxy(inner, {
    get(target, prop) {
      const method = methods.get(prop)
      if (method !== undefined) return method
      // Native methods need the real instance as `this`.
      const value = Reflect.get(target, prop)
      return typeof value === "function" ? value.bind(target) : value
    },
  })
}
