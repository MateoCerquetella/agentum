// AC-4 (spec 009): the probe plan is exactly the pinned repo — one entry, no
// others. This is the unit assertion that no WikiPage mount can sweep every
// registered repo again (the TCC prompt storm).
import { describe, expect, it } from 'vitest'

import { wikiProbePlan } from './wiki-probe'

describe('wikiProbePlan', () => {
  it('probes exactly the pinned repo — one entry, nothing else', () => {
    expect(wikiProbePlan('repo-a')).toEqual(['repo-a'])
  })

  it('never grows beyond one entry, whatever the pinned id looks like', () => {
    for (const id of ['x', 'repo-123', 'a1b2c3d4-uuid-ish', 'ssh-host-repo']) {
      const plan = wikiProbePlan(id)
      expect(plan).toHaveLength(1)
      expect(plan[0]).toBe(id)
    }
  })
})
