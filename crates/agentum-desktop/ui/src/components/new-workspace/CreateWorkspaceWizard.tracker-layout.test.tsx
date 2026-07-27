import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

describe('CreateWorkspaceWizard canonical tracker layout', () => {
  const source = readFileSync(new URL('./CreateWorkspaceWizard.tsx', import.meta.url), 'utf8')

  it('renders one tracker control immediately before downstream work fields', () => {
    const heading = source.indexOf("What&apos;s the work — and who drives it?")
    const tracker = source.indexOf('<CanonicalTrackerSection', heading)
    const worktreeName = source.indexOf("{selectedRepoIsGit ? 'Worktree name'", heading)

    expect(heading).toBeGreaterThan(-1)
    expect(tracker).toBeGreaterThan(heading)
    expect(tracker).toBeLessThan(worktreeName)
    expect(source.match(/<CanonicalTrackerSection/g)).toHaveLength(1)
    expect(source).not.toContain('function TrackerSection(')
    expect(source).not.toContain('function CreateIssuePanel(')
    expect(source).not.toContain('resolveCreateIssueProvider')
  })

  it('keeps provider choice canonical and avoids a duplicate repository picker', () => {
    expect(source).toContain('projectTrackerConfigByRepo[repoId]')
    expect(source.match(/aria-label="Search repository issues"/g)).toHaveLength(1)
    expect(source).toContain("provider === 'github'")
    expect(source).toContain("provider === 'linear'")
    expect(source).toContain('No issue')
    expect(source).toContain('aria-label="Refresh repository issues"')
    expect(source).toContain("repoIssuePicker.error ?? 'No matching open repository issues.'")
  })

  it('uses responsive execution controls and a staged mobile footer', () => {
    expect(source).toContain('grid grid-cols-1 gap-2 sm:grid-cols-2')
    expect(source).toContain('flex-col-reverse')
    expect(source).toContain('sm:flex-row sm:items-center')
    expect(source).toContain('aria-busy={step === 3 && launchBusy}')
    expect(source).toContain("newWorkBusyLabel(launchProgress)")
    expect(source).toContain('w-full items-center justify-center')
  })

  it('keeps setup-policy ask repos actionable and closes before opening Settings', () => {
    expect(source).toContain('onSetupDecisionChange={onSetupDecisionChange}')
    expect(source).toContain("decision === 'run' ? 'Run setup' : 'Skip setup'")

    const openSettings = source.indexOf('const openTrackerSettings')
    const closeModal = source.indexOf('store.closeModal()', openSettings)
    expect(openSettings).toBeGreaterThan(-1)
    expect(closeModal).toBeGreaterThan(openSettings)
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
