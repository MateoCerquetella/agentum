import { beforeEach, describe, expect, it } from 'vitest'
import { clearProjectTaskScopeAuthoritiesForTests, isLiveProjectTaskScopeAuthority, publishProjectTaskScopeAuthority, runGuardedProjectTaskAction } from './project-task-scope-authority'

const guard = { repoId: 'r', scopeKey: 'key', generation: 4 } as const
beforeEach(clearProjectTaskScopeAuthoritiesForTests)

describe('project task scope authority', () => {
  it('rejects the same binding from an older generation', () => {
    publishProjectTaskScopeAuthority({ ...guard, generation: 5 })
    expect(isLiveProjectTaskScopeAuthority(guard)).toBe(false)
  })
  it('cleans up only the guard that still owns the repo', () => {
    const oldCleanup = publishProjectTaskScopeAuthority(guard)
    publishProjectTaskScopeAuthority({ ...guard, generation: 5 })
    oldCleanup()
    expect(isLiveProjectTaskScopeAuthority({ ...guard, generation: 5 })).toBe(true)
  })
  it('discards a deferred mutation result when the same binding advances generation', async () => {
    publishProjectTaskScopeAuthority(guard)
    let resolve!: (value: string) => void
    const deferred = new Promise<string>((done) => { resolve = done })
    const applied: string[] = []
    const pending = runGuardedProjectTaskAction(
      () => isLiveProjectTaskScopeAuthority(guard),
      () => deferred,
      (value) => applied.push(value)
    )
    publishProjectTaskScopeAuthority({ ...guard, generation: 5 })
    resolve('stale')
    await expect(pending).resolves.toBe(false)
    expect(applied).toEqual([])
  })
})
