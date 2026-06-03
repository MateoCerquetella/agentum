#!/usr/bin/env node
// Guard: every command literal the typed Tauri client (src/tauri/*) calls MUST
// have a registered handler in the Rust shell (crates/agentum-desktop/src/lib.rs).
// Catches command-name drift at build time instead of at runtime.
//
// KNOWN_MISSING is the explicit, shrinking list of client commands that have no
// Rust handler yet (tracked in the P4 work list). The guard FAILS if a command
// outside this list is missing, and also FAILS if a KNOWN_MISSING entry has since
// been implemented (so the list can't rot). Run: node scripts/verify-tauri-commands.mjs
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const tauriDir = join(here, '..', 'src', 'tauri')
const libRs = join(here, '..', '..', 'src', 'lib.rs')

// Commands the client calls but Rust does not yet implement (P4 work list).
// Empty: every client command now resolves to a registered Rust handler.
const KNOWN_MISSING = new Set([])

const clientCommands = new Set()
for (const f of readdirSync(tauriDir)) {
  if (!f.endsWith('.ts')) continue
  const src = readFileSync(join(tauriDir, f), 'utf8')
  for (const m of src.matchAll(/call\('([a-z0-9_]+)'/g)) clientCommands.add(m[1])
}

// Extract the generate_handler![ ... ] command list.
const lib = readFileSync(libRs, 'utf8')
const block = lib.slice(lib.indexOf('generate_handler!['))
const handlers = new Set()
for (const m of block.matchAll(/[a-z0-9_]+::([a-z0-9_]+)\s*,/g)) handlers.add(m[1])

const missing = [...clientCommands].filter((c) => !handlers.has(c)).sort()
const unexpectedMissing = missing.filter((c) => !KNOWN_MISSING.has(c))
const staleKnown = [...KNOWN_MISSING].filter((c) => handlers.has(c)).sort()

let ok = true
if (unexpectedMissing.length) {
  ok = false
  console.error(`\n✗ ${unexpectedMissing.length} client command(s) have NO Rust handler (and are not in KNOWN_MISSING):`)
  for (const c of unexpectedMissing) console.error(`    ${c}`)
}
if (staleKnown.length) {
  ok = false
  console.error(`\n✗ ${staleKnown.length} KNOWN_MISSING command(s) are now implemented — remove them from the list:`)
  for (const c of staleKnown) console.error(`    ${c}`)
}
if (ok) {
  console.log(`✓ tauri client: ${clientCommands.size} commands, all resolve to a Rust handler (or KNOWN_MISSING).`)
  console.log(`  (${KNOWN_MISSING.size} still pending P4 implementation.)`)
}
process.exit(ok ? 0 : 1)
