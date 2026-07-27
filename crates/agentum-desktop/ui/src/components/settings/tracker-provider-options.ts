import type { TrackerProviderPreference } from '@/shared/types'

/** Option model for the per-project Tracker picker, in render order. */
export const TRACKER_PROVIDER_OPTIONS: ReadonlyArray<{
  value: TrackerProviderPreference
  label: string
}> = [
  { value: 'auto', label: 'Auto (detect)' },
  { value: 'github', label: 'GitHub' },
  { value: 'linear', label: 'Linear' }
]

/** Absent choice renders (and behaves) as `'auto'` — see `Repo.trackerProvider`.
 *  Takes `string` because persisted records can carry values the current union
 *  no longer admits (a pre-amendment build offered `'none'`); those degrade to
 *  `'auto'` rather than rendering a value the picker can't show or the server
 *  would reject. */
export function resolveTrackerProviderPreference(
  value: string | undefined
): TrackerProviderPreference {
  return value == null ? 'auto' : (parseTrackerProviderPreference(value) ?? 'auto')
}

/** Narrows the string a Select hands back; unknown values are dropped, not saved. */
export function parseTrackerProviderPreference(value: string): TrackerProviderPreference | null {
  return TRACKER_PROVIDER_OPTIONS.some((option) => option.value === value)
    ? (value as TrackerProviderPreference)
    : null
}
