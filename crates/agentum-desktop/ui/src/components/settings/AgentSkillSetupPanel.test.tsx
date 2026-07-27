import { renderToStaticMarkup } from 'react-dom/server'
import type { ComponentProps } from 'react'
import { describe, expect, it, vi } from 'vitest'
import { AgentSkillSetupPanel } from './AgentSkillSetupPanel'

function renderPanel(overrides: Partial<ComponentProps<typeof AgentSkillSetupPanel>> = {}): string {
  return renderToStaticMarkup(
    <AgentSkillSetupPanel
      title="CLI skill"
      description="Enables agents to use Agentum workflows."
      command="agentum capabilities"
      terminalTitle="CLI skill setup"
      terminalAriaLabel="CLI skill install terminal"
      terminalWorktreeId="settings-cli-skill-terminal"
      installed={false}
      loading={false}
      error={null}
      onRecheck={vi.fn()}
      {...overrides}
    />
  )
}

function buttonLabels(html: string): string[] {
  return Array.from(html.matchAll(/<button\b[^>]*>([\s\S]*?)<\/button>/g), ([, content]) =>
    content
      .replace(/<[^>]*>/g, '')
      .replace(/\s+/g, ' ')
      .trim()
  )
}

function buttonMarkupByLabel(html: string, label: string): string | undefined {
  return Array.from(html.matchAll(/<button\b[^>]*>[\s\S]*?<\/button>/g), ([button]) => button).find(
    (button) => buttonLabels(button).includes(label)
  )
}

describe('AgentSkillSetupPanel', () => {
  it('explains that capability setup is automatic', () => {
    const html = renderPanel({ installed: true })

    expect(html).toContain('Provided automatically')
    expect(html).toContain('MCP server')
    expect(buttonLabels(html)).toEqual([])
  })

  it('does not expose retired install or re-check controls', () => {
    const html = renderPanel({ installed: true, showRecheckWhenInstalled: false })

    expect(buttonLabels(html)).toEqual([])
    expect(html).not.toContain('Install')
    expect(html).not.toContain('Re-check')
  })

  it('keeps the automatic setup message before capability detection', () => {
    const html = renderPanel({ installed: false, showRecheckWhenInstalled: false })

    expect(html).toContain('no install needed')
    expect(buttonLabels(html)).toEqual([])
  })

  it('does not resurrect an install control when parent setup is disabled', () => {
    const html = renderPanel({ installDisabled: true })

    expect(buttonMarkupByLabel(html, 'Install')).toBeUndefined()
  })
})
