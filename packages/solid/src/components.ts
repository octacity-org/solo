/// Semantic primitives for Solo apps.
///
/// Thin wrappers over existing native intrinsic elements — no new Rust.
/// Written against the runtime ops directly (rather than JSX) so the package
/// builds with plain tsc; compiled user JSX lands on the same ops anyway.

import type { EventPayload } from "@solo/native"
import type { StyleDesc } from "@solo/core"
import { createMemo, createSignal, For, merge, untrack } from "solid-js"
import type { Accessor } from "solid-js"
import { createComponent, universal } from "./runtime.js"
import type { SoloSolidNode } from "./runtime.js"

export interface ViewProps {
  style?: StyleDesc
  children?: unknown
  onClick?: (event: EventPayload) => void
  onMouseDown?: (event: EventPayload) => void
  onMouseUp?: (event: EventPayload) => void
  onMouseEnter?: (event: EventPayload) => void
  onMouseLeave?: (event: EventPayload) => void
  /** Stable locator id for automation and tests. */
  testId?: string
}

/** A flex container. Maps to the native "div" element. */
export function View(props: ViewProps): SoloSolidNode {
  const el = universal.createElement("div")
  universal.spread(el, props as Record<string, unknown>)
  return el
}

export interface TextProps {
  style?: StyleDesc
  children?: unknown
  /** Stable locator id for automation and tests. */
  testId?: string
}

/** Styled text content. Maps to the native "text" element. */
export function Text(props: TextProps): SoloSolidNode {
  const el = universal.createElement("text")
  // spread handles style, events, custom props AND the (reactive) children.
  universal.spread(el, props as Record<string, unknown>)
  return el
}

const BUTTON_BASE_STYLE: StyleDesc = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  paddingLeft: 20,
  paddingRight: 20,
  paddingTop: 10,
  paddingBottom: 10,
  backgroundColor: "#a6e3a1",
  borderRadius: 8,
  cursor: "pointer",
}

export interface ButtonProps extends Omit<ViewProps, "style"> {
  style?: StyleDesc
  /** Opt into native Tab focus/Space/Enter activation, e.g. tabIndex={0}. */
  tabIndex?: number
}

/** A clickable button. A native "div" with sensible default styles. */
export function Button(props: ButtonProps): SoloSolidNode {
  const el = universal.createElement("div")
  const merged = merge(props, {
    get style(): StyleDesc {
      return { ...BUTTON_BASE_STYLE, ...props.style }
    },
  })
  universal.spread(el, merged as Record<string, unknown>)
  return el
}

export interface VirtualListProps<T> {
  items: readonly T[]
  /** Builds one row. Runs again only when the row's item changes. */
  renderRow: (item: T, index: Accessor<number>) => unknown
  /** Row identity across `items` updates. Defaults to the item itself. */
  key?: (item: T, index: number) => unknown
  /** A fixed height for every row; it also serves as the estimate. */
  itemHeight?: number
  /** Height assumed for rows not yet measured, and for placeholders. Default 32. */
  estimatedItemHeight?: number
  /** Extra pixels mounted above and below the viewport. Default 512. */
  overdraw?: number
  alignment?: "top" | "bottom"
  /** Keep the newest row in view as items are appended. */
  followTail?: boolean
  style?: StyleDesc
  /** Stable locator id for automation and tests. */
  testId?: string
}

/** Pixels of rows mounted before the list reports its first range. */
const INITIAL_EXTENT = 1024

/**
 * A virtual list over data. Only the rows in view, plus `overdraw`, exist as
 * Solid nodes and native elements: the native list reports the item range it
 * needs after each layout, and paints a placeholder at the estimated height
 * until a row is mounted.
 */
export function VirtualList<T>(props: VirtualListProps<T>): SoloSolidNode {
  const [range, setRange] = createSignal<{ start: number; end: number; focused?: number }>()
  const estimate = (): number => props.itemHeight ?? props.estimatedItemHeight ?? 32
  const mounted = createMemo(() => {
    const count = props.items.length
    let current = range()
    if (!current) {
      const extent = Math.min(count, Math.ceil(INITIAL_EXTENT / estimate()))
      current = props.alignment === "bottom"
        ? { start: count - extent, end: count }
        : { start: 0, end: extent }
    }
    const end = Math.min(current.end, count)
    const indices: number[] = []
    for (let index = current.start; index < end; index++) indices.push(index)
    // The focused row stays mounted out of range, or it would lose focus.
    const focused = current.focused
    if (focused !== undefined && focused < count && (focused < current.start || focused >= end)) {
      indices.push(focused)
    }
    return indices
  })
  const key = (index: number): unknown =>
    props.key ? props.key(props.items[index] as T, index) : props.items[index]

  const row = (index: Accessor<number>): SoloSolidNode => {
    const el = universal.createElement("div")
    const item = createMemo(() => props.items[index()] as T)
    universal.spread(el, {
      get itemIndex() {
        return index()
      },
      get style(): StyleDesc | undefined {
        return props.itemHeight === undefined ? undefined : { height: props.itemHeight }
      },
      children: () => {
        const value = item()
        return untrack(() => props.renderRow(value, index))
      },
    })
    return el
  }

  const el = universal.createElement("virtual-list")
  universal.spread(el, {
    get itemCount() {
      return props.items.length
    },
    get estimatedItemHeight() {
      return estimate()
    },
    get overdraw() {
      return props.overdraw
    },
    get alignment() {
      return props.alignment
    },
    get followTail() {
      return props.followTail
    },
    get style() {
      return props.style
    },
    get testId() {
      return props.testId
    },
    onRangeChange: (event: EventPayload) =>
      setRange({
        start: event.rangeStart ?? 0,
        end: event.rangeEnd ?? 0,
        focused: event.focusedIndex ?? undefined,
      }),
    children: createComponent(For, {
      get each() {
        return mounted()
      },
      keyed: key,
      children: row,
    }),
  })
  return el
}
