// Spec 015 F3: the Tracker tab's "New issue" intake — written intent → drafted
// issue → filed (GitHub/Linear) → optional gated run. A SIBLING of the
// untouched ProjectBindingEditor: the tab keeps its config half, this is the
// doing half. All behavior lives in `useTrackerIntake`; this file only renders
// its state (the CreateIssuePanel look, adapted to the hub's card style).
import React from 'react'
import { CheckCircle2, ExternalLink, Loader2, Play, Sparkles } from 'lucide-react'

import { api } from '@/tauri'
import { cn } from '@/lib/utils'
import type { Repo } from '@/shared/types'
import { useTrackerIntake } from './use-tracker-intake'

export function TrackerIntakePanel({
  repo,
  bindingVersion
}: {
  repo: Repo
  /** Bumped when ProjectBindingEditor saves — re-resolves the provider. */
  bindingVersion: number
}): React.JSX.Element {
  const intake = useTrackerIntake({ repo, bindingVersion })
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
        {/* Provider toggle — only when BOTH a GitHub Project and Linear resolve (AC 10). */}
        {intake.provider === 'ambiguous' ? (
          <div className="flex items-center gap-1.5">
            <span className="text-[11px] text-muted-foreground">File into</span>
            {(['github', 'linear'] as const).map((p) => (
              <button
                key={p}
                type="button"
                onClick={() => intake.setProviderChoice(p)}
                className={cn(
                  'rounded-md border px-2 py-0.5 text-[11px] capitalize transition-colors',
                  intake.effectiveProvider === p
                    ? 'border-muted-foreground/40 bg-secondary text-foreground'
                    : 'border-border text-muted-foreground hover:border-muted-foreground/25'
                )}
              >
                {p}
              </button>
            ))}
          </div>
        ) : null}

        <label className="flex flex-col gap-1.5">
          <span className="text-[11px] text-muted-foreground">What do you want to do?</span>
          <textarea
            value={intake.intent}
            onChange={(event) => intake.setIntent(event.target.value)}
            rows={2}
            placeholder="Describe the work — a title and an SDD-shaped body get drafted from it."
            className="resize-none rounded-md border border-input bg-secondary px-2.5 py-2 text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground/70 focus-visible:border-ring"
          />
        </label>

        <button
          type="button"
          onClick={intake.draft}
          disabled={!intake.canDraft}
          className="inline-flex items-center gap-1.5 self-start rounded-md border border-border px-2.5 py-1.5 text-[11.5px] text-foreground transition-colors hover:border-muted-foreground/40 disabled:cursor-not-allowed disabled:opacity-50"
        >
          {intake.phase === 'drafting' ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Sparkles className="size-3.5" />
          )}
          {intake.phase === 'drafting' ? 'Drafting…' : hasDraft ? 'Redraft from intent' : 'Draft issue'}
        </button>

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
    </div>
  )
}
