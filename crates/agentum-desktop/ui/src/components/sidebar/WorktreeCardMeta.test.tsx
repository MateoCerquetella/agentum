import { renderToStaticMarkup } from 'react-dom/server'
import type { ReactNode } from 'react'
import { describe, expect, it, vi } from 'vitest'
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
vi.mock('@/tauri/gh', () => ({ gh: { issueProjectStatus: vi.fn(async () => ({ ok: true, status: null })) } }))
vi.mock('@/runtime/github-projects-client', () => ({
  getProjectBinding: vi.fn(async () => ({ slug: 'acme/agentum', binding: null }))
}))

describe('WorktreeCardDetailsHover', () => {
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
    expect(markup.indexOf('feature/local-branch')).toBeLessThan(markup.indexOf('PR #456'))
    expect(markup).toContain('Fix stale GH PR')
  })

  it('accepts the spec-018 workdir/repoId props and renders the issue without a status chip synchronously', () => {
    // The Project-status read is effect-gated (fetch-on-open), so a static
    // render carries the issue but no chip — and must not throw with the new
    // props threaded through (spec 018 #365).
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
    // No status option name renders without the effect firing (silent absence).
    expect(markup).not.toContain('In Progress')
  })
})
