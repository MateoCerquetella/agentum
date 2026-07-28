import { api } from '@/tauri'
import { useCallback, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Plus, Upload } from 'lucide-react'
import { MAX_SSH_RELAY_GRACE_PERIOD_SECONDS, type SshTarget } from '@/shared/ssh-types'
import { SSH_TERMINATE_RECONNECT_REQUIRED } from '@/shared/constants'
import { useAppStore } from '@/store'
import { useMountedRef } from '@/hooks/useMountedRef'
import { Button } from '../ui/button'
import { removeSshTargetWithBestEffortCleanup } from './ssh-target-remove'
import {
  resolveServerHostIdForConnection,
  syncServerHostAuthForTarget,
  testServerHost
} from '@/runtime/server-host-client'
import { SshTargetCard } from './SshTargetCard'
import { SshTargetDestructiveActions } from './SshTargetDestructiveActions'
import { SshTargetForm, EMPTY_FORM, type EditingTarget } from './SshTargetForm'
import {
  getEditingTargetForSshTarget,
  getSshTargetDraftConnectionFields,
  isRelayGracePeriodValid,
  parseRelayGracePeriodSeconds
} from './ssh-target-draft'


type SshPaneProps = Record<string, never>

export function SshPane(_props: SshPaneProps): React.JSX.Element {
  const [targets, setTargets] = useState<SshTarget[]>([])
  // Why: connection states are already hydrated and kept up-to-date by the
  // global store (via useIpcEvents.ts). Reading from the store avoids
  // duplicating the onStateChanged listener and per-target getState IPC calls.
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)
  const setSshConnectionState = useAppStore((s) => s.setSshConnectionState)
  const recordFeatureInteraction = useAppStore((s) => s.recordFeatureInteraction)
  const [showForm, setShowForm] = useState(false)
  const [editingId, setEditingId] = useState<string | null>(null)
  const [form, setForm] = useState<EditingTarget>(EMPTY_FORM)
  const [testingIds, setTestingIds] = useState<Set<string>>(new Set())
  const mountedRef = useMountedRef()

  const setSshTargetsMetadata = useAppStore((s) => s.setSshTargetsMetadata)
  const clearRemovedSshTargetState = useAppStore((s) => s.clearRemovedSshTargetState)

  const loadTargets = useCallback(
    async (opts?: { signal?: AbortSignal }) => {
      try {
        const result = (await api.ssh.listTargets()) as SshTarget[]
        if (opts?.signal?.aborted || !mountedRef.current) {
          return
        }
        setTargets(result)
        setSshTargetsMetadata(result)
      } catch {
        if (!opts?.signal?.aborted && mountedRef.current) {
          toast.error('Failed to load SSH targets')
        }
      }
    },
    [mountedRef, setSshTargetsMetadata]
  )

  useEffect(() => {
    const abortController = new AbortController()
    void loadTargets({ signal: abortController.signal })
    return () => abortController.abort()
  }, [loadTargets])

  const handleSave = async (): Promise<void> => {
    const { host, configHost, username, port } = getSshTargetDraftConnectionFields(form)
    if (!host) {
      toast.error('Host or SSH config alias is required')
      return
    }

    if (isNaN(port) || port < 1 || port > 65535) {
      toast.error('Port must be between 1 and 65535')
      return
    }

    const graceSeconds = parseRelayGracePeriodSeconds(form)
    if (!isRelayGracePeriodValid(form, graceSeconds)) {
      toast.error(
        `Relay grace period must be between 60 and ${MAX_SSH_RELAY_GRACE_PERIOD_SECONDS} seconds`
      )
      return
    }

    const target = {
      label: form.label.trim() || (username ? `${username}@${host}` : configHost),
      configHost,
      host,
      port,
      username,
      relayGracePeriodSeconds: graceSeconds,
      ...(form.identityFile.trim() ? { identityFile: form.identityFile.trim() } : {}),
      // Send the password verbatim — never trimmed. Leading/trailing whitespace
      // can be a real part of it (and pasting from a password manager often
      // carries it), so trimming would silently store the wrong secret and every
      // login would fail with "Permission denied". Guard on the trimmed value
      // only to treat a whitespace-only field as empty (→ omit it).
      ...(form.password.trim() ? { password: form.password } : {}),
      ...(form.proxyCommand.trim() ? { proxyCommand: form.proxyCommand.trim() } : {}),
      ...(form.jumpHost.trim() ? { jumpHost: form.jumpHost.trim() } : {})
    }

    try {
      // The pre-edit target: the server host row still carries these coords, so
      // the sync below can find it by them after the user changes the IP/user/
      // port (matching by the new coords alone would miss and silently no-op,
      // leaving every session dialing the old, dead address).
      const previous = editingId ? targets.find((t) => t.id === editingId) : undefined
      const saved = (await (editingId
        ? api.ssh.updateTarget({ id: editingId, updates: target })
        : api.ssh.addTarget({ target }))) as SshTarget | undefined
      recordFeatureInteraction('ssh')
      // Push the edited coords + (possibly re-entered) password/key to the
      // embedded server host so the daemon connects with the current address
      // and secret, not stale ones — what makes "fix the IP/password" work.
      if (saved?.id) {
        void syncServerHostAuthForTarget(saved, previous)
      }
      if (!mountedRef.current) {
        return
      }
      toast.success(editingId ? 'Target updated' : 'Target added')
      setShowForm(false)
      setEditingId(null)
      setForm(EMPTY_FORM)
      await loadTargets()
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Failed to save target')
      }
    }
  }

  const terminateSessionsWithReconnect = async (targetId: string): Promise<void> => {
    try {
      await api.ssh.terminateSessions({ targetId })
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      if (!message.includes(SSH_TERMINATE_RECONNECT_REQUIRED)) {
        throw err
      }
      // Why: disconnect is now non-destructive, so preserved remote PTYs may
      // require a fresh relay attachment before they can be explicitly killed.
      await api.ssh.connect({ targetId })
      await api.ssh.terminateSessions({ targetId })
    }
  }

  const handleRemove = async (id: string): Promise<void> => {
    try {
      await removeSshTargetWithBestEffortCleanup(api.ssh, id)
      // Why: a deleted passphrase-gated target may still have deferred
      // reconnect metadata; clear it so focused SSH tabs stop retrying it.
      clearRemovedSshTargetState(id)
      if (mountedRef.current) {
        toast.success('Target removed')
      }
      await loadTargets()
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Failed to remove target')
      }
    }
  }

  const handleEdit = (target: SshTarget): void => {
    setEditingId(target.id)
    setForm(getEditingTargetForSshTarget(target))
    setShowForm(true)
  }

  const handleConnect = async (targetId: string): Promise<void> => {
    // Why: the native ssh_connect transport was never ported (returns null), so
    // this used to silently leave the target "Disconnected". With sessions now
    // routed through the embedded server's host_runtime (it SSHes per session,
    // no persistent client connection), "Connect" means: register the target as
    // a server host and probe it over SSH. A successful probe marks it connected
    // and ensures the host exists so remote workspaces/terminals can run on it.
    recordFeatureInteraction('ssh')
    setSshConnectionState(targetId, {
      targetId,
      status: 'connecting',
      error: null,
      reconnectAttempt: 0
    })
    try {
      const hostId = await resolveServerHostIdForConnection(targetId)
      if (!hostId) {
        throw new Error('Could not register this host with the server')
      }
      const probe = await testServerHost(hostId)
      if (!mountedRef.current) {
        return
      }
      if (probe.ok) {
        setSshConnectionState(targetId, {
          targetId,
          status: 'connected',
          error: null,
          reconnectAttempt: 0
        })
        toast.success(
          probe.tmux ? 'Connected' : 'Connected — but tmux is missing on the host'
        )
      } else {
        setSshConnectionState(targetId, {
          targetId,
          status: 'error',
          error: probe.message,
          reconnectAttempt: 0
        })
        toast.error(probe.message || 'Connection failed')
      }
    } catch (err) {
      if (mountedRef.current) {
        const message = err instanceof Error ? err.message : 'Connection failed'
        setSshConnectionState(targetId, {
          targetId,
          status: 'error',
          error: message,
          reconnectAttempt: 0
        })
        toast.error(message)
      }
    }
  }

  const handleDisconnect = async (targetId: string): Promise<void> => {
    try {
      await api.ssh.disconnect({ targetId })
      recordFeatureInteraction('ssh')
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Disconnect failed')
    }
  }

  const handleTerminateSessions = async (targetId: string): Promise<void> => {
    try {
      await terminateSessionsWithReconnect(targetId)
      toast.success('Remote terminals ended')
    } catch (err) {
      toast.error(err instanceof Error ? err.message : 'Failed to end remote terminals')
    }
  }

  const handleResetRelay = async (targetId: string): Promise<void> => {
    try {
      await api.ssh.resetRelay({ targetId })
      if (mountedRef.current) {
        toast.success('Remote relay reset')
      }
      await loadTargets()
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Failed to reset remote relay')
      }
    }
  }

  const handleTest = async (targetId: string): Promise<void> => {
    setTestingIds((prev) => new Set(prev).add(targetId))
    try {
      // Why: route Test through the server probe (same backend as Connect and as
      // real sessions). The native ssh_test_connection mishandles a target whose
      // configHost is the IP itself — it treats it as an ssh-config alias and
      // drops `-p <port>`, so ssh falls back to port 22 and times out. The server
      // probe always passes the explicit port + identity, so the test reflects
      // exactly how sessions connect.
      const hostId = await resolveServerHostIdForConnection(targetId)
      if (!hostId) {
        throw new Error('Could not register this host with the server')
      }
      const probe = await testServerHost(hostId)
      recordFeatureInteraction('ssh')
      if (mountedRef.current) {
        if (probe.ok) {
          toast.success(
            probe.tmux ? 'Connection successful' : 'Connected — but tmux is missing on the host'
          )
        } else {
          toast.error(probe.message || 'Connection test failed')
        }
      }
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Test failed')
      }
    } finally {
      if (mountedRef.current) {
        setTestingIds((prev) => {
          const next = new Set(prev)
          next.delete(targetId)
          return next
        })
      }
    }
  }

  const handleImport = async (): Promise<void> => {
    try {
      const imported = (await api.ssh.importConfig()) as SshTarget[]
      recordFeatureInteraction('ssh')
      if (mountedRef.current) {
        if (imported.length === 0) {
          toast('No new hosts found in ~/.ssh/config')
        } else {
          toast.success(`Imported ${imported.length} host${imported.length > 1 ? 's' : ''}`)
        }
      }
      await loadTargets()
    } catch (err) {
      if (mountedRef.current) {
        toast.error(err instanceof Error ? err.message : 'Import failed')
      }
    }
  }

  const cancelForm = (): void => {
    setShowForm(false)
    setEditingId(null)
    setForm(EMPTY_FORM)
  }

  return (
    <div className="space-y-4">
      {/* Header row */}
      <div className="flex items-center justify-between gap-3">
        <div className="space-y-0.5">
          <p className="text-sm font-medium">Targets</p>
          <p className="text-xs text-muted-foreground">
            Add a remote host to connect to it in Agentum.
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1.5">
          <Button
            variant="outline"
            size="xs"
            onClick={() => void handleImport()}
            className="gap-1.5"
          >
            <Upload className="size-3" />
            Import
          </Button>
          {!showForm ? (
            <Button
              variant="outline"
              size="xs"
              onClick={() => {
                setEditingId(null)
                setForm(EMPTY_FORM)
                setShowForm(true)
              }}
              className="gap-1.5"
            >
              <Plus className="size-3" />
              Add Target
            </Button>
          ) : null}
        </div>
      </div>

      <SshTargetDestructiveActions
        connectionStates={sshConnectionStates}
        onRemove={handleRemove}
        onResetRelay={handleResetRelay}
        onTerminateSessions={handleTerminateSessions}
      >
        {({ busyActionForTarget, requestRemove, requestResetRelay, requestTerminateSessions }) => (
          <>
            {/* Target list */}
            {targets.length === 0 && !showForm ? (
              <div className="flex items-center justify-center rounded-lg border border-dashed border-border/60 bg-card/30 px-4 py-5 text-sm text-muted-foreground">
                No SSH targets configured.
              </div>
            ) : (
              <div className="space-y-2">
                {targets.map((target) => (
                  <SshTargetCard
                    key={target.id}
                    target={target}
                    state={sshConnectionStates.get(target.id)}
                    testing={testingIds.has(target.id)}
                    busyAction={busyActionForTarget(target.id)}
                    onConnect={handleConnect}
                    onDisconnect={handleDisconnect}
                    onTerminateSessions={(id) =>
                      requestTerminateSessions({ id, label: target.label })
                    }
                    onResetRelay={(id) => requestResetRelay({ id, label: target.label })}
                    onTest={handleTest}
                    onEdit={handleEdit}
                    onRemove={(id) => requestRemove({ id, label: target.label })}
                  />
                ))}
              </div>
            )}

            {/* Add/Edit form */}
            {showForm ? (
              <SshTargetForm
                editingId={editingId}
                form={form}
                onFormChange={setForm}
                onSave={() => void handleSave()}
                onCancel={cancelForm}
              />
            ) : null}
          </>
        )}
      </SshTargetDestructiveActions>
    </div>
  )
}
