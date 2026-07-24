import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('ProjectIntegrationsSection', () => {
  it('owns the canonical provider, GitHub, and Linear controls under one stable target', () => {
    const source = readFileSync(
      new URL('./ProjectIntegrationsSection.tsx', import.meta.url),
      'utf8'
    )

    expect(source).toContain('project-integrations')
    expect(source).toContain('projectTrackerConfigByRepo[repo.id]')
    expect(source).toContain('saveProjectTrackerConfig')
    expect(source).toContain('ProjectBindingEditor')
    expect(source).toContain('Without a Project binding, issue lists fall back')
    expect(source).toContain('Save Linear tracker')
    expect(source).not.toContain('updateRepo(')
  })
})
