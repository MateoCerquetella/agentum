// Spec 015 F3: the Tracker tab's "New issue" intake — written intent → drafted
// issue → filed (GitHub/Linear) → optional gated run. A SIBLING of the
// untouched ProjectBindingEditor: the tab keeps its config half, this is the
// doing half. All behavior lives in `useTrackerIntake`; this file only renders
// its state (the CreateIssuePanel look, adapted to the hub's card style).
import React, { useState } from 'react'
import { CheckCircle2, ChevronDown, ExternalLink, Loader2, Play, Sparkles, WandSparkles } from 'lucide-react'

import { api } from '@/tauri'
import type { Repo } from '@/shared/types'
import type { ProjectTaskScope } from '@/lib/project-task-scope'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { IssueSpecInterviewDialog } from './IssueSpecInterviewDialog'
import { useTrackerIntake } from './use-tracker-intake'

export function TrackerIntakePanel({
  repo,
  scope
}: {
  repo: Repo
  scope: Extract<ProjectTaskScope, { status: 'bound' }>
}): React.JSX.Element {
  const intake = useTrackerIntake({ repo, scope })
  const [specOpen, setSpecOpen] = useState(false)
  const [specResetVersion, setSpecResetVersion] = useState(0)
  const [specSeed, setSpecSeed] = useState('')
  const hasDraft = intake.body.trim().length > 0 || intake.title.trim().length > 0
  const filedUrl = intake.filed?.url ?? null

  return (
    <div className="rounded-lg border border-border bg-card p-4">
      <h2 className="text-[14px] font-semibold tracking-tight text-foreground">New issue</h2>
      <p className="mt-0.5 text-[12px] text-muted-foreground">
        Write down what you want to do — it drafts a reviewable issue, files it with the resolved
        tracker, and can kick off a gated run from there.
      </p>

      <div className="mt-3 flex flex-col gap-2.5">
        {/* #379: always SAY where a filed issue lands + the Linear state —
            "no Linear sync showing" was the complaint. When only one provider
            resolves there is no toggle, so this line is the only signal. */}
        <div className="text-[11px] text-muted-foreground">
          Files into{' '}
          <span className="font-medium capitalize text-foreground">{intake.effectiveProvider}</span>
          <> · locked by Project Settings</>
          {' · '}
          {intake.linearConnected ? (
            <span className="text-emerald-600 dark:text-emerald-400">Linear connected</span>
          ) : (
            <>Linear not connected — add the API key in Settings → Integrations</>
          )}
        </div>

        <label className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-foreground">What do you want to do?</span>
          <textarea
            value={intake.intent}
            onChange={(event) => intake.setIntent(event.target.value)}
            rows={2}
            placeholder="Describe the work — a concise title and description get drafted from it."
            className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
          />
        </label>

        <div className="flex self-start">
          <button
            type="button"
            onClick={() => {
              setSpecOpen(false)
              setSpecResetVersion((value) => value + 1)
              intake.draft()
            }}
            disabled={!intake.canDraft}
            className="inline-flex items-center gap-1.5 rounded-l-md border border-border px-2.5 py-1.5 text-[11.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {intake.phase === 'drafting' ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <Sparkles className="size-3.5" />
            )}
            {intake.phase === 'drafting' ? 'Drafting…' : hasDraft ? 'Redraft from intent' : 'Draft issue'}
          </button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                disabled={!intake.canDraft}
                aria-label="More drafting options"
                className="-ml-px inline-flex w-7 items-center justify-center rounded-r-md border border-border text-foreground transition-colors hover:border-muted-foreground/40 hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50"
              >
                <ChevronDown className="size-3.5" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-56">
              <DropdownMenuItem
                onSelect={() => {
                  setSpecOpen(false)
                  setSpecResetVersion((value) => value + 1)
                  intake.draft()
                }}
              >
                <Sparkles className="size-3.5" />
                <span className="flex flex-col">
                  <span>Draft issue</span>
                  <span className="text-[10.5px] font-normal text-muted-foreground">A concise issue from this intent</span>
                </span>
              </DropdownMenuItem>
              <DropdownMenuItem
                onSelect={() => {
                  const seed = intake.intent.trim()
                  if (seed !== specSeed) {
                    setSpecSeed(seed)
                    setSpecResetVersion((value) => value + 1)
                  }
                  setSpecOpen(true)
                }}
              >
                <WandSparkles className="size-3.5" />
                <span className="flex flex-col">
                  <span>Shape into spec…</span>
                  <span className="text-[10.5px] font-normal text-muted-foreground">Clarify scope and done criteria</span>
                </span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {hasDraft ? (
          <>
            <label className="flex flex-col gap-1.5">
              <span className="text-[11px] text-muted-foreground">Title</span>
              <input
                value={intake.title}
                onChange={(event) => intake.setTitle(event.target.value)}
                placeholder="Issue title"
                className="h-[34px] rounded-md border border-input bg-secondary px-2.5 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
              />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[11px] text-muted-foreground">Description</span>
              <textarea
                value={intake.body}
                onChange={(event) => intake.setBody(event.target.value)}
                rows={6}
                placeholder="Drafted description — review and edit before filing."
                className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 font-mono text-[11.5px] leading-relaxed text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
              />
            </label>

            {/* Spec 020 AC 9: honesty, not failure — muted, never destructive. */}
            {intake.groundingNote ? (
              <span className="text-[11px] text-muted-foreground">{intake.groundingNote}</span>
            ) : null}

            {/* Pick the Linear team when filing into Linear and >1 exists. */}
            {intake.effectiveProvider === 'linear' && intake.teams.length > 1 ? (
              <label className="flex flex-col gap-1.5">
                <span className="text-[11px] text-muted-foreground">Linear team</span>
                <select
                  value={intake.teamId ?? ''}
                  onChange={(event) => intake.setTeamId(event.target.value || null)}
                  className="h-[34px] rounded-md border border-input bg-secondary px-2 text-[12.5px] text-foreground outline-none focus-visible:border-ring"
                >
                  <option value="">Select a team…</option>
                  {intake.teams.map((team) => (
                    <option key={team.id} value={team.id}>
                      {team.name} ({team.key})
                    </option>
                  ))}
                </select>
              </label>
            ) : null}

            <button
              type="button"
              onClick={intake.file}
              disabled={!intake.canFile}
              className="inline-flex items-center gap-1.5 self-start rounded-full bg-primary px-3.5 py-1.5 text-[12px] font-medium text-primary-foreground transition-opacity hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-50"
            >
              {intake.phase === 'filing' ? <Loader2 className="size-3.5 animate-spin" /> : null}
              {intake.phase === 'filing'
                ? 'Creating…'
                : intake.effectiveProvider === 'linear'
                  ? 'Create Linear issue'
                  : 'Create issue'}
            </button>
          </>
        ) : null}

        {/* Inline + non-fatal (AC 12): the form stays usable for a retry. */}
        {intake.error ? (
          <span className="text-[11px] text-destructive">{intake.error}</span>
        ) : null}

        {/* Provider-confirmed filed issue (AC 11): the link, and — for a local
            GitHub issue — the pre-armed composer hop into a gated run. */}
        {intake.filed ? (
          <div className="flex flex-col gap-2 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-3 py-2.5">
            <div className="flex min-w-0 items-center gap-2">
              <CheckCircle2 className="size-3.5 flex-none text-emerald-500" />
              <span className="min-w-0 truncate text-[12px] text-foreground">
                Filed{' '}
                <span className="font-mono">
                  {intake.filed.provider === 'github'
                    ? `#${intake.filed.number}`
                    : intake.filed.identifier}
                </span>{' '}
                · {intake.filed.title}
              </span>
              {filedUrl ? (
                <button
                  type="button"
                  onClick={() => void api.shell.openUrl(filedUrl)}
                  className="inline-flex flex-none items-center gap-1 text-[11px] text-muted-foreground transition-colors hover:text-foreground"
                >
                  Open
                  <ExternalLink className="size-3" />
                </button>
              ) : null}
            </div>
            {intake.gate.eligible ? (
              <button
                type="button"
                onClick={intake.startGatedRun}
                className="inline-flex items-center gap-1.5 self-start rounded-md border border-border px-2.5 py-1.5 text-[11.5px] text-foreground transition-colors hover:border-muted-foreground/40"
              >
                <Play className="size-3.5" />
                Start gated run
              </button>
            ) : intake.filed.provider === 'linear' ? (
              // D3: honest, not silent — Linear issues can't start a gated run.
              <span className="text-[11px] text-muted-foreground">
                Gated runs: GitHub issues only.
              </span>
            ) : !intake.gate.eligible && intake.gate.reason === 'remote-repo' ? (
              <span className="text-[11px] text-muted-foreground">
                Gated runs start locally — this repo is remote (SSH).
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
      <IssueSpecInterviewDialog
        open={specOpen}
        onOpenChange={setSpecOpen}
        repo={repo}
        scope={scope}
        seedIntent={specSeed}
        resetVersion={specResetVersion}
        onApplyDraft={(draft) => {
          intake.applyDraft(draft)
          setSpecSeed('')
          setSpecResetVersion((value) => value + 1)
        }}
      />
    </div>
  )
}
