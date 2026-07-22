import { renderToStaticMarkup } from 'react-dom/server'
import type { ReactNode } from 'react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { WorktreeCardDetailsHover } from './WorktreeCardMeta'

vi.mock('@/components/ui/hover-card', () => ({
  HoverCard: ({ children }: { children: ReactNode }) => <>{children}</>,
  HoverCardContent: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  HoverCardTrigger: ({ children }: { children: ReactNode }) => <>{children}</>
}))

vi.mock('@/components/ui/tooltip', () => ({
  Tooltip: ({ children }: { children: ReactNode }) => <>{children}</>,
  TooltipContent: ({ children }: { children: ReactNode }) => <>{children}</>,
  TooltipTrigger: ({ children }: { children: ReactNode }) => <>{children}</>
}))

// The Project-status chip (spec 018) reaches Tauri + the loopback server; stub
// both so the static render never touches native internals. The lazy fetch is
// effect-gated, so renderToStaticMarkup never calls these anyway.
vi.mock('@/tauri/gh', () => ({
  gh: { issueProjectStatus: vi.fn(async () => ({ ok: true, status: null })) }
}))
vi.mock('@/runtime/github-projects-client', () => ({
  getProjectBinding: vi.fn(async () => ({
    slug: 'acme/agentum',
    binding: null
  }))
}))
const projectStatusState = vi.hoisted(() => ({ status: 'TODO' as string | null, warning: null as string | null }))
vi.mock('./IssueProjectStatusChip', () => ({
  useIssueProjectStatus: ({ issueUrl }: { issueUrl?: string }) => ({
    status: issueUrl ? projectStatusState.status : null,
    warning: issueUrl ? projectStatusState.warning : null
  }),
  IssueProjectStatusChip: ({ status }: { status: string | null }) =>
    status ? <span data-project-status="true">{status}</span> : null
}))

describe('WorktreeCardDetailsHover', () => {
  beforeEach(() => {
    projectStatusState.status = 'TODO'
    projectStatusState.warning = null
  })
  it('includes branch identity before metadata details', () => {
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        branchName="feature/local-branch"
        workspaceTitle="Fix stale GH PR"
        issue={null}
        linearIssue={null}
        review={{
          provider: 'github',
          number: 456,
          title: 'Fix stale GH PR',
          state: 'open',
          url: 'https://github.com/acme/agentum/pull/456',
          status: 'success',
          updatedAt: '2026-05-17T00:00:00.000Z',
          mergeable: 'MERGEABLE'
        }}
        comment={null}
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
      >
        <span>Fix stale GH PR</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup).toContain('feature/local-branch')
    expect(markup).toContain('Fix stale GH PR')
  })

  it('accepts the status-read props and renders only the external lifecycle value', () => {
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        issue={{
          number: 365,
          title: 'Issue hover card project status',
          state: 'open',
          url: 'https://github.com/acme/agentum/issues/365',
          labels: ['area/desktop']
        }}
        linearIssue={null}
        comment={null}
        workdir="/home/dev/agentum"
        repoId="repo-1"
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
      >
        <span>hover</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup).toContain('Issue hover card project status')
    expect(markup).toContain('area/desktop')
    // The local pipeline status is never rendered as a second lifecycle chip.
    expect(markup).not.toContain('In Progress')
  })

  it.each([
    ['in_progress', 'stale local In Progress + external TODO'],
    ['todo', 'matching local/external TODO']
  ])('renders exactly one external status chip for %s (%s)', (trackerPhase) => {
    const legacyLocalProps = { trackerPhase } as unknown as Record<string, unknown>
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        issue={{
          number: 399,
          title: 'Authoritative tracker status',
          state: 'open',
          url: 'https://github.com/acme/agentum/issues/399',
          labels: []
        }}
        linearIssue={null}
        comment={null}
        worktreeId="repo-1::/worktree"
        workdir="/worktree"
        repoId="repo-1"
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
        {...legacyLocalProps}
      >
        <span>hover</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup.match(/data-project-status="true"/g)).toHaveLength(1)
    expect(markup.match(/>TODO</g)).toHaveLength(1)
    expect(markup).not.toContain('In Progress')
  })

  it('suppresses canonical tracker labels beside a resolved Project status', () => {
    projectStatusState.status = 'In progress'
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        issue={{
          number: 402,
          title: 'Single authoritative issue status',
          state: 'open',
          url: 'https://github.com/acme/agentum/issues/402',
          labels: ['status/blocked', 'status/in-progress', 'status/qa', 'area/desktop']
        }}
        linearIssue={null}
        comment={null}
        workdir="/worktree"
        repoId="repo-1"
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
      >
        <span>hover</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup.match(/data-project-status="true"/g)).toHaveLength(1)
    expect(markup.match(/>In progress</g)).toHaveLength(1)
    expect(markup).not.toContain('status/blocked')
    expect(markup).not.toContain('status/in-progress')
    expect(markup).toContain('status/qa')
    expect(markup).toContain('area/desktop')
  })

  it('preserves canonical tracker labels when no Project status resolves', () => {
    projectStatusState.status = null
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        issue={{
          number: 402,
          title: 'Label-only issue status fallback',
          state: 'open',
          url: 'https://github.com/acme/agentum/issues/402',
          labels: ['status/blocked', 'status/in-progress', 'status/qa', 'area/desktop']
        }}
        linearIssue={null}
        comment={null}
        workdir="/worktree"
        repoId="repo-1"
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
      >
        <span>hover</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup).not.toContain('data-project-status="true"')
    expect(markup).toContain('status/blocked')
    expect(markup).toContain('status/in-progress')
    expect(markup).toContain('status/qa')
    expect(markup).toContain('area/desktop')
  })

  it('surfaces an actionable warning while a GitHub transition remains pending', () => {
    projectStatusState.warning =
      'GitHub status sync pending: project scope missing. Check gh authentication and the Project binding; Agentum will retry.'
    const markup = renderToStaticMarkup(
      <WorktreeCardDetailsHover
        issue={{
          number: 399,
          title: 'Authoritative tracker status',
          state: 'open',
          url: 'https://github.com/acme/agentum/issues/399',
          labels: []
        }}
        linearIssue={null}
        comment={null}
        workdir="/worktree"
        repoId="repo-1"
        onEditIssue={vi.fn()}
        onEditComment={vi.fn()}
      >
        <span>hover</span>
      </WorktreeCardDetailsHover>
    )

    expect(markup).toContain('GitHub status sync pending')
    expect(markup).toContain('Check gh authentication')
    expect(markup).toContain('Agentum will retry')
  })
})
