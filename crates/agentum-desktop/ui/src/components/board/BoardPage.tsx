import { Columns3, GitBranch, Sparkles } from 'lucide-react'
import { DrillInHeader } from '@/components/nav/DrillInHeader'

/**
 * Board (Kanban) — Phase 2 placeholder (#48).
 *
 * The desktop Kanban view will let cards (goals/tickets) flow
 * Backlog → Building → Review → Done, and starting a card will create a worktree
 * and spawn an agent to build it. The backend primitives already exist
 * (`/api/board`, `/api/worktrees/create`); this is the labeled landing spot for
 * the rail item until Phase 2 wires the UI, so the nav shell is complete and the
 * concept is explained rather than missing.
 */
export default function BoardPage() {
  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
      <DrillInHeader
        icon={Columns3}
        title="Board"
        description="Your Kanban of agent tickets — Backlog → Building → Review → Done"
      />
      <div className="flex flex-1 items-center justify-center p-8">
        <div className="max-w-md text-center">
          <div className="mx-auto mb-4 flex size-12 items-center justify-center rounded-xl border border-border/60 bg-card/50">
            <Columns3 className="size-6 text-muted-foreground" />
          </div>
          <h2 className="text-base font-semibold tracking-tight">Board is coming soon</h2>
          <p className="mt-2 text-sm text-muted-foreground">
            A Kanban of your agent tickets. Each card is a goal — move it from
            Backlog → Building → Review → Done. Starting a card will create a
            worktree and launch an agent to build it.
          </p>
          <div className="mt-5 flex flex-wrap items-center justify-center gap-2 text-[12px] text-muted-foreground/80">
            <span className="inline-flex items-center gap-1 rounded-full border border-border/60 bg-muted/30 px-2.5 py-1">
              <Sparkles className="size-3.5" /> Phase 2
            </span>
            <span className="inline-flex items-center gap-1 rounded-full border border-border/60 bg-muted/30 px-2.5 py-1">
              <GitBranch className="size-3.5" /> card → worktree → agent
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}
