import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('Project Tasks structural isolation', () => {
  it('dispatches only locked providers and links to repo settings', () => { const source = readFileSync(new URL('./ProjectTasksPage.tsx', import.meta.url), 'utf8'); expect(source).toContain('lockedScope={scope}'); expect(source).toContain('LockedLinearProjectTasks'); expect(source).toContain("pane: 'repo'") })
  it('Project Hub no longer embeds global TaskPage or binding controls', () => { const source = readFileSync(new URL('./ProjectHubPage.tsx', import.meta.url), 'utf8'); expect(source).not.toContain("@/components/TaskPage"); expect(source).not.toContain('ProjectBindingEditor'); expect(source).toContain('ProjectTasksPage') })
})
