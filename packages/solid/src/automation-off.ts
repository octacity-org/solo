/// Stand-in for automation-host.ts in builds without automation; see
/// `automation` in vite.ts. Nothing here touches stdin or stdout.

import type { SoloRenderer } from "@solo/native"

export function stdinIsTTY(): boolean {
  return false
}

export function markStdinAsTTY(): void {}

export function automateWhenPiped(_renderer: SoloRenderer): void {}
