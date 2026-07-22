import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('Project Tasks structural isolation', () => {
  it('dispatches only locked providers and links to repo settings', () => { const source = readFileSync(new URL('./ProjectTasksPage.tsx', import.meta.url), 'utf8'); expect(source).toContain('lockedScope={scope}'); expect(source).toContain('LockedLinearProjectTasks'); expect(source).toContain("pane: 'repo'") })
  it('Project Hub no longer embeds global TaskPage or binding controls', () => { const source = readFileSync(new URL('./ProjectHubPage.tsx', import.meta.url), 'utf8'); expect(source).not.toContain("@/components/TaskPage"); expect(source).not.toContain('ProjectBindingEditor'); expect(source).toContain('ProjectTasksPage') })

  it('renders only external tracker sources or the settings empty state', () => {
    const source = readFileSync(new URL('./ProjectTasksPage.tsx', import.meta.url), 'utf8')

    expect(source).toContain("repo.trackerProvider !== 'github'")
    expect(source).toContain("repo.trackerProvider !== 'linear'")
    expect(source).toContain('ProjectViewWrapper')
    expect(source).toContain('LockedLinearProjectTasks')
    expect(source).toContain('No tracker is configured for')
    expect(source).toContain('configured tracker is unavailable')
    expect(source).toContain('Choose a GitHub or Linear tracker in Project Settings.')
    expect(source).not.toContain('internal board')
  })

  it('global tasks has no internal board sync affordance', () => {
    const source = readFileSync(new URL('../TaskPage.tsx', import.meta.url), 'utf8')

    expect(source).not.toContain("@/runtime/board-client")
    expect(source).not.toContain('syncExternalIssues')
    expect(source).not.toContain('syncTasksToBoard')
    expect(source).not.toContain('Send these issues to the Board')
  })
})
