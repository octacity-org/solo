import type { NativeRenderer } from "@solo/core"

/**
 * `renderer[method]` bound to the renderer. Throws "This renderer does not
 * <ability>" when the renderer lacks the method.
 */
export function requireCapability<K extends keyof NativeRenderer>(
  renderer: NativeRenderer,
  method: K,
  ability: string
): NonNullable<NativeRenderer[K]> {
  const fn = renderer[method]
  if (typeof fn !== "function") throw new Error(`This renderer does not ${ability}`)
  return fn.bind(renderer) as NonNullable<NativeRenderer[K]>
}
