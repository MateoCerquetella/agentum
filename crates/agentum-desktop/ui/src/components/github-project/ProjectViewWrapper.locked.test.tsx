import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('locked GitHub board', () => {
  it('bypasses global resolution and hides ProjectPicker', () => { const source = readFileSync(new URL('./ProjectViewWrapper.tsx', import.meta.url), 'utf8'); expect(source).toContain("? { source: 'binding'"); expect(source).toContain('!lockedScope ? <ProjectPicker') })
  it('guards async results and opens scoped workspace composers', () => { const source = readFileSync(new URL('./ProjectViewWrapper.tsx', import.meta.url), 'utf8'); expect(source).toContain('isLiveProjectTaskScopeAuthority(lockedGuard)'); expect(source).toContain('runGuardedProjectTaskAction'); expect(source).toContain('requiredProjectTaskScope: lockedGuard'); expect(source).toContain('if (!guardCurrent()) return') })
})
