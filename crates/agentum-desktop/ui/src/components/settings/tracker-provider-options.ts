import type { TrackerProviderPreference } from '../../../../shared/types'

/** Option model for the per-project Tracker picker, in render order. */
export const TRACKER_PROVIDER_OPTIONS: ReadonlyArray<{
  value: TrackerProviderPreference
  label: string
}> = [
  { value: 'auto', label: 'Auto (detect)' },
  { value: 'github', label: 'GitHub' },
  { value: 'linear', label: 'Linear' },
  { value: 'none', label: 'None' }
]

/** Absent choice renders (and behaves) as `'auto'` — see `Repo.trackerProvider`. */
export function resolveTrackerProviderPreference(
  value: TrackerProviderPreference | undefined
): TrackerProviderPreference {
  return value ?? 'auto'
}

/** Narrows the string a Select hands back; unknown values are dropped, not saved. */
export function parseTrackerProviderPreference(value: string): TrackerProviderPreference | null {
  return TRACKER_PROVIDER_OPTIONS.some((option) => option.value === value)
    ? (value as TrackerProviderPreference)
    : null
}
