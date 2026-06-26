/**
 * Narrow an `unknown` to a plain object (non-null, non-array). Parse boundaries
 * across the app — notebook JSON, settings blobs, package.json — all need this
 * guard; keeping one definition means a fix to the predicate lands in one place
 * instead of drifting across five hand-rolled copies.
 */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}
