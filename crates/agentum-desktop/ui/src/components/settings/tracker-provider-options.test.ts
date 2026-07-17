import { describe, expect, it } from 'vitest'
import type { Repo } from '../../../../shared/types'
import { getRepositoryPaneSearchEntries } from './repository-search'
import { matchesSettingsSearch } from './settings-search'
import {
  parseTrackerProviderPreference,
  resolveTrackerProviderPreference,
  TRACKER_PROVIDER_OPTIONS
} from './tracker-provider-options'

const repo: Repo = {
  id: 'repo-1',
  path: '/tmp/repo',
  displayName: 'Example Repo',
  badgeColor: '#000000',
  addedAt: 1,
  kind: 'git'
}

describe('tracker provider picker model', () => {
  it('offers exactly Auto (detect), GitHub, Linear, and None', () => {
    expect(TRACKER_PROVIDER_OPTIONS.map((option) => [option.value, option.label])).toEqual([
      ['auto', 'Auto (detect)'],
      ['github', 'GitHub'],
      ['linear', 'Linear'],
      ['none', 'None']
    ])
  })

  it('defaults an absent choice to auto and round-trips saved values', () => {
    expect(resolveTrackerProviderPreference(undefined)).toBe('auto')
    for (const option of TRACKER_PROVIDER_OPTIONS) {
      expect(resolveTrackerProviderPreference(option.value)).toBe(option.value)
    }
  })

  it('drops unknown select values instead of saving them', () => {
    expect(parseTrackerProviderPreference('github')).toBe('github')
    expect(parseTrackerProviderPreference('jira')).toBeNull()
    expect(parseTrackerProviderPreference('')).toBeNull()
  })

  it('is reachable through settings search for git repos but not folders', () => {
    const entries = getRepositoryPaneSearchEntries(repo)
    expect(entries.some((entry) => entry.title === 'Tracker')).toBe(true)
    expect(matchesSettingsSearch('tracker', entries)).toBe(true)
    expect(matchesSettingsSearch('linear', entries)).toBe(true)

    const folderEntries = getRepositoryPaneSearchEntries({ ...repo, kind: 'folder' })
    expect(folderEntries.some((entry) => entry.title === 'Tracker')).toBe(false)
  })
})
