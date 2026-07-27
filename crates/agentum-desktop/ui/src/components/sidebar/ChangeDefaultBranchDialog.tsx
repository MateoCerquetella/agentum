import React, { useEffect, useRef, useState } from 'react'
import { AlertTriangle, Check, GitBranch, LoaderCircle } from 'lucide-react'
import { toast } from 'sonner'

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import { ScrollArea } from '@/components/ui/scroll-area'
import { cn } from '@/lib/utils'
import { api } from '@/tauri'
import type { Repo } from '@/shared/types'

// Wire shapes returned by the gh_repo_branches / gh_set_default_branch commands.
// Both use the codebase's { ok, ... } | { ok:false, error } envelope. Modeled as
// flat optional-field shapes (not discriminated unions) because the values are
// untyped JSON crossing the Tauri IPC boundary, and so reads don't hinge on
// control-flow narrowing — a single `error` string drives the failure message.
type BranchesResult = {
  ok: boolean
  branches?: string[]
  default?: string
  error?: string
}
type SetResult = { ok: boolean; error?: string }

type ChangeDefaultBranchDialogProps = {
  /** The repo whose GitHub default branch is being changed; null closes the dialog. */
  repo: Repo | null
  onClose: () => void
}

/**
 * Picker for a repository's GitHub default branch, opened from the project
 * right-click menu. It reads the live branch list + current default from GitHub
 * (so the choice matches what will actually change) and applies the selection via
 * `gh repo edit --default-branch`. Changing the default re-targets every new PR and
 * the branch fresh clones check out, so the dialog itself — with explicit warning
 * copy and a deliberate "Set as default" action — is the confirmation step.
 */
export function ChangeDefaultBranchDialog({
  repo,
  onClose
}: ChangeDefaultBranchDialogProps): React.JSX.Element {
  const open = repo !== null
  // Radix keeps content mounted through the close animation; retain the last repo
  // so the title/body don't blank out as `repo` flips to null on close.
  const retainedRepo = useRef<Repo | null>(repo)
  if (repo) {
    retainedRepo.current = repo
  }
  const shownRepo = repo ?? retainedRepo.current

  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [branches, setBranches] = useState<string[]>([])
  const [currentDefault, setCurrentDefault] = useState('')
  const [selected, setSelected] = useState('')
  const [applying, setApplying] = useState(false)

  // Fetch branches whenever a repo opens the dialog. Keyed on id+path so reopening
  // for a different repo refetches; a cancel flag drops a late response if the user
  // closes or switches repos before it lands.
  useEffect(() => {
    if (!repo) {
      return
    }
    let cancelled = false
    setLoading(true)
    setError(null)
    setBranches([])
    setCurrentDefault('')
    setSelected('')
    void (api.gh.repoBranches({ repoPath: repo.path }) as Promise<BranchesResult>)
      .then((result) => {
        if (cancelled) {
          return
        }
        if (result.ok) {
          setBranches(result.branches ?? [])
          setCurrentDefault(result.default ?? '')
          setSelected(result.default ?? '')
        } else {
          setError(result.error ?? "Couldn't load branches.")
        }
        setLoading(false)
      })
      .catch((e: unknown) => {
        if (cancelled) {
          return
        }
        setError(e instanceof Error ? e.message : String(e))
        setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [repo?.id, repo?.path])

  const unchanged = selected === '' || selected === currentDefault
  const canApply = !loading && !applying && error === null && !unchanged

  const handleApply = async (): Promise<void> => {
    if (!repo || !canApply) {
      return
    }
    setApplying(true)
    try {
      const result = (await api.gh.setDefaultBranch({
        repoPath: repo.path,
        branch: selected
      })) as SetResult
      if (result.ok) {
        toast.success(`Default branch set to “${selected}”.`, {
          description: shownRepo?.displayName
        })
        onClose()
      } else {
        toast.error("Couldn't change default branch", {
          description: result.error ?? 'The gh command failed.'
        })
      }
    } catch (e: unknown) {
      toast.error("Couldn't change default branch", {
        description: e instanceof Error ? e.message : String(e)
      })
    } finally {
      setApplying(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Change default branch</DialogTitle>
          <DialogDescription>
            {shownRepo ? <span className="font-medium">{shownRepo.displayName}</span> : null} — new
            pull requests will target this branch and it&apos;s what fresh clones check out.
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <LoaderCircle className="size-4 animate-spin" />
            Loading branches…
          </div>
        ) : error ? (
          <div className="flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" />
            <span>{error}</span>
          </div>
        ) : (
          <ScrollArea className="max-h-72 rounded-md border">
            <div className="p-1">
              {branches.map((branch) => {
                const isSelected = branch === selected
                return (
                  <button
                    key={branch}
                    type="button"
                    onClick={() => setSelected(branch)}
                    className={cn(
                      'flex w-full items-center justify-between gap-2 rounded-sm px-3 py-2 text-left text-sm',
                      isSelected ? 'bg-accent text-accent-foreground' : 'hover:bg-accent/50'
                    )}
                  >
                    <span className="flex min-w-0 items-center gap-2">
                      <GitBranch className="size-3.5 shrink-0 opacity-70" />
                      <span className="truncate">{branch}</span>
                      {branch === currentDefault ? (
                        <Badge variant="secondary" className="ml-1 shrink-0">
                          current
                        </Badge>
                      ) : null}
                    </span>
                    {isSelected ? <Check className="size-4 shrink-0" /> : null}
                  </button>
                )
              })}
            </div>
          </ScrollArea>
        )}

        <DialogFooter>
          <Button type="button" variant="outline" onClick={onClose} disabled={applying}>
            Cancel
          </Button>
          <Button type="button" onClick={() => void handleApply()} disabled={!canApply}>
            {applying ? <LoaderCircle className="size-4 animate-spin" /> : null}
            Set as default
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
