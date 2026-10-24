/// Shared test utilities for @solo/solid suites.
/// Ported from packages/react/src/__tests__/test-utils.ts.

import fs from "fs"
import path from "path"
import { fileURLToPath } from "url"
import { inflateSync } from "zlib"
import { expect } from "vitest"

export const isCI = !!process.env.CI

/** Where visual tests write their PNGs. Kept in the repo (gitignored) rather
 *  than /tmp so the output can actually be looked at after a run. */
export const SHOTS_DIR = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../../screenshots"
)

/** Compute byte-level similarity between two buffers (0..1).
 *  For PNGs from the same renderer, identical pixels → identical bytes. */
export function bufferSimilarity(a: Buffer, b: Buffer): number {
  const len = Math.max(a.length, b.length)
  if (len === 0) return 1
  let matching = 0
  for (let i = 0; i < len; i++) {
    if (a[i] === b[i]) matching++
  }
  return matching / len
}

/** Assert two screenshot PNGs exist, are non-empty, and are visually
 *  different (less than 99% byte similarity).
 *  Skipped on CI — Metal on macOS VMs doesn't repaint between captures. */
export function expectScreenshotsDiffer(beforePath: string, afterPath: string) {
  expect(fs.existsSync(beforePath)).toBe(true)
  expect(fs.existsSync(afterPath)).toBe(true)
  expect(fs.statSync(beforePath).size).toBeGreaterThan(0)

  if (isCI) return

  const before = fs.readFileSync(beforePath)
  const after = fs.readFileSync(afterPath)
  const similarity = bufferSimilarity(before, after)
  expect(similarity).toBeLessThan(0.99)
}

/** Decode an 8-bit, non-interlaced RGB/RGBA PNG (what the test renderer
 *  writes) so tests can assert on exact pixels. */
export function readPng(file: string) {
  const data = fs.readFileSync(file)
  let width = 0, height = 0, channels = 0
  const chunks: Buffer[] = []
  for (let pos = 8; pos < data.length;) {
    const length = data.readUInt32BE(pos)
    const type = data.toString("ascii", pos + 4, pos + 8)
    const body = data.subarray(pos + 8, pos + 8 + length)
    if (type === "IHDR") {
      width = body.readUInt32BE(0)
      height = body.readUInt32BE(4)
      channels = body[9] === 6 ? 4 : body[9] === 2 ? 3 : 0
      if (body[8] !== 8 || !channels || body[12] !== 0) throw new Error(`Unsupported PNG: ${file}`)
    } else if (type === "IDAT") chunks.push(body)
    pos += length + 12
  }
  const raw = inflateSync(Buffer.concat(chunks))
  const stride = width * channels
  const pixels = Buffer.alloc(height * stride)
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)]
    for (let x = 0; x < stride; x++) {
      const i = y * stride + x
      const a = x >= channels ? pixels[i - channels]! : 0
      const b = y > 0 ? pixels[i - stride]! : 0
      const c = x >= channels && y > 0 ? pixels[i - stride - channels]! : 0
      const p = a + b - c
      const paeth = Math.abs(p - a) <= Math.abs(p - b) && Math.abs(p - a) <= Math.abs(p - c) ? a
        : Math.abs(p - b) <= Math.abs(p - c) ? b : c
      const predictor = [0, a, b, (a + b) >> 1, paeth][filter!]!
      pixels[i] = (raw[y * (stride + 1) + 1 + x]! + predictor) & 0xff
    }
  }
  return {
    width,
    height,
    /** [r, g, b] at device pixel (x, y). */
    rgb(x: number, y: number): [number, number, number] {
      const i = Math.round(y) * stride + Math.round(x) * channels
      return [pixels[i]!, pixels[i + 1]!, pixels[i + 2]!]
    },
  }
}
