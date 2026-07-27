import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { buildSettingsNavigationMetadata } from './useSettingsNavigationMetadata'
import { buildCmdJSettingsResults } from '../components/cmd-j/palette-results'
import type { Repo } from '@/shared/types'

const repo = {
  id: 'repo-1',
  path: '/repo',
  displayName: 'Repo',
  badgeColor: '#000',
  addedAt: 0
} satisfies Repo

function ids(args: { isMac?: boolean; isWindows?: boolean; isWebClient?: boolean } = {}): string[] {
  return buildSettingsNavigationMetadata({
    isMac: args.isMac ?? false,
    isWindows: args.isWindows ?? false,
    isWebClient: args.isWebClient ?? false,
    repos: [repo]
  }).map((section) => section.id)
}

describe('settings navigation metadata', () => {
  it('puts AI capability panes at the top on desktop', () => {
    expect(ids().slice(0, 7)).toEqual([
      'agents',
      'accounts',
      'agents-automation',
      'voice',
      'general',
      'integrations',
      'git'
    ])
  })

  it('puts web-safe AI capability panes at the top while hiding desktop-only panes', () => {
    expect(ids({ isWebClient: true }).slice(0, 6)).toEqual([
      'agents',
      'accounts',
      'agents-automation',
      'general',
      'integrations',
      'git'
    ])
  })

  it('keeps desktop-only Settings panes out of web metadata', () => {
    const webIds = ids({ isWebClient: true })

    expect(webIds).not.toContain('browser')
    expect(webIds).not.toContain('ssh')
    expect(webIds).not.toContain('mobile')
    expect(webIds).not.toContain('computer-use')
    expect(webIds).not.toContain('voice')
    expect(webIds).not.toContain('servers')
    expect(webIds).toContain('repo-repo-1')
  })

  it('Cmd+J / Cmd+Shift+P settings results exclude Phase-1-removed sections', () => {
    // Why: the command palette (Cmd+Shift+P) reuses buildCmdJSettingsResults
    // over the single navigation registry, so removing a section from the
    // registry must drop it from the palette automatically.
    const sections = buildSettingsNavigationMetadata({
      isMac: false,
      isWindows: false,
      isWebClient: false,
      repos: [repo]
    })
    const resultSectionIds = buildCmdJSettingsResults(sections).map((result) => result.sectionId)
    expect(resultSectionIds).not.toContain('floating-workspace')
    expect(resultSectionIds).not.toContain('servers')
    expect(resultSectionIds).not.toContain('privacy')
  })

  it('keeps macOS permissions mac-only', () => {
    expect(ids({ isMac: false })).not.toContain('developer-permissions')
    expect(ids({ isMac: true })).toContain('developer-permissions')
  })

  it('keeps every settings navigation page paired with a rendered configuration surface', () => {
    const testDir = dirname(fileURLToPath(import.meta.url))
    const settingsSource = readFileSync(
      resolve(testDir, '../components/settings/Settings.tsx'),
      'utf8'
    )
    const renderedSectionIds = Array.from(
      settingsSource.matchAll(/<SettingsSection\s+[\s\S]*?\bid="([^"]+)"/g),
      (match) => match[1]
    )
    const registeredSectionIds = buildSettingsNavigationMetadata({
      isMac: true,
      isWindows: false,
      isWebClient: false,
      repos: []
    }).map((section) => section.id)

    expect(new Set(renderedSectionIds).size).toBe(renderedSectionIds.length)
    expect(renderedSectionIds.sort()).toEqual(registeredSectionIds.sort())
    expect(settingsSource).toContain('id={repoSectionId}')
    expect(settingsSource).toContain('<RepositoryPane')
  })

  it('does not import Settings page or pane UI modules from the metadata hook', () => {
    const testDir = dirname(fileURLToPath(import.meta.url))
    const hookSource = readFileSync(resolve(testDir, 'useSettingsNavigationMetadata.ts'), 'utf8')
    const importLines = hookSource
      .split('\n')
      .filter((line) => line.trim().startsWith('import '))
      .join('\n')

    expect(importLines).not.toMatch(/components\/settings\/Settings(?:'|")/)
    expect(importLines).not.toMatch(/components\/settings\/[A-Z][A-Za-z]+Pane(?:'|")/)
    expect(importLines).not.toMatch(/components\/stats\/StatsPane(?:'|")/)
  })

  it('does not import Settings page or pane UI modules from the quick action registry', () => {
    const testDir = dirname(fileURLToPath(import.meta.url))
    const registrySource = readFileSync(
      resolve(testDir, '../components/cmd-j/quick-actions.ts'),
      'utf8'
    )
    const importLines = registrySource
      .split('\n')
      .filter((line) => line.trim().startsWith('import '))
      .join('\n')

    expect(importLines).not.toMatch(/components\/settings\/Settings(?:'|")/)
    expect(importLines).not.toMatch(/components\/settings\/[A-Z][A-Za-z]+Pane(?:'|")/)
    expect(importLines).not.toMatch(/components\/stats\/StatsPane(?:'|")/)
  })
})
