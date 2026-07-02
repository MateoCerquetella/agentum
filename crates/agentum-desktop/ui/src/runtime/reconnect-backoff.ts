// The ONE reconnect-backoff policy for every server-facing socket/stream in
// the renderer (events bus, session streams, harness events, sidebar
// bootstrap). Capped exponential with jitter — same shape as the TUI's loop.
// Keep it here rather than per-module: identical inline copies drifted into
// five files before this was extracted, and a policy change (ceiling, jitter,
// attempt cap) must apply to all reconnect paths at once.
export const reconnectBackoffMs = (attempt: number): number =>
  Math.min(5000, 250 * 2 ** Math.min(attempt - 1, 5)) + Math.floor(Math.random() * 250)
