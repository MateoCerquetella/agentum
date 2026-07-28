import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('CreateWorkspaceWizard hard cutover', () => {
  const source = readFileSync(new URL('./CreateWorkspaceWizard.tsx', import.meta.url), 'utf8')

  it('keeps manual workspace creation tracker-neutral', () => {
    const heading = source.indexOf('Name the workspace — and choose its agent')
    const worktreeName = source.indexOf("{selectedRepoIsGit ? 'Worktree name'", heading)

    expect(heading).toBeGreaterThan(-1)
    expect(worktreeName).toBeGreaterThan(heading)
    expect(source).toContain('initialLinkedWorkItem: null')
    expect(source).toContain('enableIssueAutomation: false')
    expect(source).toContain("const workSource: WorkSource = 'none'")
    expect(source).not.toContain('CanonicalTrackerSection')
    expect(source).not.toContain('onCreateIssueSubmit')
  })

  it('points tracker-originated work to New Spec without exposing a fallback', () => {
    expect(source).toContain('Use New Spec for GitHub, Linear, Jira, or imported work.')
    expect(source).not.toContain('Search repository issues')
    expect(source).not.toContain('Create issue')
    expect(source).not.toContain('Start workspace from')
  })

  it('uses responsive execution controls and a staged mobile footer', () => {
    expect(source).toContain('grid grid-cols-1 gap-2 sm:grid-cols-2')
    expect(source).toContain('flex-col-reverse')
    expect(source).toContain('sm:flex-row sm:items-center')
    expect(source).toContain('aria-busy={step === 3 && launchBusy}')
    expect(source).toContain("newWorkBusyLabel(launchProgress)")
    expect(source).toContain('w-full items-center justify-center')
  })

  it('keeps setup-policy ask repos actionable', () => {
    expect(source).toContain('onSetupDecisionChange={onSetupDecisionChange}')
    expect(source).toContain("decision === 'run' ? 'Run setup' : 'Skip setup'")
  })

  it('switches an added project before hydration and blocks navigation while it is pending', () => {
    expect(source).toContain('selectionPending: addingRepo || remoteAddOpen')

    const addFlow = source.indexOf('await selectAddedRepoBeforeHydration({')
    const selectedRepo = source.indexOf('selectRepo: onRepoChange', addFlow)
    const hydration = source.indexOf('hydrateRepo: fetchWorktrees', addFlow)

    expect(addFlow).toBeGreaterThan(-1)
    expect(selectedRepo).toBeGreaterThan(addFlow)
    expect(hydration).toBeGreaterThan(selectedRepo)
  })

  it('locks the staged launch owner and all mutable step-three controls while busy', () => {
    expect(source).toContain('<Dialog open onOpenChange={handleDialogOpenChange}>')
    expect(source).toContain('if (!open && launchBusy) return')
    expect(source).toContain('<StepDots step={step} locked={launchScopeLocked}')
    expect(source).toContain('Boolean(launchCheckpoint.worktreeResult)')
    expect(source).toMatch(
      /disabled=\{launchScopeLocked\}\s+className="m-0 min-w-0 border-0 p-0/
    )
    expect(source).toContain('!launchCheckpoint.worktreeResult ? (')
    expect(source).toContain('disabled={!done || locked}')
  })

  it('keeps the durable launch stages in a visible bottom panel with cancel', () => {
    const body = source.indexOf('{/* Body */}')
    const progress = source.indexOf('<NewWorkProgressPanel')
    const footer = source.indexOf('{/* Footer */}')

    expect(progress).toBeGreaterThan(body)
    expect(progress).toBeLessThan(footer)
    expect(source).toContain('aria-label="Workspace creation progress"')
    expect(source).toContain('NEW_WORK_STAGES.map((stage, index)')
    expect(source).toContain('These stages stay visible while your workspace is prepared.')
    expect(source).toContain('disabled={busy}')
    expect(source).toContain('Cancel')
    expect(source).toContain('grid grid-cols-2 gap-2 sm:grid-cols-4')
    expect(source).not.toContain('NewWorkProgressRail')
    expect(source).not.toContain('md:w-[238px]')
    expect(source).not.toContain('md:border-l')
    expect(source).not.toContain('Object.values(progress).some')
  })

  it('uses the top title bar as the drag surface without a dedicated grip icon', () => {
    const dragHandle = source.indexOf('data-dialog-drag-handle')
    const wizardNavigation = source.indexOf('{/* Wizard navigation */}')

    expect(dragHandle).toBeGreaterThan(-1)
    expect(dragHandle).toBeLessThan(wizardNavigation)
    expect(source).toContain('onPointerDown={handleDialogDragStart}')
    expect(source).toContain('onPointerMove={handleDialogDragMove}')
    expect(source).toContain('left: `calc(50% + ${dialogOffset.x}px)`')
    expect(source).toContain('top: `calc(50% + ${dialogOffset.y}px)`')
    expect(source).not.toContain('GripHorizontal')
    expect(source).toContain('clampDialogOffset({')
  })
})
