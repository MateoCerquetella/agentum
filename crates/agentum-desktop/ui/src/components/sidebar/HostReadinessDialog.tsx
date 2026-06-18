import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { AlertTriangle, Check, Loader2, Plus } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from '@/components/ui/dialog'
import {
  getFullHostReadiness,
  provisionHostSkills,
  resolveServerHostIdForHostKey,
  type FullHostReadiness
} from '@/runtime/server-host-client'
import { Button } from '@/components/ui/button'

/**
 * Host Readiness & Provisioning (Option B). Opened from the sidebar host
 * header. Shows the host's REQUIRED tier (tmux/git — gates running any agent)
 * and OPTIONAL capabilities (agentum skills) that can be added per host. Skills
 * missing on the host get a one-click "Add" that copies them over SSH via
 * `/api/hosts/{id}/provision-skills`. The Playwright/MCP browser stays optional
 * (a host that doesn't want it simply never adds the verification skill).
 */
export function HostReadinessDialog({
  hostKey,
  hostLabel,
  open,
  onOpenChange
}: {
  hostKey: string | null
  hostLabel: string
  open: boolean
  onOpenChange: (open: boolean) => void
}): React.JSX.Element {
  const [hostId, setHostId] = useState<string | null>(null)
  const [readiness, setReadiness] = useState<FullHostReadiness | null>(null)
  const [loading, setLoading] = useState(false)
  const [busySkill, setBusySkill] = useState<string | null>(null)

  useEffect(() => {
    if (!open || !hostKey) {
      return
    }
    let cancelled = false
    setLoading(true)
    setReadiness(null)
    setHostId(null)
    void (async () => {
      try {
        const id = await resolveServerHostIdForHostKey(hostKey)
        if (cancelled) return
        setHostId(id)
        if (!id) {
          setLoading(false)
          return
        }
        const report = await getFullHostReadiness(id)
        if (!cancelled) setReadiness(report)
      } catch (err) {
        if (!cancelled) {
          console.warn('[agentum] host readiness failed', err)
          toast.error('Failed to read host readiness')
        }
      } finally {
        if (!cancelled) setLoading(false)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [open, hostKey])

  const addSkill = async (skillId: string): Promise<void> => {
    if (!hostId) return
    setBusySkill(skillId)
    try {
      const report = await provisionHostSkills(hostId, [skillId])
      setReadiness(report)
      toast.success(`Synced ${skillId} to ${hostLabel}`)
    } catch (err) {
      toast.error(`Failed to sync ${skillId}`, {
        description: err instanceof Error ? err.message : undefined
      })
    } finally {
      setBusySkill(null)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>{hostLabel} — Host readiness</DialogTitle>
          <DialogDescription>
            What this host has for running agents, plus optional capabilities you can add to it.
          </DialogDescription>
        </DialogHeader>

        {loading ? (
          <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" /> Probing host…
          </div>
        ) : !hostId ? (
          <p className="py-4 text-sm text-muted-foreground">
            Couldn’t resolve this host on the server.
          </p>
        ) : !readiness ? (
          <p className="py-4 text-sm text-muted-foreground">No readiness data for this host.</p>
        ) : (
          <div className="space-y-4 text-sm">
            <section className="space-y-1.5">
              <h4 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Required (to run any agent here)
              </h4>
              {readiness.required.map((dep) => (
                <ReadinessRow key={dep.id} label={dep.label} ok={dep.installed} />
              ))}
            </section>

            <section className="space-y-1.5">
              <h4 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                Optional capabilities — add to this host
              </h4>
              {readiness.skills.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  No provisionable skills are installed on this machine to copy over.
                </p>
              ) : (
                readiness.skills.map((skill) => (
                  <div key={skill.id} className="flex items-center justify-between gap-2">
                    <ReadinessRow label={skill.label} ok={skill.installed} />
                    {skill.installed ? null : (
                      <Button
                        size="sm"
                        variant="outline"
                        className="h-6 gap-1 px-2 text-xs"
                        disabled={busySkill !== null}
                        onClick={() => void addSkill(skill.id)}
                      >
                        {busySkill === skill.id ? (
                          <Loader2 className="size-3 animate-spin" />
                        ) : (
                          <Plus className="size-3" />
                        )}
                        Add
                      </Button>
                    )}
                  </div>
                ))
              )}
            </section>
          </div>
        )}
      </DialogContent>
    </Dialog>
  )
}

function ReadinessRow({ label, ok }: { label: string; ok: boolean }): React.JSX.Element {
  return (
    <div className="flex items-center gap-1.5">
      {ok ? (
        <Check className="size-3.5 shrink-0 text-emerald-500" />
      ) : (
        <AlertTriangle className="size-3.5 shrink-0 text-amber-500" />
      )}
      <span className={ok ? '' : 'text-muted-foreground'}>{label}</span>
    </div>
  )
}
