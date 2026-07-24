import { describe, expect, it } from 'vitest'
import { TUI_AGENT_CONFIG } from '../../../../shared/tui-agent-config'
import type { TuiAgent } from '../../../../shared/types'
import { resolveSddToolbarAgent } from '../../lib/sdd-toolbar-agent'

describe('resolveSddToolbarAgent', () => {
  it('keeps the persisted agent authoritative through missing live evidence', () => {
    expect(
      resolveSddToolbarAgent({
        sessionTool: 'codex',
        requestedAgent: 'claude',
        liveAgent: null
      })
    ).toBe('codex')
  })

  it('allows a manually started recognized agent in a terminal session', () => {
    expect(
      resolveSddToolbarAgent({ sessionTool: 'terminal', liveAgent: 'claude' })
    ).toBe('claude')
    expect(resolveSddToolbarAgent({ sessionTool: 'terminal', liveAgent: null })).toBeNull()
  })

  it('keeps every desktop-supported requested agent eligible before binding', () => {
    for (const agent of Object.keys(TUI_AGENT_CONFIG) as TuiAgent[]) {
      expect(
        resolveSddToolbarAgent({
          sessionTool: undefined,
          requestedAgent: agent,
          liveAgent: null
        })
      ).toBe(agent)
    }
  })

  it('keeps requested identity through a transient session lookup failure', () => {
    expect(
      resolveSddToolbarAgent({
        sessionTool: null,
        requestedAgent: 'codex',
        liveAgent: null
      })
    ).toBe('codex')
  })
})
