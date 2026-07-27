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

  it('keeps the durable launch stages in a visible side rail with cancel', () => {
    expect(source).toContain('<NewWorkProgressRail')
    expect(source).toContain('aria-label="Workspace creation progress"')
    expect(source).toContain('NEW_WORK_STAGES.map((stage, index)')
    expect(source).toContain('These stages stay visible while your workspace is prepared.')
    expect(source).toContain('disabled={busy}')
    expect(source).toContain('Cancel')
    expect(source).not.toContain('Object.values(progress).some')
  })

  it('moves the dialog from its title row without allowing it off-screen', () => {
    expect(source).toContain('data-dialog-drag-handle')
    expect(source).toContain('onPointerDown={handleDialogDragStart}')
    expect(source).toContain('onPointerMove={handleDialogDragMove}')
    expect(source).toContain('style={{ translate: `${dialogOffset.x}px ${dialogOffset.y}px` }}')
    expect(source).toContain('clampDialogOffset({')
  })
})
