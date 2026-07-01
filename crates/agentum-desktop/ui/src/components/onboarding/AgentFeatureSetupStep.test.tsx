import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import { AgentFeatureSetupStep } from './AgentFeatureSetupStep'

describe('AgentFeatureSetupStep', () => {
  it('renders the agent MCP tools as toggles', () => {
    const html = renderToStaticMarkup(
      <AgentFeatureSetupStep
        featureSetup={{
          browserUse: true,
          computerUse: true,
          orchestration: true
        }}
        onFeatureSetupChange={vi.fn()}
      />
    )

    expect(html).toContain('Agent MCP tools')
    expect(html).toContain('Agent Browser Use')
    expect(html).toContain('Computer Use')
    expect(html).toContain('Agent Orchestration')
    // Toggle switches, not select-cards or an "Enable capabilities" button.
    expect(html).toContain('role="switch"')
    expect(html).not.toContain('Enable capabilities')
    expect(html).not.toContain('role="checkbox"')
  })
})
