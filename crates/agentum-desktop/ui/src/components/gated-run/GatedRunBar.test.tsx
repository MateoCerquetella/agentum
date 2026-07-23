// Spec 023 Part B — markup pins for the gated-run strip, rendered via
// renderToStaticMarkup (the HarnessSpecBanner.test.tsx pattern; no jsdom).
// The store, hook, and harness client are mocked module-level; the component
// module is imported dynamically inside the tests so the mock factories never
// hit the TDZ.
import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import type { FeatureState, HarnessStatus } from '@/runtime/harness-client'

const STORE_STATE = {
  gatedRunStartingByWorktreeId: {},
  worktreesByRepo: {
    'repo-1': [{ id: 'wt-1', repoId: 'repo-1', path: '/workspace/feature' }]
  }
}

vi.mock('@/store', () => ({
  useAppStore: (selector: (s: typeof STORE_STATE) => unknown) => selector(STORE_STATE)
}))

// This suite is also run by the harness from the repository root, where the
// UI Vite aliases are not loaded. Keep every aliased dependency behind the
// same explicit test boundary as the store, hook, and runtime client.
vi.mock('@/shared/tui-agent-config', () => ({
  isTuiAgent: (tool: string) => tool === 'claude'
}))

vi.mock('@/lib/utils', async () => await import('../../lib/utils'))
vi.mock('@/lib/harness-run', async () => await import('../../lib/harness-run'))

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
        },
        {
          id: 'F2',
          name: 'Ship the thing',
          description: '',
          state: 'pending',
          attempts: 0
        }
      ],
      max_retries: 2,
      agent_tool: 'claude',
      settle_grace_secs: 8,
      settle_timeout_secs: 1200,
      agent_yolo: true,
      roles: true
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
    expect(html.trim()).not.toBe('')
    expect(html.match(/aria-label="Gated run progress"/g)).toHaveLength(1)
    expect(html).toContain('Gated run')
    expect(html).toContain('Working on Build the thing')
    expect(html).toContain('PM spec')
    expect(html).toContain('Architecture')
    expect(html).toContain('Build the thing')
    expect(html).toContain('0/2 complete')
    expect(html).toContain('#42')
    expect(html).toContain('Unlink issue')
    // Load-bearing vs the launcher overlay's z-20: the strip must paint above.
    expect(html).toContain('z-30')
  })

  it('renders lifecycle copy for coding, verification, browser QA, blocked, and complete', async () => {
    const { default: GatedRunBar } = await importBar()
    const cases: Array<{
      runState: HarnessStatus['state']
      featureState: HarnessStatus['features']['features'][number]['state']
      expected: string
    }> = [
      { runState: 'running', featureState: 'coding', expected: 'Working on Build the thing' },
      { runState: 'running', featureState: 'verifying', expected: 'Verifying Build the thing' },
      {
        runState: 'running',
        featureState: 'ready_to_test',
        expected: 'Browser QA for Build the thing'
      },
      { runState: 'blocked', featureState: 'blocked', expected: 'Blocked on Build the thing' },
      { runState: 'done', featureState: 'done', expected: 'Gated run complete' }
    ]

    for (const testCase of cases) {
      const run = makeRun({ state: testCase.runState })
      run.features.features[0].state = testCase.featureState
      hookState = { run, refresh: () => {} }
      const html = renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)
      expect(html.trim()).not.toBe('')
      expect(html.match(/aria-label="Gated run progress"/g)).toHaveLength(1)
      expect(html).toContain(testCase.expected)
    }
  })

  it('renders nothing when no run owns the worktree', async () => {
    const { default: GatedRunBar } = await importBar()
    hookState = { run: undefined, refresh: () => {} }
    expect(renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)).toBe('')
  })

  it('keeps completed progress visible but removes the unlink mutation', async () => {
    const { default: GatedRunBar } = await importBar()
    hookState = { run: makeRun({ state: 'done' }), refresh: () => {} }
    const html = renderToStaticMarkup(<GatedRunBar worktreeId="wt-1" />)
    expect(html).toContain('Gated run complete')
    expect(html).not.toContain('Unlink issue')
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
  it('renders exactly one progress region across verifying, ready_to_test, done, and blocked', async () => {
    const { GatedRunBarView } = await importBar()
    const cases: Array<{
      featureState: FeatureState
      overrides: Partial<HarnessStatus>
      expectedHeadline: string
      expectedLabel: string
    }> = [
      {
        featureState: 'verifying',
        overrides: { state: 'verifying' },
        expectedHeadline: 'Verifying Build the thing',
        expectedLabel: 'Unit gate'
      },
      {
        featureState: 'ready_to_test',
        overrides: { state: 'running' },
        expectedHeadline: 'Browser QA for Build the thing',
        expectedLabel: 'Browser QA'
      },
      {
        featureState: 'done',
        overrides: { state: 'done', phase: 'done' },
        expectedHeadline: 'Gated run complete',
        expectedLabel: 'Done'
      },
      {
        featureState: 'blocked',
        overrides: { state: 'blocked', phase: 'blocked', blocked_phase: 'executing' },
        expectedHeadline: 'Blocked on Build the thing',
        expectedLabel: 'Blocked'
      }
    ]

    for (const testCase of cases) {
      const run = makeRun(testCase.overrides)
      run.features.features[0].state = testCase.featureState
      const html = renderToStaticMarkup(
        <GatedRunBarView
          run={run}
          issueLabel={null}
          busy={false}
          arming={false}
          expanded={true}
          restarting={false}
          onToggleExpanded={() => {}}
          onUnlink={() => {}}
          onRetry={() => {}}
        />
      )

      expect(html.match(/aria-label="Gated run progress"/g) ?? []).toHaveLength(1)
      expect(html).toContain(testCase.expectedHeadline)
      expect(html).toContain(testCase.expectedLabel)
    }
  })

  it('arms the two-tap confirm instead of unlinking on the first click', async () => {
    const { GatedRunBarView } = await importBar()
    const html = renderToStaticMarkup(
      <GatedRunBarView
        run={makeRun()}
        issueLabel="#42"
        busy={false}
        arming={true}
        expanded={true}
        restarting={false}
        onToggleExpanded={() => {}}
        onUnlink={() => {}}
        onRetry={() => {}}
      />
    )
    expect(html).toContain('Confirm unlink')
  })

  it('busy disables the unlink button', async () => {
    const { GatedRunBarView } = await importBar()
    const html = renderToStaticMarkup(
      <GatedRunBarView
        run={makeRun()}
        issueLabel="#42"
        busy={true}
        arming={false}
        expanded={true}
        restarting={false}
        onToggleExpanded={() => {}}
        onUnlink={() => {}}
        onRetry={() => {}}
      />
    )
    expect(html).toContain('disabled=""')
  })

  it('offers recovery for a failed run and keeps the stage connector behind labels', async () => {
    const { GatedRunBarView } = await importBar()
    const html = renderToStaticMarkup(
      <GatedRunBarView
        run={makeRun({ state: 'failed' })}
        issueLabel="#42"
        busy={false}
        arming={false}
        expanded={true}
        restarting={false}
        onToggleExpanded={() => {}}
        onUnlink={() => {}}
        onRetry={() => {}}
      />
    )
    expect(html).toContain('Retry run')
    expect(html).toContain('w-[calc(100%+0.25rem)]')
    expect(html).toContain('relative z-10 truncate bg-background')
  })

  it('renders concurrent workers, enforcement, patch state, context, and conflicts', async () => {
    const { GatedRunBarView } = await importBar()
    const run = makeRun({
      execution_mode: 'orchestrated',
      max_concurrency: 4,
      coordinator_session: 'coordinator-session',
      active_workers: [
        {
          task_id: 'T-ui',
          state: 'patch_pending',
          session_id: 'worker-session',
          enforcement: 'enforced',
          context_remaining: 42,
          patch_state: 'prepared'
        },
        {
          task_id: 'T-api',
          state: 'blocked',
          session_id: 'worker-2',
          enforcement: 'best_effort',
          conflict: 'external drift at src/api.rs'
        }
      ]
    })
    const html = renderToStaticMarkup(
      <GatedRunBarView
        run={run}
        issueLabel="#42"
        busy={false}
        arming={false}
        expanded={true}
        restarting={false}
        onToggleExpanded={() => {}}
        onUnlink={() => {}}
        onRetry={() => {}}
        onSelectWorker={() => {}}
      />
    )
    expect(html).toContain('2/4 active')
    expect(html).toContain('T-ui')
    expect(html).toContain('enforced')
    expect(html).toContain('patch prepared')
    expect(html).toContain('42% ctx')
    expect(html).toContain('best_effort')
    expect(html).toContain('external drift at src/api.rs')
  })
})
