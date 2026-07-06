// The Wiki view state machine (spec 009 F3) — the pure, vitest-covered core
// behind WikiPage's push-based updates. Two inputs feed the view:
//
//   - `GET /api/wiki` responses — the ONLY source of every state, including
//     `ready` (the server's validated-index path is the sole gate; D-A6
//     "discriminator honesty"). WikiPage sets those directly.
//   - `wiki.updated` bus events — handled HERE. A `running` frame merges the
//     progressively-written page slugs into a current running state; a
//     `ready`/`failed` frame produces a REFETCH command, never a state flip
//     (the event doesn't carry enough to build an honest Ready, and must not).
//
// This module also absorbs the F2 probe-plan precursor module (`wikiProbePlan`,
// the AC-4 one-repo-only contract) and owns the prettified-slug titles the
// progressive TOC renders until the validated index supplies real ones.
import type { WikiIndexResponse } from '@/runtime/wiki-client'

/** The repos a WikiPage mount probes: the pinned repo, nothing else (AC-4 —
 *  the every-repo sweep this replaced was the macOS TCC-prompt-storm trigger). */
export function wikiProbePlan(pinnedRepoId: string): string[] {
  return [pinnedRepoId]
}

/** What the caller must do after reducing an input: nothing, or re-issue the
 *  authoritative `GET /api/wiki` for the pinned repo. */
export type WikiViewCommand = 'none' | 'refetch'

export type WikiEventOutcome = {
  /** The (possibly progressively-merged) view state. Reference-equal to the
   *  input when the event changed nothing — callers can skip a re-render. */
  index: WikiIndexResponse | null
  command: WikiViewCommand
}

/** A `/api/events` frame as the shared bus delivers it (`ServerEventFrame`
 *  shape, declared structurally so this module stays dependency-free). */
export type WikiEventFrame = { kind?: unknown; payload?: unknown }

/** Kebab/underscore slug → Title Case ("getting-started" → "Getting Started")
 *  for progressive TOC entries; the validated index replaces these at Ready. */
export function prettifySlug(slug: string): string {
  const words = slug.split(/[-_]+/).filter(Boolean)
  if (words.length === 0) return slug
  return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')
}

/** The socket (re)open contract (bus docs + D-A5): any snapshot a consumer
 *  depends on must be refetched on (re)connect — reconnect gaps heal here,
 *  which is exactly why no fallback poll exists. */
export function commandForSocketOpen(): WikiViewCommand {
  return 'refetch'
}

/** Union of the slugs already rendered and the frame's listing, sorted. Union
 *  (not replace) so an out-of-order/early `pages: []` frame — e.g. the
 *  generate request path's initial emit — can never contract a TOC the GET
 *  already populated. */
function mergeSlugs(current: string[], incoming: string[]): string[] {
  return [...new Set([...current, ...incoming])].sort()
}

/**
 * Reduce one bus frame into the view. The load-bearing rule (D-A6): this
 * function can only ever (a) merge pages into an EXISTING running state or
 * (b) command a refetch — it never constructs `ready` (or any other state)
 * from an event. `ready` is reachable solely via a `GET /api/wiki` response.
 */
export function applyWikiEvent(
  current: WikiIndexResponse | null,
  pinnedRepoId: string,
  frame: WikiEventFrame
): WikiEventOutcome {
  const unchanged: WikiEventOutcome = { index: current, command: 'none' }
  if (frame.kind !== 'wiki.updated') return unchanged
  const payload = frame.payload
  if (!payload || typeof payload !== 'object') return unchanged
  const { repo_id, status, pages } = payload as {
    repo_id?: unknown
    status?: unknown
    pages?: unknown
  }
  // Another project's wiki — not ours to react to.
  if (repo_id !== pinnedRepoId) return unchanged
  if (status === 'ready' || status === 'failed') {
    // Refetch, never flip: the GET returns the validated Ready (or the
    // recorded Failed{error} — the event carries no error detail either).
    return { index: current, command: 'refetch' }
  }
  if (status !== 'running') return unchanged
  if (current?.state !== 'running') {
    // A run just started (this window's own generate, or another client's).
    // The GET carries the authoritative Running{sessionId, pages}.
    return { index: current, command: 'refetch' }
  }
  const incoming = Array.isArray(pages)
    ? pages.filter((s): s is string => typeof s === 'string')
    : []
  const merged = mergeSlugs(current.pages ?? [], incoming)
  // Growth-only by construction (union): same length = nothing new landed.
  if (merged.length === (current.pages ?? []).length) return unchanged
  return { index: { ...current, pages: merged }, command: 'none' }
}
