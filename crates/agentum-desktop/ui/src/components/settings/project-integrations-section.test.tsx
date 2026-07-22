import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('ProjectIntegrationsSection', () => {
  it('owns provider, GitHub, and Linear controls under one stable target', () => { const source = readFileSync(new URL('./ProjectIntegrationsSection.tsx', import.meta.url), 'utf8'); expect(source).toContain('project-integrations'); expect(source).toContain('ProjectBindingEditor'); expect(source).toContain('Save Linear board') })
})
