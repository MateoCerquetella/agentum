import { renderToStaticMarkup } from 'react-dom/server'
import { describe, expect, it, vi } from 'vitest'
import NewWorkspaceAutomationPanel, {
  type NewWorkspaceAutomationPanelProps
} from './NewWorkspaceAutomationPanel'

function renderPanel(overrides: Partial<NewWorkspaceAutomationPanelProps> = {}): string {
  const props: NewWorkspaceAutomationPanelProps = {
    primaryActionLabel: 'Create Worktree',
    selectedRepoIsGit: true,
    selectedSource: null,
    canCreateGithubIssue: true,
    createIssueOpen: false,
    onCreateIssueOpenChange: vi.fn(),
    createIssueTitle: '',
    onCreateIssueTitleChange: vi.fn(),
    createIssueBody: '',
    onCreateIssueBodyChange: vi.fn(),
    createIssueSubmitting: false,
    createIssueError: null,
    onCreateIssueSubmit: vi.fn(),
    createIssueGenerating: false,
    onGenerateIssueBody: vi.fn(),
    createIssueLabels: [],
    createIssueLabelOptions: null,
    onToggleCreateIssueLabel: vi.fn(),
    canScaffoldSpec: false,
    scaffoldSpec: false,
    onScaffoldSpecChange: vi.fn(),
    canStartGatedRun: false,
    startGatedRun: false,
    onStartGatedRunChange: vi.fn(),
    sddRolesEnabled: true,
    ...overrides
  }

  return renderToStaticMarkup(<NewWorkspaceAutomationPanel {...props} />)
}

describe('NewWorkspaceAutomationPanel', () => {
  it('keeps the full issue-to-run flow visible beside worktree creation', () => {
    const markup = renderPanel()

    expect(markup).toContain('aria-label="Issue, worktree, spec, and run options"')
    expect(markup).toContain('Issue → Worktree → Spec → Run')
    expect(markup.indexOf('>Issue<')).toBeLessThan(markup.indexOf('>Worktree<'))
    expect(markup.indexOf('>Worktree<')).toBeLessThan(markup.indexOf('>Spec<'))
    expect(markup.indexOf('>Spec<')).toBeLessThan(markup.indexOf('>Run<'))
    expect(markup).toContain('Cancel below leaves without creating anything.')
  })

  it('shows unavailable optional steps instead of hiding them', () => {
    const markup = renderPanel({ canCreateGithubIssue: false })

    expect(markup).toContain('aria-label="Scaffold spec from issue"')
    expect(markup).toContain('aria-label="Start gated run"')
    expect(markup).toContain('Link a supported GitHub issue to enable it.')
    expect(markup.match(/disabled=""/g)).toHaveLength(2)
  })

  it('shows that a gated run includes the spec step', () => {
    const markup = renderPanel({
      selectedSource: { kind: 'github-issue', label: '#42 Fix sidebar flow' },
      canCreateGithubIssue: false,
      canScaffoldSpec: true,
      canStartGatedRun: true,
      startGatedRun: true
    })

    expect(markup).toContain('#42 Fix sidebar flow')
    expect(markup).toContain('Included in the gated run.')
    expect(markup).toContain('Start the PM → Architect → Build → Review gated role loop.')
  })
})
