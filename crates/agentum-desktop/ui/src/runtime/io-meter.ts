// Per-host WS throughput counters — the desktop mirror of the TUI's iometer
// (crates/agentum-tui/src/commands/terminal/iometer.rs). Each server session
// streams its tmux pane over a bidirectional WebSocket: pane bytes come in,
// keystrokes/resizes go out. We bucket the cumulative byte totals by host so
// the status-bar chip can show "↓ in / ↑ out" for whichever host the user
// selects (local, or any SSH host).
//
// Design: plain module-level integer counters, no React state. The hot path
// (every WS frame) does one or two integer adds via record(). The status-bar
// hook samples snapshot() on an interval and derives the rate from the delta,
// so per-frame work stays out of React entirely.

/** Stable host bucket key. `'local'` for the daemon's own machine, `'ssh:<id>'`
 *  for a native SSH target — matching the HostKey scheme in the hosts slice. */
export type HostKey = string

/** The canonical key for the local host. */
export const LOCAL_HOST_KEY = 'local'

/** Cumulative byte totals for one host since the meter started. Monotonic. */
export type IoCounters = {
  bytesIn: number
  bytesOut: number
}

const counters = new Map<HostKey, IoCounters>()

function bucket(hostKey: HostKey): IoCounters {
  let entry = counters.get(hostKey)
  if (!entry) {
    entry = { bytesIn: 0, bytesOut: 0 }
    counters.set(hostKey, entry)
  }
  return entry
}

/** Accumulate WS bytes for a host. `delta.in` = bytes received (server→client),
 *  `delta.out` = bytes sent (client→server). Negative/non-finite values are
 *  ignored so a bad caller can't corrupt the running totals. */
export function record(hostKey: HostKey, delta: { in?: number; out?: number }): void {
  const inc = (n: number | undefined): number =>
    typeof n === 'number' && Number.isFinite(n) && n > 0 ? n : 0
  const addIn = inc(delta.in)
  const addOut = inc(delta.out)
  if (addIn === 0 && addOut === 0) {
    return
  }
  const entry = bucket(hostKey)
  entry.bytesIn += addIn
  entry.bytesOut += addOut
}

/** Current cumulative totals for a host. Returns zeros for an unseen host so
 *  callers never have to null-check. The returned object is a copy — mutating
 *  it does not affect the stored counters. */
export function snapshot(hostKey: HostKey): IoCounters {
  const entry = counters.get(hostKey)
  return entry ? { bytesIn: entry.bytesIn, bytesOut: entry.bytesOut } : { bytesIn: 0, bytesOut: 0 }
}

/** Which hosts have recorded any traffic. Used as a hint, not as the selector's
 *  source of truth (the selector lists configured hosts regardless of traffic). */
export function knownHostKeys(): HostKey[] {
  return [...counters.keys()]
}

/** Test-only: wipe all counters so cases don't bleed into each other. */
export function resetAll(): void {
  counters.clear()
}

/**
 * Bytes/sec between two cumulative samples. Mirrors `IoMeter::rate` in the TUI:
 * Δbytes / Δtime, with a 1ms floor on the span so two samples in the same tick
 * don't divide by ~0. Guards the first sample (no prior) and counter resets
 * (delta < 0) by returning 0.
 */
export function rateFromSamples(
  prev: { bytes: number; at: number } | null,
  next: { bytes: number; at: number }
): number {
  if (!prev) {
    return 0
  }
  const deltaBytes = next.bytes - prev.bytes
  if (deltaBytes <= 0) {
    return 0
  }
  const spanMs = Math.max(1, next.at - prev.at)
  return (deltaBytes / spanMs) * 1000
}

/**
 * Compact human-readable rate, mirroring `fmt_rate` in the TUI's iometer:
 * B/s → KiB/s → MiB/s → GiB/s on 1024 boundaries, one decimal under 10,
 * none above. A rate under 1 B/s reads as an em dash so an idle host doesn't
 * scream "0 B/s". (We use the "/s" suffix the chip displays alongside ↓/↑.)
 */
export function formatRate(bps: number): string {
  if (!Number.isFinite(bps) || bps < 1) {
    return '—'
  }
  const KIB = 1024
  const MIB = KIB * 1024
  const GIB = MIB * 1024
  let val: number
  let unit: string
  if (bps < KIB) {
    val = bps
    unit = 'B/s'
  } else if (bps < MIB) {
    val = bps / KIB
    unit = 'KiB/s'
  } else if (bps < GIB) {
    val = bps / MIB
    unit = 'MiB/s'
  } else {
    val = bps / GIB
    unit = 'GiB/s'
  }
  const num = val < 10 ? val.toFixed(1) : Math.round(val).toString()
  return `${num} ${unit}`
}
