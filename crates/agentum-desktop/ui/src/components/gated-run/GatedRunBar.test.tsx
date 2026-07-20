// Spec 023 Part B — markup pins for the gated-run strip, rendered via
// renderToStaticMarkup (the HarnessSpecBanner.test.tsx pattern; no jsdom).
// The store, hook, and harness client are mocked module-level; the component
// module is imported dynamically inside the tests so the mock factories never
// hit the TDZ.
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import type { HarnessStatus } from '@/runtime/harness-client'

const STORE_STATE = {
  worktreesByRepo: {
    'repo-1': [{ id: 'wt-1', repoId: 'repo-1', path: '/workspace/feature' }]
  }
}

vi.mock('@/store', () => ({
  useAppStore: (selector: (s: typeof STORE_STATE) => unknown) => selector(STORE_STATE)
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
          attempts: 1,
          tracker_provider: 'github',
          tracker_url: 'https://github.com/o/r/issues/42'
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

let hookState: { run: HarnessStatus | undefined; refresh: () => void } = {
  run: makeRun(),
  refresh: () => {}
}

vi.mock('@/hooks/useWorktreeHarnessRun', () => ({
  useWorktreeHarnessRun: () => hookState
}))

vi.mock('@/runtime/harness-client', () => ({
  unlinkHarnessIssue: vi.fn(() => Promise.resolve())
}))

vi.mock('sonner', () => ({
  toast: { success: vi.fn(), error: vi.fn() }
}))

async function importBar(): Promise<typeof import('./GatedRunBar')> {
  hookState = { run: makeRun(), refresh: () => {} }
  return await import('./GatedRunBar')
}

describe('GatedRunBar (host)', () => {
  it('renders the strip with run detail + the linked-issue chip', async () => {
    const { default: GatedRunBar } = await importBar()
    const html = renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)
    expect(html).toContain('Gated run')
    expect(html).toContain('running · executing · Build the thing')
    expect(html).toContain('#42')
    expect(html).toContain('Unlink issue')
    // Load-bearing vs the launcher overlay's z-20: the strip must paint above.
    expect(html).toContain('z-30')
  })

  it('renders nothing when no run owns the worktree', async () => {
    const { default: GatedRunBar } = await importBar()
    hookState = { run: undefined, refresh: () => {} }
    expect(renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)).toBe('')
  })

  it('renders nothing for a finished run (nothing left to unlink)', async () => {
    const { default: GatedRunBar } = await importBar()
    hookState = { run: makeRun({ state: 'done' }), refresh: () => {} }
    expect(renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)).toBe('')
  })

  it('renders no chip once the run is unlinked', async () => {
    const { default: GatedRunBar } = await importBar()
    const unlinked = makeRun()
    unlinked.features.features[0].tracker_provider = null
    unlinked.features.features[0].tracker_url = null
    hookState = { run: unlinked, refresh: () => {} }
    const html = renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)
    expect(html).toContain('Gated run')
    expect(html).not.toContain('Unlink issue')
  })
})

describe('GatedRunBarView', () => {
  it('arms the two-tap confirm instead of unlinking on the first click', async () => {
    const { GatedRunBarView } = await importBar()
    const html = renderToStaticMarkup(
      <GatedRunBarView
        state="running"
        phase={null}
        currentFeature={null}
        issueLabel="#42"
        busy={false}
        arming={true}
        onUnlink={() => {}}
      />
    )
    expect(html).toContain('Confirm unlink')
  })

  it('busy disables the unlink button', async () => {
    const { GatedRunBarView } = await importBar()
    const html = renderToStaticMarkup(
      <GatedRunBarView
        state="running"
        phase={null}
        currentFeature={null}
        issueLabel="#42"
        busy={true}
        arming={false}
        onUnlink={() => {}}
      />
    )
    expect(html).toContain('disabled=""')
  })
})
