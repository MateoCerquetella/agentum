// Chat — the repo-grounded SDD intake surface (formerly "Harness Engine").
// Per the dclaude "Pipeline Demo" design: a feature begins as a conversation —
// you describe it, the agent drafts a spec into an ordered backlog of cards,
// you Approve, and the harness drives the cards behind the verify gate. Left:
// the "Chats" history. Right: the conversation + the spec/cards panel + composer.
//
// Wired to the real harness backend (crate `agentum-server` /api/harness/* via
// runtime/harness-client). The conversational Socratic draft (4-choices intake)
// is the next slice; this surface already lists runs as chats, renders each
// run's backlog as cards, streams live state, and Approves (runs) a backlog.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Clock, MessagesSquare, Plus, Send, Sparkles } from 'lucide-react'
import { DrillInHeader } from '@/components/nav/DrillInHeader'

import { useAppStore } from '@/store'
import { cn } from '@/lib/utils'
import {
  type Feature,
  type FeatureState,
  type HarnessEvent,
  type HarnessStatus,
  getHarnessStatus,
  listHarnesses,
  openHarnessEventStream,
  runHarness,
  scaffoldHarness,
  startHarness
} from '@/runtime/harness-client'

// ---- state → colour, matching the design's amber/green/red/grey language ----
function stateColor(s: FeatureState | HarnessStatus['state']): { dot: string; text: string } {
  switch (s) {
    case 'coding':
    case 'running':
    case 'init_verifying':
      return { dot: 'bg-amber-500', text: 'text-amber-500' }
    case 'verifying':
      return { dot: 'bg-sky-400', text: 'text-sky-400' }
    case 'done':
      return { dot: 'bg-emerald-500', text: 'text-emerald-500' }
    case 'blocked':
    case 'failed':
      return { dot: 'bg-red-500', text: 'text-red-500' }
    default:
      return { dot: 'bg-muted-foreground/40', text: 'text-muted-foreground' }
  }
}

function dirName(path: string): string {
  const parts = path.replace(/\/+$/, '').split('/')
  return parts[parts.length - 1] || path
}

function chatTitle(run: HarnessStatus): string {
  return dirName(run.workdir)
}

function runSubtitle(run: HarnessStatus): string {
  const feats = run.features?.features ?? []
  const done = feats.filter((f) => f.state === 'done').length
  if (run.state === 'done') return `${feats.length} cards · done`
  if (run.state === 'running' || run.state === 'verifying')
    return `${done}/${feats.length} cards · in progress`
  if (run.state === 'blocked') return `blocked · ${done}/${feats.length}`
  return feats.length ? `spec ready · ${feats.length} cards` : 'new'
}

export default function ChatPage() {
  const repos = useAppStore((s) => s.repos)

  const [runs, setRuns] = useState<HarnessStatus[]>([])
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [draft, setDraft] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const streamRef = useRef<{ close: () => void } | null>(null)

  // Feature intake (describe → draft spec) isn't finished yet, so the composer
  // is a disabled "SOON" chip. Easter egg: 5 clicks on the chip unlocks it for
  // testing. Keeping it locked also avoids the `no .agentum-harness/` 400 from
  // firing startHarness against a repo that hasn't been scaffolded.
  const [unlocked, setUnlocked] = useState(false)
  const [, setSoonClicks] = useState(0)
  const bumpSoon = useCallback(() => {
    setSoonClicks((n) => {
      const next = n + 1
      if (next >= 5) setUnlocked(true)
      return next
    })
  }, [])

  const refresh = useCallback(async () => {
    try {
      const list = await listHarnesses()
      setRuns(list)
      setSelectedId((cur) => cur ?? (list.length ? list[0].id : null))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  // Live updates for the selected run.
  useEffect(() => {
    streamRef.current?.close()
    if (!selectedId) return
    let alive = true
    const apply = (patch: (r: HarnessStatus) => HarnessStatus) =>
      setRuns((rs) => rs.map((r) => (r.id === selectedId ? patch(r) : r)))

    void getHarnessStatus(selectedId)
      .then((st) => {
        if (alive) apply(() => st)
      })
      .catch(() => {})

    void openHarnessEventStream(selectedId, (ev: HarnessEvent) => {
      if (!alive) return
      if (ev.type === 'state_changed') apply((r) => ({ ...r, state: ev.state }))
      else if (ev.type === 'feature_state_changed')
        apply((r) => ({
          ...r,
          features: {
            ...r.features,
            features: (r.features?.features ?? []).map((f) =>
              f.id === ev.feature_id ? { ...f, state: ev.state } : f
            )
          }
        }))
      else if (ev.type === 'harness_completed') void getHarnessStatus(selectedId).then((st) => alive && apply(() => st))
    }).then((s) => {
      streamRef.current = s
    })

    return () => {
      alive = false
      streamRef.current?.close()
    }
  }, [selectedId])

  const selected = useMemo(() => runs.find((r) => r.id === selectedId) ?? null, [runs, selectedId])
  const cards: Feature[] = selected?.features?.features ?? []
  const awaitingApproval = !!selected && selected.state === 'idle' && cards.length > 0

  const approve = useCallback(async () => {
    if (!selected) return
    setBusy(true)
    setError(null)
    try {
      await runHarness(selected.id)
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [selected, refresh])

  // Composer: describe a feature → register a harness run from a repo that has
  // an `.agentum-harness/`. (Full Socratic draft via agentum_harness_plan is next.)
  const submit = useCallback(async () => {
    const text = draft.trim()
    if (!text) return
    const workdir = repos[0]?.path
    if (!workdir) {
      setError('Open a repo first — Chat drives an `.agentum-harness/` backlog in your project.')
      return
    }
    setBusy(true)
    setError(null)
    try {
      // Scaffold the surface first (idempotent) so a repo without an
      // `.agentum-harness/` doesn't 400 — then register the run.
      await scaffoldHarness(workdir)
      const { harness_id } = await startHarness(workdir)
      setDraft('')
      await refresh()
      setSelectedId(harness_id)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [draft, repos, refresh])

  return (
    <div className="flex h-full min-h-0 flex-col bg-background">
      <DrillInHeader
        icon={MessagesSquare}
        title="Chat"
        description="Describe what you want in plain words — SDD intake, repo-grounded"
      />

      <div className="flex min-h-0 flex-1">
        {/* ---- Chats history ---- */}
        <aside className="flex w-56 flex-none flex-col border-r border-border bg-sidebar/60">
          <div className="p-3">
            <button
              type="button"
              onClick={() => {
                setSelectedId(null)
                setError(null)
              }}
              className="flex w-full items-center gap-2 rounded-md border border-border bg-card px-3 py-2 text-[13px] font-medium hover:border-foreground/30 hover:bg-accent"
            >
              <Plus className="size-3.5" /> New feature
            </button>
          </div>
          <div className="px-3.5 pb-1.5 font-mono text-[10px] uppercase tracking-wider text-muted-foreground">
            Chats
          </div>
          <div className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-3">
            {runs.length === 0 ? (
              <div className="px-3 py-2 text-[12px] text-muted-foreground">No chats yet.</div>
            ) : (
              runs.map((r) => {
                const c = stateColor(r.state)
                const active = r.id === selectedId
                return (
                  <button
                    key={r.id}
                    type="button"
                    onClick={() => setSelectedId(r.id)}
                    className={cn(
                      'flex w-full flex-col gap-1 rounded-md px-2.5 py-2 text-left',
                      active ? 'bg-accent' : 'hover:bg-foreground/5'
                    )}
                  >
                    <div className="flex items-center gap-1.5">
                      <span className={cn('size-1.5 flex-none rounded-full', c.dot)} />
                      <span className="flex-1 truncate text-[13px]">{chatTitle(r)}</span>
                    </div>
                    <span className="truncate pl-3 font-mono text-[10.5px] text-muted-foreground">
                      {runSubtitle(r)}
                    </span>
                  </button>
                )
              })
            )}
          </div>
        </aside>

        {/* ---- chat column ---- */}
        <div className="flex min-h-0 flex-1 flex-col">
          {/* sub-header */}
          <div className="flex h-11 flex-none items-center gap-2.5 border-b border-border px-5">
            <Sparkles className="size-4 text-primary" />
            <span className="text-[13.5px] font-medium">
              {selected ? chatTitle(selected) : 'New feature'}
            </span>
            <span className="font-mono text-[11px] text-muted-foreground">
              agentum · {selected ? dirName(selected.workdir) : 'main'} · repo-grounded
            </span>
            {selected ? (
              <span
                className={cn(
                  'ml-auto rounded-full border border-border bg-card px-2.5 py-0.5 font-mono text-[11px]',
                  stateColor(selected.state).text
                )}
              >
                {selected.state}
              </span>
            ) : null}
          </div>

          {/* thread */}
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-6">
            <div className="mx-auto flex max-w-[720px] flex-col gap-4">
              {!selected ? (
                <div className="rounded-lg border border-dashed border-border p-8 text-center text-muted-foreground">
                  <MessagesSquare className="mx-auto mb-3 size-6 opacity-60" />
                  <div className="text-sm">Describe a feature to begin.</div>
                  <div className="mt-1 font-mono text-[11px]">
                    The agent reads the repo, drafts a spec, and decomposes it into cards.
                  </div>
                </div>
              ) : (
                <>
                  {/* you */}
                  <Message who="you" isUser text={`Build the backlog for ${dirName(selected.workdir)}.`} />
                  {/* agent */}
                  <Message
                    who="agentum · spec"
                    text={
                      cards.length
                        ? `Read the repo and decomposed the spec into ${cards.length} ordered cards. Review the plan and approve to run them behind the verify gate.`
                        : 'No backlog yet — add features to `.agentum-harness/feature_list.json` or describe the feature below.'
                    }
                  />

                  {/* spec ready · N cards */}
                  {cards.length > 0 ? (
                    <div className="rounded-lg border border-border bg-card/60 p-4">
                      <div className="mb-3 flex items-center gap-2.5">
                        <span className="font-mono text-[10px] uppercase tracking-wider text-amber-500">
                          {selected.state === 'idle'
                            ? `spec ready · ${cards.length} cards`
                            : `${cards.length} cards · ${selected.state}`}
                        </span>
                        <span className="font-mono text-[11px] text-muted-foreground">
                          .agentum-harness/feature_list.json
                        </span>
                      </div>
                      <div className="flex flex-col gap-1.5">
                        {cards.map((f, i) => {
                          const c = stateColor(f.state)
                          return (
                            <div
                              key={f.id}
                              className="grid grid-cols-[26px_1fr_auto] items-center gap-2.5 rounded-md border border-border bg-background px-3 py-2 text-[13px]"
                            >
                              <span className="font-mono text-[11px] text-muted-foreground">
                                {String(i + 1).padStart(2, '0')}
                              </span>
                              <span className="truncate">{f.name}</span>
                              <span className={cn('inline-flex items-center gap-1.5 font-mono text-[10px]', c.text)}>
                                <span className={cn('size-1.5 rounded-full', c.dot)} />
                                {f.state}
                              </span>
                            </div>
                          )
                        })}
                      </div>
                      {awaitingApproval ? (
                        <div className="mt-3.5 flex flex-wrap items-center gap-3">
                          <button
                            type="button"
                            onClick={approve}
                            disabled={busy}
                            className="inline-flex h-9 items-center gap-2 rounded-full bg-primary px-4.5 text-[13.5px] font-medium text-primary-foreground hover:opacity-85 disabled:opacity-50"
                          >
                            {busy ? 'Starting…' : `Approve — run ${cards.length} cards`}
                          </button>
                          <span className="font-mono text-[11px] text-muted-foreground">
                            verify gate · one human confirm at QA
                          </span>
                        </div>
                      ) : null}
                    </div>
                  ) : null}
                </>
              )}

              {error ? (
                <div className="rounded-md border border-red-500/40 bg-red-500/10 px-3 py-2 text-[12px] text-red-400">
                  {error}
                </div>
              ) : null}
            </div>
          </div>

          {/* composer */}
          <div className="flex-none border-t border-border px-5 pb-4.5 pt-3">
            <div
              className={cn(
                'mx-auto flex max-w-[720px] items-center gap-2.5 rounded-lg border border-border bg-card px-3 py-2.5',
                !unlocked && 'opacity-80'
              )}
            >
              <input
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                disabled={!unlocked}
                onKeyDown={(e) => {
                  if (unlocked && e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault()
                    void submit()
                  }
                }}
                placeholder={
                  unlocked
                    ? 'Try "Add a CSV export to the board"…'
                    : 'Feature intake — drafts a spec from a description'
                }
                className="flex-1 bg-transparent text-[14px] text-foreground placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
              />
              {unlocked ? (
                <button
                  type="button"
                  onClick={() => void submit()}
                  disabled={busy || !draft.trim()}
                  className="inline-flex size-8 items-center justify-center rounded-md bg-primary text-primary-foreground hover:opacity-85 disabled:opacity-40"
                  aria-label="Send"
                >
                  <Send className="size-4" />
                </button>
              ) : (
                <button
                  type="button"
                  onClick={bumpSoon}
                  title="Coming soon"
                  className="inline-flex select-none items-center gap-1.5 rounded-full border border-border bg-muted/40 px-2.5 py-1 font-mono text-[10px] uppercase tracking-wider text-muted-foreground hover:bg-muted/60"
                >
                  <Clock className="size-3.5" /> Soon
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function Message({ who, text, isUser }: { who: string; text: string; isUser?: boolean }) {
  return (
    <div className={cn('flex items-start gap-3', isUser && 'flex-row-reverse')}>
      <div
        className={cn(
          'grid size-7 flex-none place-items-center rounded-full border',
          isUser ? 'border-border bg-card' : 'border-primary/40 bg-primary/10'
        )}
      >
        {isUser ? (
          <span className="font-mono text-[10px]">you</span>
        ) : (
          <Sparkles className="size-3.5 text-primary" />
        )}
      </div>
      <div className={cn('flex min-w-0 max-w-[80%] flex-col', isUser && 'items-end')}>
        <div className="mb-1.5 font-mono text-[10px] uppercase tracking-wide text-muted-foreground">{who}</div>
        <div className="rounded-lg border border-border bg-card px-4 py-3 text-[14px] leading-relaxed">{text}</div>
      </div>
    </div>
  )
}
