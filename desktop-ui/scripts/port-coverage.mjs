// Computes Tauri command coverage for the Electron->Tauri port.
// Mirrors electron-bridge.ts segmentToSnakeCase so the derived command
// names match exactly what the renderer will invoke() at runtime.
import { execSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'

const ROOT = new URL('..', import.meta.url).pathname

function segmentToSnakeCase(segment) {
  return segment
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/[:.-]/g, '_')
    .toLowerCase()
}
const pathToCommand = (ns, method) =>
  [ns, method].map(segmentToSnakeCase).join('_')

// 1. Registered Rust commands from lib.rs invoke_handler.
const libRs = readFileSync(`${ROOT}src-tauri/src/lib.rs`, 'utf8')
// Command identifiers can contain digits (e.g. e2e_get_config), and so can module
// names (e2e), so the char classes must include 0-9 to capture them — the renderer-
// call regex below already allows digits.
const registered = new Set(
  [...libRs.matchAll(/[a-z0-9_]+::([a-z0-9_]+)\s*,?/g)].map((m) => m[1])
)

// 2. window.api.<ns>.<method> calls across the renderer.
const grep = execSync(
  `grep -rhoIE "window\\.api\\.[a-zA-Z0-9_]+\\.[a-zA-Z0-9_]+" src --include="*.ts" --include="*.tsx"`,
  { cwd: ROOT, encoding: 'utf8' }
)
const counts = new Map()
for (const line of grep.split('\n')) {
  const m = line.match(/^window\.api\.([a-zA-Z0-9_]+)\.([a-zA-Z0-9_]+)$/)
  if (!m) continue
  const key = `${m[1]}.${m[2]}`
  counts.set(key, (counts.get(key) ?? 0) + 1)
}

// 3. Cross-reference. Event subscriptions (onX) resolve to listeners, not
//    commands, so flag them separately rather than as missing commands.
const byNamespace = new Map()
let covered = 0
let needed = 0
for (const [key, count] of counts) {
  const [ns, method] = key.split('.')
  const isEvent = /^on[A-Z]/.test(method)
  const cmd = pathToCommand(ns, method)
  const exists = registered.has(cmd)
  if (!isEvent) {
    needed++
    if (exists) covered++
  }
  const entry = byNamespace.get(ns) ?? []
  entry.push({ method, cmd, count, exists, isEvent })
  byNamespace.set(ns, entry)
}

// 4. Emit a prioritized markdown report (busiest namespaces first).
const namespaces = [...byNamespace.entries()]
  .map(([ns, methods]) => ({
    ns,
    methods: methods.sort((a, b) => b.count - a.count),
    calls: methods.reduce((s, m) => s + m.count, 0),
    missing: methods.filter((m) => !m.exists && !m.isEvent).length,
  }))
  .sort((a, b) => b.calls - a.calls)

let md = `# Orca Tauri Port — Command Coverage\n\n`
md += `Generated from renderer \`window.api.*\` usage vs. registered Rust commands.\n`
md += `Run \`node scripts/port-coverage.mjs\` to regenerate.\n\n`
md += `## Summary\n\n`
md += `- Backend command coverage: **${covered}/${needed}** (${((covered / needed) * 100).toFixed(1)}%)\n`
md += `- Distinct namespaces used by renderer: **${namespaces.length}**\n`
md += `- Registered Rust commands: **${registered.size}**\n\n`
md += `## Gap by namespace (busiest first)\n\n`
md += `| Namespace | Calls | Methods | Missing cmds |\n|---|---:|---:|---:|\n`
for (const n of namespaces) {
  md += `| \`${n.ns}\` | ${n.calls} | ${n.methods.length} | ${n.missing} |\n`
}
md += `\n## Method detail\n\n`
for (const n of namespaces) {
  md += `### \`${n.ns}\` — ${n.calls} calls, ${n.missing} missing\n\n`
  for (const m of n.methods) {
    const status = m.isEvent ? 'event' : m.exists ? '✅' : '❌'
    md += `- ${status} \`${m.method}\` → \`${m.cmd}\` (${m.count})\n`
  }
  md += `\n`
}

writeFileSync(`${ROOT}PORT_STATUS_COMMANDS.md`, md)
console.log(`coverage: ${covered}/${needed} commands (${((covered / needed) * 100).toFixed(1)}%)`)
console.log(`namespaces: ${namespaces.length}, registered: ${registered.size}`)
console.log(`wrote PORT_STATUS_COMMANDS.md`)
