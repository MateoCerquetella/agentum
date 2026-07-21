// Spec 023 Part A — markup pins for the surface gate, rendered via
// renderToStaticMarkup (the GatedRunBar.test.tsx pattern; no jsdom, effects
// never fire — these pin the RENDERED decision only). The store, hook, and
// launcher are mocked module-level.
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import type { HarnessStatus } from '@/runtime/harness-client'

let sliceEntry: { worktreeId: string; workdir: string } | undefined = {
  worktreeId: 'wt-1',
  workdir: '/workspace/feature'
}

vi.mock('@/store', () => ({
  useAppStore: (selector: (s: unknown) => unknown) =>
    selector({
      gatedRunStartingByWorktreeId: sliceEntry ? { 'wt-1': sliceEntry } : {},
      clearGatedRunStarting: () => {},
      worktreesByRepo: {
        'repo-1': [{ id: 'wt-1', repoId: 'repo-1', path: '/workspace/feature' }]
      }
    })
}))

let hookRun: HarnessStatus | undefined

vi.mock('@/hooks/useWorktreeHarnessRun', () => ({
  useWorktreeHarnessRun: () => ({ run: hookRun, refresh: () => {} })
}))

vi.mock('../WorkspaceAgentLauncher', () => ({
  default: () => <div>LAUNCHER-PICKER</div>
}))

function makeRun(overrides: Partial<HarnessStatus> = {}): HarnessStatus {
  return {
    id: 'run-1',
    workdir: '/workspace/feature',
    state: 'running',
    phase: 'executing',
    current_feature: 'F1',
    features: {
      features: [
        {
          id: 'F1',
          name: 'Build the thing',
          description: '',
          state: 'coding',
          attempts: 1
        }
      ],
      max_retries: 2,
      agent_tool: 'claude',
      settle_grace_secs: 8,
      settle_timeout_secs: 1200,
      agent_yolo: true
    },
    elapsed_secs: 3,
    agent_instructions: '',
    ...overrides
  }
}

describe('GatedRunSurface (AC 1)', () => {
  it('pending + no run snapshot yet → the starting state, not the picker', async () => {
    sliceEntry = { worktreeId: 'wt-1', workdir: '/workspace/feature' }
    hookRun = undefined
    const { default: GatedRunSurface } = await import('./GatedRunSurface')
    const html = renderToStaticMarkup(<GatedRunSurface worktreeId="wt-1" />)
    expect(html).toContain('Gated run starting')
    expect(html).not.toContain('LAUNCHER-PICKER')
  })

  it('pending + booting run → starting, reflecting the live state/feature', async () => {
    sliceEntry = { worktreeId: 'wt-1', workdir: '/workspace/feature' }
    hookRun = makeRun()
    const { default: GatedRunSurface } = await import('./GatedRunSurface')
    const html = renderToStaticMarkup(<GatedRunSurface worktreeId="wt-1" />)
    expect(html).toContain('Gated run starting')
    expect(html).toContain('running · executing · Build the thing')
  })

  it('pending + halted run → falls back to the picker', async () => {
    sliceEntry = { worktreeId: 'wt-1', workdir: '/workspace/feature' }
    hookRun = makeRun({ state: 'failed' })
    const { default: GatedRunSurface } = await import('./GatedRunSurface')
    const html = renderToStaticMarkup(<GatedRunSurface worktreeId="wt-1" />)
    expect(html).toContain('LAUNCHER-PICKER')
    expect(html).not.toContain('Gated run starting')
  })

  it('no pending slice → the plain picker (AC 3 fallback preserved)', async () => {
    sliceEntry = undefined
    hookRun = makeRun()
    const { default: GatedRunSurface } = await import('./GatedRunSurface')
    const html = renderToStaticMarkup(<GatedRunSurface worktreeId="wt-1" />)
    expect(html).toContain('LAUNCHER-PICKER')
    expect(html).not.toContain('Gated run starting')
  })
})
