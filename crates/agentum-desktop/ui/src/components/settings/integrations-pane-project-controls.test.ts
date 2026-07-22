import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('global Integrations project controls', () => {
  it('contains no repo selector or project binding editor', () => { const source = readFileSync(new URL('./IntegrationsPane.tsx', import.meta.url), 'utf8'); expect(source).not.toContain('GithubProjectsBoardEditor'); expect(source).not.toContain('ProjectBindingEditor') })
})
