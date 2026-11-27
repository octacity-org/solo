/// MockNativeRenderer — in-memory NativeRenderer for framework tests on
/// platforms without TestSoloRenderer (the GPU-backed test renderer is
/// macOS-only). Records every mutation op so tests can assert exactly which
/// protocol messages a framework integration sends.

import type { NativeRenderer } from "./types.js"

export interface MockElement {
  id: number
  type: string
  style: Record<string, unknown>
  text: string | null
  events: Set<string>
  customProps: Record<string, unknown>
  children: number[]
  parentId: number | null
}

export class MockNativeRenderer implements NativeRenderer {
  /** Every mutation in order, as [op, ...args] with objects kept raw. */
  ops: [string, ...unknown[]][] = []

  elements = new Map<number, MockElement>()
  rootId: number | null = null

  private record(op: string, ...args: unknown[]): void {
    this.ops.push([op, ...args])
  }

  private el(id: number): MockElement {
    const found = this.elements.get(id)
    if (!found) throw new Error(`MockNativeRenderer: no element ${id}`)
    return found
  }

  /**
   * Mutation ops tolerate unknown ids exactly like Rust's RetainedTree,
   * which uses `if let Some(...)` everywhere — Solid's bookkeeping may emit
   * redundant ops for elements an earlier op in the same batch destroyed.
   */
  private elOrUndefined(id: number): MockElement | undefined {
    return this.elements.get(id)
  }

  createElement(id: number, elementType: string): void {
    this.record("createElement", id, elementType)
    this.elements.set(id, {
      id,
      type: elementType,
      style: {},
      text: null,
      events: new Set(),
      customProps: {},
      children: [],
      parentId: null,
    })
  }

  destroyElement(id: number): Array<number> {
    const destroyed: number[] = []
    const walk = (current: number): void => {
      const element = this.elOrUndefined(current)
      if (!element) return
      for (const child of [...element.children]) walk(child)
      this.elements.delete(current)
      if (element.parentId != null) {
        const parent = this.elOrUndefined(element.parentId)
        if (parent) parent.children = parent.children.filter((c) => c !== current)
      }
      element.parentId = null
      destroyed.push(current)
    }
    this.record("destroyElement", id)
    walk(id)
    return destroyed
  }

  appendChild(parentId: number, childId: number): void {
    this.record("appendChild", parentId, childId)
    const parent = this.elOrUndefined(parentId)
    const child = this.elOrUndefined(childId)
    if (!parent || !child) return
    if (child.parentId != null) {
      const oldParent = this.elOrUndefined(child.parentId)
      if (oldParent) oldParent.children = oldParent.children.filter((c) => c !== childId)
    }
    child.parentId = parentId
    parent.children.push(childId)
  }

  removeChild(parentId: number, childId: number): void {
    this.record("removeChild", parentId, childId)
    // Lenient like RetainedTree::remove_child — either end may already be gone.
    const parent = this.elOrUndefined(parentId)
    if (parent) parent.children = parent.children.filter((c) => c !== childId)
    const child = this.elOrUndefined(childId)
    if (child) child.parentId = null
  }

  insertBefore(parentId: number, childId: number, beforeId: number): void {
    this.record("insertBefore", parentId, childId, beforeId)
    const parent = this.elOrUndefined(parentId)
    const child = this.elOrUndefined(childId)
    if (!parent || !child) return
    if (child.parentId != null) {
      const oldParent = this.elOrUndefined(child.parentId)
      if (oldParent) oldParent.children = oldParent.children.filter((c) => c !== childId)
    }
    child.parentId = parentId
    const index = parent.children.indexOf(beforeId)
    if (index === -1) {
      parent.children.push(childId)
    } else {
      parent.children.splice(index, 0, childId)
    }
  }

  setStyle(id: number, styleJson: string | object): void {
    this.record("setStyle", id, styleJson)
    const element = this.elOrUndefined(id)
    if (!element) return
    const style = typeof styleJson === "string" ? JSON.parse(styleJson) : styleJson
    Object.assign(element.style, style)
  }

  setText(id: number, content: string): void {
    this.record("setText", id, content)
    const element = this.elOrUndefined(id)
    if (element) element.text = content
  }

  setEventListener(id: number, eventType: string, hasHandler: boolean): void {
    this.record("setEventListener", id, eventType, hasHandler)
    const element = this.elOrUndefined(id)
    if (!element) return
    if (hasHandler) {
      element.events.add(eventType)
    } else {
      element.events.delete(eventType)
    }
  }

  setRoot(id: number): void {
    this.record("setRoot", id)
    this.rootId = id
  }

  setCustomProp(id: number, key: string, valueJson: string | object | number | boolean | null): void {
    this.record("setCustomProp", id, key, valueJson)
    const element = this.elOrUndefined(id)
    if (!element) return
    const value =
      typeof valueJson === "string" ? maybeParse(valueJson) : (valueJson ?? null)
    if (value == null) {
      delete element.customProps[key]
    } else {
      element.customProps[key] = value
    }
  }

  commitMutations(): void {
    this.record("commitMutations")
  }

  /** Answers `callElement`; the default returns null. */
  elementCallHandler: (id: number, method: string, args: unknown) => unknown = () => null

  callElement(elementId: number, method: string, argsJson: string): string {
    this.record("callElement", elementId, method, argsJson)
    return JSON.stringify(this.elementCallHandler(elementId, method, JSON.parse(argsJson)) ?? null)
  }

  applyBatch(json: string): Array<number> {
    const ops = JSON.parse(json) as [string, ...unknown[]][]
    const destroyed: number[] = []
    for (const op of ops) {
      switch (op[0]) {
        case "createElement":
          this.createElement(op[1] as number, op[2] as string)
          break
        case "destroyElement":
          destroyed.push(...this.destroyElement(op[1] as number))
          break
        case "appendChild":
          this.appendChild(op[1] as number, op[2] as number)
          break
        case "removeChild":
          this.removeChild(op[1] as number, op[2] as number)
          break
        case "insertBefore":
          this.insertBefore(op[1] as number, op[2] as number, op[3] as number)
          break
        case "setStyle":
          this.setStyle(op[1] as number, op[2] as object)
          break
        case "setText":
          this.setText(op[1] as number, op[2] as string)
          break
        case "setEventListener":
          this.setEventListener(op[1] as number, op[2] as string, op[3] as boolean)
          break
        case "setRoot":
          this.setRoot(op[1] as number)
          break
        case "setCustomProp":
          this.setCustomProp(op[1] as number, op[2] as string, op[3] as never)
          break
        case "setCustomPropValue":
          // Raw value form — no double parsing.
          this.record("setCustomProp", op[1], op[2], op[3])
          {
            const element = this.el(op[1] as number)
            if (op[3] == null) delete element.customProps[op[2] as string]
            else element.customProps[op[2] as string] = op[3]
          }
          break
        default:
          throw new Error(`MockNativeRenderer.applyBatch: unknown op ${op[0]}`)
      }
    }
    return destroyed
  }

  // ── Inspection helpers ─────────────────────────────────────────

  getRoot(): MockElement | undefined {
    return this.rootId != null ? this.elements.get(this.rootId) : undefined
  }

  getElement(id: number): MockElement | undefined {
    return this.elements.get(id)
  }

  findByType(type: string): MockElement[] {
    return [...this.elements.values()].filter((e) => e.type === type)
  }

  getAllText(): string[] {
    return [...this.elements.values()]
      .filter((e) => e.type === "text" && e.text)
      .map((e) => e.text as string)
  }

  /** Ops recorded since the last call — e.g. to assert one setText only. */
  drainOps(): [string, ...unknown[]][] {
    const drained = this.ops
    this.ops = []
    return drained
  }
}

function maybeParse(value: string): unknown {
  try {
    return JSON.parse(value)
  } catch {
    return value
  }
}
