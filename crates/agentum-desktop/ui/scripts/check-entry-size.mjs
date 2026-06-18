#!/usr/bin/env node
// Regression guard for the eager startup chunk.
//
// The desktop app's launch speed on weak CPUs is dominated by parsing the
// eager entry chunk before first paint. #21 cut it from 3.44 MB to ~1.96 MB by
// deferring react-markdown and xterm. This script fails CI (and `bun run
// build:check`) if the entry grows back past a budget, so heavy eager imports
// can't silently creep in again.
//
// It identifies the entry as the largest `assets/index-*.js` (Vite names the
// app entry `index-<hash>.js`; lazy chunks are smaller). Run it after a build.

import { readdirSync, statSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'

// Budget for the eager entry chunk, in bytes. Current entry is ~2.05 MB; the
// headroom catches a real regression (a heavy lib re-entering the graph)
// without flagging normal feature growth. Lower it as the entry shrinks.
const BUDGET_BYTES = 2_300_000

const here = dirname(fileURLToPath(import.meta.url))
const assetsDir = join(here, '..', 'dist', 'assets')

let entries
try {
  entries = readdirSync(assetsDir)
} catch {
  console.error(`[check-entry-size] No build output at ${assetsDir}. Run the build first.`)
  process.exit(1)
}

const indexChunks = entries
  .filter((f) => /^index-.*\.js$/.test(f))
  .map((f) => ({ f, size: statSync(join(assetsDir, f)).size }))
  .sort((a, b) => b.size - a.size)

if (indexChunks.length === 0) {
  console.error('[check-entry-size] No index-*.js entry chunk found in dist/assets.')
  process.exit(1)
}

const entry = indexChunks[0]
const mb = (n) => (n / 1_000_000).toFixed(2)

if (entry.size > BUDGET_BYTES) {
  console.error(
    `[check-entry-size] FAIL: entry chunk ${entry.f} is ${mb(entry.size)} MB ` +
      `(> budget ${mb(BUDGET_BYTES)} MB).\n` +
      `A heavy dependency likely re-entered the eager startup graph. Defer it ` +
      `behind a lazy() boundary (see #21), or raise BUDGET_BYTES intentionally.`
  )
  process.exit(1)
}

console.log(
  `[check-entry-size] OK: entry chunk ${entry.f} is ${mb(entry.size)} MB ` +
    `(budget ${mb(BUDGET_BYTES)} MB).`
)
