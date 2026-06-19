// Goals — the desktop surface for the chat-to-features pipeline (spec 012).
// Lists board goals (`board_items` with `lbl: 'goal'`) and their planner-
// produced child cards, lets the user create a goal from natural language, and
// exposes the per-goal "Plan harness" action (spec 011) that writes the harness
// backlog. Running the harness stays a deliberate, separate step.
import { type FormEvent, useCallback, useEffect, useState } from 'react'
import { Loader2, Plus, RefreshCw, Rocket, Target } from 'lucide-react'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import { DrillInHeader } from '@/components/nav/DrillInHeader'
import {
  type GoalWithChildren,
  createGoal,
  listBoard,
  selectGoalsWithChildren
} from '@/runtime/board-client'
import { type PlanGoalHarnessResult, planGoalHarness } from '@/runtime/harness-client'

type PlanState =
  | { status: 'idle' }
  | { status: 'planning' }
  | { status: 'done'; result: PlanGoalHarnessResult }
  | { status: 'error'; message: string }

export default function GoalsPage() {
  const openHarnessPage = useAppStore((s) => s.openHarnessPage)

  const [goals, setGoals] = useState<GoalWithChildren[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  // Create-goal form.
  const [title, setTitle] = useState('')
  const [body, setBody] = useState('')
  const [workdir, setWorkdir] = useState('')
  const [creating, setCreating] = useState(false)

  // Per-goal "Plan harness" state, keyed by goal id.
  const [planById, setPlanById] = useState<Record<number, PlanState>>({})

  const refresh = useCallback(async () => {
    try {
      const board = await listBoard()
      setGoals(selectGoalsWithChildren(board))
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  // Load + poll: the planner creates child cards asynchronously after a goal is
  // created, so refresh on an interval to surface them as they land.
  useEffect(() => {
    void refresh()
    const t = setInterval(() => void refresh(), 5000)
    return () => clearInterval(t)
  }, [refresh])

  const onCreate = useCallback(
    async (e: FormEvent) => {
      e.preventDefault()
      const t = title.trim()
      if (!t) return
      setCreating(true)
      try {
        await createGoal({
          title: t,
          body: body.trim() || undefined,
          workdir: workdir.trim() || undefined
        })
        setTitle('')
        setBody('')
        // Keep workdir — the next goal is likely in the same project.
        await refresh()
      } catch (e2) {
        setError(e2 instanceof Error ? e2.message : String(e2))
      } finally {
        setCreating(false)
      }
    },
    [title, body, workdir, refresh]
  )

  const onPlan = useCallback(async (goalId: number) => {
    setPlanById((m) => ({ ...m, [goalId]: { status: 'planning' } }))
    try {
      const result = await planGoalHarness(goalId)
      setPlanById((m) => ({ ...m, [goalId]: { status: 'done', result } }))
    } catch (e) {
      setPlanById((m) => ({
        ...m,
        [goalId]: { status: 'error', message: e instanceof Error ? e.message : String(e) }
      }))
    }
  }, [])

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-background text-foreground">
      <DrillInHeader
        icon={Target}
        title="Goals"
        description="Turn a goal into planned feature cards"
        actions={
          <button
            type="button"
            onClick={() => void refresh()}
            aria-label="Refresh"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-[12px] text-foreground/60 hover:bg-foreground/8"
          >
            <RefreshCw className="size-3.5" />
            Refresh
          </button>
        }
      />

      <div className="mx-auto w-full max-w-3xl flex-1 overflow-y-auto p-4">
        {/* Create goal */}
        <form
          onSubmit={onCreate}
          className="mb-5 rounded-lg border border-border/60 bg-card/40 p-3"
        >
          <div className="mb-2 text-[13px] font-medium">Describe a goal</div>
          <input
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            placeholder="e.g. Add Google OAuth login"
            className="mb-2 w-full rounded-md border border-border/60 bg-background px-2.5 py-1.5 text-[13px] outline-none focus:border-foreground/40"
          />
          <textarea
            value={body}
            onChange={(e) => setBody(e.target.value)}
            placeholder="Details / acceptance criteria (optional)"
            rows={2}
            className="mb-2 w-full resize-y rounded-md border border-border/60 bg-background px-2.5 py-1.5 text-[13px] outline-none focus:border-foreground/40"
          />
          <input
            value={workdir}
            onChange={(e) => setWorkdir(e.target.value)}
            placeholder="Project directory (where .agentum-harness/ lives)"
            className="mb-2 w-full rounded-md border border-border/60 bg-background px-2.5 py-1.5 text-[12px] text-foreground/80 outline-none focus:border-foreground/40"
          />
          <button
            type="submit"
            disabled={creating || !title.trim()}
            className={cn(
              'flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[13px] font-medium',
              creating || !title.trim()
                ? 'cursor-not-allowed bg-foreground/10 text-foreground/40'
                : 'bg-foreground text-background hover:bg-foreground/90'
            )}
          >
            {creating ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />}
            Create goal
          </button>
          <p className="mt-1.5 text-[11px] text-foreground/40">
            The planner decomposes the goal into feature cards (they appear below
            as they land).
          </p>
        </form>

        {error ? (
          <div className="mb-4 rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12px] text-red-400">
            {error}
          </div>
        ) : null}

        {/* Goals list */}
        {loading ? (
          <div className="flex items-center gap-2 px-1 py-6 text-[13px] text-foreground/50">
            <Loader2 className="size-4 animate-spin" /> Loading goals…
          </div>
        ) : goals.length === 0 ? (
          <div className="px-1 py-6 text-[13px] text-foreground/50">
            No goals yet — describe one above to get started.
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {goals.map(({ goal, children }) => {
              const plan = planById[goal.id] ?? { status: 'idle' as const }
              return (
                <div
                  key={goal.id}
                  className="rounded-lg border border-border/60 bg-card/40 p-3"
                >
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="rounded bg-foreground/10 px-1.5 py-0.5 font-mono text-[11px] text-foreground/60">
                          {goal.key}
                        </span>
                        <span className="truncate text-[13px] font-medium">{goal.title}</span>
                      </div>
                      {goal.workdir ? (
                        <div className="mt-0.5 truncate font-mono text-[11px] text-foreground/40">
                          {goal.workdir}
                        </div>
                      ) : null}
                    </div>
                    <button
                      type="button"
                      onClick={() => void onPlan(goal.id)}
                      disabled={plan.status === 'planning'}
                      className={cn(
                        'flex shrink-0 items-center gap-1.5 rounded-md px-2.5 py-1 text-[12px] font-medium',
                        plan.status === 'planning'
                          ? 'cursor-not-allowed bg-foreground/10 text-foreground/40'
                          : 'bg-foreground/10 text-foreground hover:bg-foreground/20'
                      )}
                    >
                      {plan.status === 'planning' ? (
                        <Loader2 className="size-3.5 animate-spin" />
                      ) : (
                        <Rocket className="size-3.5" />
                      )}
                      Plan harness
                    </button>
                  </div>

                  {/* Child cards */}
                  {children.length > 0 ? (
                    <ul className="mt-2 flex flex-col gap-1">
                      {children.map((c) => (
                        <li
                          key={c.id}
                          className="flex items-center gap-2 rounded border border-border/40 bg-background/40 px-2 py-1 text-[12px]"
                        >
                          <span className="font-mono text-[10px] text-foreground/40">{c.key}</span>
                          <span className="truncate">{c.title}</span>
                          <span className="ml-auto shrink-0 text-[10px] text-foreground/40">
                            {c.status}
                          </span>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <div className="mt-2 text-[11px] text-foreground/40">
                      No feature cards yet — the planner may still be decomposing
                      this goal.
                    </div>
                  )}

                  {/* Plan result / error */}
                  {plan.status === 'done' ? (
                    <div className="mt-2 flex flex-wrap items-center gap-2 rounded-md border border-emerald-500/30 bg-emerald-500/10 px-2.5 py-1.5 text-[12px] text-emerald-300">
                      <span>
                        Wrote {plan.result.feature_count} feature
                        {plan.result.feature_count === 1 ? '' : 's'} to the harness via{' '}
                        <span className="font-medium">{plan.result.provider}</span>.
                      </span>
                      <button
                        type="button"
                        onClick={openHarnessPage}
                        className="ml-auto rounded bg-emerald-500/20 px-2 py-0.5 text-[11px] font-medium hover:bg-emerald-500/30"
                      >
                        Open Harness →
                      </button>
                    </div>
                  ) : plan.status === 'error' ? (
                    <div className="mt-2 rounded-md border border-red-500/40 bg-red-500/10 px-2.5 py-1.5 text-[12px] text-red-400">
                      {plan.message}
                    </div>
                  ) : null}
                </div>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
