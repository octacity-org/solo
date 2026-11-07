import { describe, expect, expectTypeOf, it } from "vitest"
import { SchemaError, v, type Infer, type Schema } from "../automation/schema.js"

function failure(run: () => unknown): SchemaError {
  try {
    run()
  } catch (error) {
    if (error instanceof SchemaError) return error
    throw error
  }
  throw new Error("expected a SchemaError")
}

describe("automation schema", () => {
  it("checks primitives and literals", () => {
    expect(v.number().parse(1.5)).toBe(1.5)
    expect(v.string().parse("a")).toBe("a")
    expect(v.boolean().parse(false)).toBe(false)
    expect(v.unknown().parse(Symbol.for("x"))).toBe(Symbol.for("x"))
    expect(v.literal(1).parse(1)).toBe(1)
    expect(v.enum(["a", "b"]).parse("b")).toBe("b")
    expect(failure(() => v.number().parse("1")).message).toBe("expected a number, got string")
    expect(() => v.number().parse(Number.NaN)).toThrow(SchemaError)
    expect(() => v.number().parse(Number.POSITIVE_INFINITY)).toThrow(SchemaError)
    expect(() => v.string().parse(1)).toThrow(SchemaError)
    expect(() => v.boolean().parse("true")).toThrow(SchemaError)
    expect(() => v.literal(1).parse(2)).toThrow(SchemaError)
    expect(() => v.enum(["a", "b"]).parse("c")).toThrow(SchemaError)
  })

  it("applies number rules in order", () => {
    const button = v.number().int().min(0).max(2)
    expect(button.parse(2)).toBe(2)
    expect(failure(() => button.parse(1.5)).message).toBe("expected an integer, got 1.5")
    expect(failure(() => button.parse(-1)).message).toBe("expected at least 0, got -1")
    expect(failure(() => button.parse(3)).message).toBe("expected at most 2, got 3")
    expect(() => v.number().nonnegative().parse(-0.1)).toThrow(SchemaError)
  })

  it("parses objects, stripping unknown keys and omitting absent optional ones", () => {
    const point = v.object({ x: v.number(), label: v.string().optional() })
    expect(point.parse({ x: 1, extra: true })).toEqual({ x: 1 })
    expect("label" in point.parse({ x: 1 })).toBe(false)
    expect(point.parse({ x: 1, label: "a" })).toEqual({ x: 1, label: "a" })
    expect(failure(() => point.parse({ x: "1" })).message).toBe("x: expected a number, got string")
    expect(failure(() => point.parse(null)).message).toBe("expected an object, got null")
    expect(() => point.parse([])).toThrow(SchemaError)
  })

  it("extends objects, letting new fields replace old ones", () => {
    const base = v.object({ x: v.number(), y: v.number() })
    const extended = base.extend({ y: v.string(), z: v.boolean().optional() })
    expect(extended.parse({ x: 1, y: "a" })).toEqual({ x: 1, y: "a" })
    expect(() => extended.parse({ x: 1, y: 2 })).toThrow(SchemaError)
    expectTypeOf<Infer<typeof extended>>().toEqualTypeOf<{ x: number; y: string; z?: boolean | undefined }>()
  })

  it("handles nullable, arrays, tuples and records with paths", () => {
    expect(v.string().nullable().parse(null)).toBeNull()
    expect(v.array(v.number()).parse([1, 2])).toEqual([1, 2])
    expect(failure(() => v.object({ list: v.array(v.number()) }).parse({ list: [1, "2"] })).message)
      .toBe("list[1]: expected a number, got string")
    expect(v.tuple([v.number(), v.number()]).parse([1, 2])).toEqual([1, 2])
    expect(failure(() => v.tuple([v.number(), v.number()]).parse([1])).message)
      .toBe("expected an array of length 2, got 1")
    expect(v.record(v.unknown()).parse({ a: 1 })).toEqual({ a: 1 })
    expect(failure(() => v.record(v.number()).parse({ a: "x" })).message).toBe("a: expected a number, got string")
    expect(failure(() => v.object({ a: v.number().optional() }).parse({ a: null })).path).toEqual(["a"])
  })

  it("supports recursive schemas through lazy", () => {
    interface Node { id: number; children?: Node[] }
    const node: Schema<Node> = v.lazy(() => v.object({ id: v.number(), children: v.array(node).optional() }))
    expect(node.parse({ id: 1, children: [{ id: 2 }] })).toEqual({ id: 1, children: [{ id: 2 }] })
    expect(failure(() => node.parse({ id: 1, children: [{ id: "2" }] })).message)
      .toBe("children[0].id: expected a number, got string")
  })

  it("infers the TypeScript type of each schema", () => {
    const schema = v.object({
      n: v.number(),
      s: v.string().nullable(),
      e: v.enum(["a", "b"]),
      l: v.literal(1),
      t: v.tuple([v.number(), v.string()]),
      o: v.array(v.boolean()).optional(),
    })
    expectTypeOf<Infer<typeof schema>>().toEqualTypeOf<{
      n: number
      s: string | null
      e: "a" | "b"
      l: 1
      t: [number, string]
      o?: boolean[] | undefined
    }>()
  })
})
