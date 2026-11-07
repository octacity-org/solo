/// Minimal runtime validation for the automation protocol.
///
/// The protocol needs a dozen primitives. A general library costs far more
/// than that in a binary built with `bun build --compile --aot`, which turns
/// all of its JavaScript into machine code (zod was about 7 MB), and zod's
/// validators are generated with `new Function`, which such a binary may not
/// contain. Nothing here generates code at run time.

type Path = (string | number)[]

/** A value failed validation; `path` locates it inside the parsed value. */
export class SchemaError extends Error {
  readonly path: Path

  constructor(path: Path, message: string) {
    super(path.length === 0 ? message : `${formatPath(path)}: ${message}`)
    this.name = "SchemaError"
    this.path = path
  }
}

function formatPath(path: Path): string {
  return path
    .map((part, index) => (typeof part === "number" ? `[${part}]` : index === 0 ? part : `.${part}`))
    .join("")
}

function describe(value: unknown): string {
  if (value === null) return "null"
  if (Array.isArray(value)) return "array"
  if (typeof value === "number") return String(value)
  return typeof value
}

export interface Schema<T> {
  /** Return `value` as `T` (objects without unknown keys) or throw SchemaError. */
  parse(value: unknown): T
  /** Parse `value` found at `path`; used by containers to report locations. */
  check(value: unknown, path: Path): T
  /** Also accept `undefined`; an object field becomes optional. */
  optional(): OptionalSchema<T>
  /** Also accept `null`. */
  nullable(): Schema<T | null>
}

export interface OptionalSchema<T> extends Schema<T | undefined> {
  readonly optionalField: true
}

export type Infer<S> = S extends Schema<infer T> ? T : never

function schema<T>(check: (value: unknown, path: Path) => T): Schema<T> {
  return {
    check,
    parse: (value) => check(value, []),
    optional: () => ({
      ...schema<T | undefined>((value, path) => (value === undefined ? undefined : check(value, path))),
      optionalField: true,
    }),
    nullable: () => schema<T | null>((value, path) => (value === null ? null : check(value, path))),
  }
}

function typed<T>(name: string, test: (value: unknown) => boolean): () => Schema<T> {
  return () =>
    schema<T>((value, path) => {
      if (!test(value)) throw new SchemaError(path, `expected ${name}, got ${describe(value)}`)
      return value as T
    })
}

export interface NumberSchema extends Schema<number> {
  int(): NumberSchema
  min(minimum: number): NumberSchema
  max(maximum: number): NumberSchema
  nonnegative(): NumberSchema
}

type NumberRule = (value: number) => string | undefined

function numberWith(rules: NumberRule[]): NumberSchema {
  const base = schema<number>((value, path) => {
    if (typeof value !== "number" || !Number.isFinite(value)) {
      throw new SchemaError(path, `expected a number, got ${describe(value)}`)
    }
    for (const rule of rules) {
      const problem = rule(value)
      if (problem) throw new SchemaError(path, problem)
    }
    return value
  })
  const add = (rule: NumberRule): NumberSchema => numberWith([...rules, rule])
  return {
    ...base,
    int: () => add((n) => (Number.isInteger(n) ? undefined : `expected an integer, got ${n}`)),
    min: (minimum) => add((n) => (n >= minimum ? undefined : `expected at least ${minimum}, got ${n}`)),
    max: (maximum) => add((n) => (n <= maximum ? undefined : `expected at most ${maximum}, got ${n}`)),
    nonnegative: () => add((n) => (n >= 0 ? undefined : `expected a nonnegative number, got ${n}`)),
  }
}

type Shape = Record<string, Schema<unknown>>

type OptionalKeys<S extends Shape> = {
  [K in keyof S]: S[K] extends OptionalSchema<unknown> ? K : never
}[keyof S]

type Simplify<T> = { [K in keyof T]: T[K] } & {}

export type ObjectOf<S extends Shape> = Simplify<
  { [K in Exclude<keyof S, OptionalKeys<S>>]: Infer<S[K]> } & { [K in OptionalKeys<S>]?: Infer<S[K]> }
>

export interface ObjectSchema<S extends Shape> extends Schema<ObjectOf<S>> {
  readonly shape: S
  /** A new object schema with `extra`'s fields added, replacing same-named ones. */
  extend<E extends Shape>(extra: E): ObjectSchema<Simplify<Omit<S, keyof E> & E>>
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

function object<S extends Shape>(shape: S): ObjectSchema<S> {
  const keys = Object.keys(shape)
  const base = schema<ObjectOf<S>>((value, path) => {
    if (!isPlainObject(value)) throw new SchemaError(path, `expected an object, got ${describe(value)}`)
    const output: Record<string, unknown> = {}
    for (const key of keys) {
      const parsed = shape[key].check(value[key], [...path, key])
      if (parsed !== undefined || key in value) output[key] = parsed
    }
    return output as ObjectOf<S>
  })
  return {
    ...base,
    shape,
    extend: <E extends Shape>(extra: E) => object({ ...shape, ...extra } as Simplify<Omit<S, keyof E> & E>),
  }
}

function array<T>(item: Schema<T>): Schema<T[]> {
  return schema((value, path) => {
    if (!Array.isArray(value)) throw new SchemaError(path, `expected an array, got ${describe(value)}`)
    return value.map((element, index) => item.check(element, [...path, index]))
  })
}

function tuple<T extends Schema<unknown>[]>(items: [...T]): Schema<{ [K in keyof T]: Infer<T[K]> }> {
  return schema((value, path) => {
    if (!Array.isArray(value)) throw new SchemaError(path, `expected an array, got ${describe(value)}`)
    if (value.length !== items.length) {
      throw new SchemaError(path, `expected an array of length ${items.length}, got ${value.length}`)
    }
    return items.map((item, index) => item.check(value[index], [...path, index])) as { [K in keyof T]: Infer<T[K]> }
  })
}

/** An object with string keys and values of one schema. */
function record<T>(item: Schema<T>): Schema<Record<string, T>> {
  return schema((value, path) => {
    if (!isPlainObject(value)) throw new SchemaError(path, `expected an object, got ${describe(value)}`)
    const output: Record<string, T> = {}
    for (const key of Object.keys(value)) output[key] = item.check(value[key], [...path, key])
    return output
  })
}

function literal<const T extends string | number | boolean>(expected: T): Schema<T> {
  return schema((value, path) => {
    if (value !== expected) throw new SchemaError(path, `expected ${JSON.stringify(expected)}, got ${describe(value)}`)
    return expected
  })
}

function oneOf<const T extends readonly string[]>(values: T): Schema<T[number]> {
  return schema((value, path) => {
    if (typeof value !== "string" || !values.includes(value)) {
      throw new SchemaError(path, `expected one of ${values.join(", ")}, got ${describe(value)}`)
    }
    return value
  })
}

/** Defer building a schema, for recursive types. Annotate the result's type. */
function lazy<T>(get: () => Schema<T>): Schema<T> {
  return schema((value, path) => get().check(value, path))
}

export const v = {
  number: () => numberWith([]),
  string: typed<string>("a string", (value) => typeof value === "string"),
  boolean: typed<boolean>("a boolean", (value) => typeof value === "boolean"),
  unknown: () => schema<unknown>((value) => value),
  literal,
  enum: oneOf,
  object,
  array,
  tuple,
  record,
  lazy,
}
