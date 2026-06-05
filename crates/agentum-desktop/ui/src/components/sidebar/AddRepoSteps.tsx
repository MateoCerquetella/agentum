import { api } from '@/tauri'
/* eslint-disable max-lines -- Why: AddRepoDialog step views are already split from the parent,
   and keeping clone/remote/setup step props together avoids a larger wizard refactor in this
   leak fix. */
import React, { useCallback, useEffect, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Folder, FolderOpen, Settings } from 'lucide-react'
import { useAppStore } from '@/store'
import { DialogHeader, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { RemoteFileBrowser } from './RemoteFileBrowser'
import { SshTargetRow } from './SshTargetRow'
import { reposAddRemote } from '@/runtime/server-repo-client'
import { connectSshTargetViaServer } from '@/runtime/server-host-client'
import { useMountedRef } from '@/hooks/useMountedRef'
import type { AddRepoExistingWorkspaceSource } from '../../../../shared/telemetry-events'
import type { NestedRepoScanResult, Repo } from '../../../../shared/types'
import type { SshTarget, SshConnectionState } from '../../../../shared/ssh-types'
import { createNestedRepoTelemetryAttemptId } from '../../../../shared/nested-repo-telemetry'

// ── Remote project hook ─────────────────────────────────────────────

export function useRemoteRepo(
  fetchWorktrees: (repoId: string) => Promise<unknown>,
  setStep: (step: 'add' | 'clone' | 'remote' | 'create' | 'nested' | 'setup') => void,
  setAddedRepo: (repo: Repo | null) => void,
  closeModal: () => void,
  setExistingWorkspaceSource?: (source: AddRepoExistingWorkspaceSource) => void,
  scanNestedRepos?: (path: string, connectionId?: string) => Promise<NestedRepoScanResult | null>,
  showNestedRepoReview?: (
    scan: NestedRepoScanResult,
    selectedPath: string,
    connectionId: string,
    attemptId: string
  ) => void,
  onNestedScanResult?: (scan: NestedRepoScanResult | null, attemptId: string) => void
) {
  const [sshTargets, setSshTargets] = useState<(SshTarget & { state?: SshConnectionState })[]>([])
  const [selectedTargetId, setSelectedTargetId] = useState<string | null>(null)
  const [remotePath, setRemotePath] = useState('~/')
  const [remoteError, setRemoteError] = useState<string | null>(null)
  const [isAddingRemote, setIsAddingRemote] = useState(false)
  const remoteGenRef = useRef(0)
  const mountedRef = useMountedRef()
  // Why: connection state lives in the store (sshConnectionStates), updated by
  // connectSshTargetViaServer. The old native getState/onStateChanged this step
  // used are dead stubs, so the modal showed every target as Disconnected even
  // after a successful Connect. Drive the displayed state from the store.
  const sshConnectionStates = useAppStore((s) => s.sshConnectionStates)

  const resetRemoteState = useCallback(() => {
    remoteGenRef.current++
    setSshTargets([])
    setSelectedTargetId(null)
    setRemotePath('~/')
    setRemoteError(null)
    setIsAddingRemote(false)
  }, [])

  const handleOpenRemoteStep = useCallback(async () => {
    const gen = ++remoteGenRef.current
    setStep('remote')
    try {
      const targets = (await api.ssh.listTargets()) as SshTarget[]
      if (gen !== remoteGenRef.current) {
        return
      }
      const states = useAppStore.getState().sshConnectionStates
      const withState = targets.map((t) => ({ ...t, state: states.get(t.id) }))
      setSshTargets(withState)
      const connected = withState.find((t) => t.state?.status === 'connected')
      if (connected) {
        setSelectedTargetId(connected.id)
      }
    } catch {
      if (gen !== remoteGenRef.current) {
        return
      }
      setSshTargets([])
    }
  }, [setStep])

  // Why: keep the target list's connection state in sync while the dialog is
  // open, so clicking the inline Connect button updates the dot/label live.
  // Driven by the store's sshConnectionStates (the native onStateChanged event
  // was never emitted), so a server-backed Connect reflects immediately and
  // auto-selects the now-connected target.
  useEffect(() => {
    setSshTargets((prev) => prev.map((t) => ({ ...t, state: sshConnectionStates.get(t.id) })))
    for (const [targetId, state] of sshConnectionStates) {
      if (state.status === 'connected') {
        setSelectedTargetId((curr) => curr ?? targetId)
      }
    }
  }, [sshConnectionStates])

  const handleConnectTarget = useCallback(async (targetId: string) => {
    // Native ssh_connect is a no-op; connect through the server host probe.
    const result = await connectSshTargetViaServer(targetId)
    if (result.ok) {
      toast.success(result.message)
    } else {
      toast.error(result.message)
    }
  }, [])

  const handleAddRemoteRepo = useCallback(async () => {
    if (!selectedTargetId || !remotePath.trim()) {
      return
    }

    const trimmedRemotePath = remotePath.trim()
    setIsAddingRemote(true)
    setRemoteError(null)
    try {
      const attemptId = createNestedRepoTelemetryAttemptId()
      const scan = await scanNestedRepos?.(trimmedRemotePath, selectedTargetId)
      if (!mountedRef.current) {
        return
      }
      onNestedScanResult?.(scan ?? null, attemptId)
      if (scan?.selectedPathKind === 'non_git_folder' && scan.repos.length > 0) {
        showNestedRepoReview?.(scan, trimmedRemotePath, selectedTargetId, attemptId)
        return
      }
      // Register the remote repo on the server with its SSH connection. The
      // native addRemote command was a hardcoded "not available" stub; this
      // routes through /api/repos so the repo carries its connectionId and its
      // sessions run in tmux on the remote host.
      const result = await reposAddRemote(selectedTargetId, trimmedRemotePath)
      if ('error' in result) {
        throw new Error(result.error)
      }
      const repo = result.repo

      const state = useAppStore.getState()
      const existingIdx = state.repos.findIndex((r) => r.id === repo.id)
      if (existingIdx !== -1) {
        state.clearAgentumHookTrustForRepo(repo.id)
      }
      if (existingIdx === -1) {
        useAppStore.setState({ repos: [...state.repos, repo] })
      } else {
        const updated = [...state.repos]
        updated[existingIdx] = repo
        useAppStore.setState({ repos: updated })
      }

      if (!mountedRef.current) {
        return
      }
      toast.success('Remote project added', { description: repo.displayName })
      setAddedRepo(repo)
      setExistingWorkspaceSource?.('ssh_remote_path')
      await fetchWorktrees(repo.id)
      if (!mountedRef.current) {
        return
      }
      setStep('setup')
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      if (message.includes('Not a valid git repository')) {
        // Why: match the local add-project flow — show confirmation dialog so
        // users understand git features will be unavailable, rather than
        // silently adding as a folder.
        closeModal()
        useAppStore.getState().openModal('confirm-non-git-folder', {
          folderPath: trimmedRemotePath,
          connectionId: selectedTargetId
        })
        return
      }
      if (mountedRef.current) {
        setRemoteError(message)
      }
    } finally {
      if (mountedRef.current) {
        setIsAddingRemote(false)
      }
    }
  }, [
    selectedTargetId,
    remotePath,
    scanNestedRepos,
    showNestedRepoReview,
    onNestedScanResult,
    fetchWorktrees,
    mountedRef,
    setStep,
    setAddedRepo,
    closeModal,
    setExistingWorkspaceSource
  ])

  return {
    sshTargets,
    selectedTargetId,
    remotePath,
    remoteError,
    isAddingRemote,
    setSelectedTargetId,
    setRemotePath,
    setRemoteError,
    resetRemoteState,
    handleOpenRemoteStep,
    handleAddRemoteRepo,
    handleConnectTarget
  }
}

// ── Remote step ──────────────────────────────────────────────────────

type RemoteStepProps = {
  sshTargets: (SshTarget & { state?: SshConnectionState })[]
  selectedTargetId: string | null
  remotePath: string
  remoteError: string | null
  isAddingRemote: boolean
  onSelectTarget: (id: string) => void
  onRemotePathChange: (value: string) => void
  onAdd: () => void
  onOpenSshSettings: () => void
  onConnectTarget: (id: string) => Promise<void>
}

export function RemoteStep({
  sshTargets,
  selectedTargetId,
  remotePath,
  remoteError,
  isAddingRemote,
  onSelectTarget,
  onRemotePathChange,
  onAdd,
  onOpenSshSettings,
  onConnectTarget
}: RemoteStepProps): React.JSX.Element {
  const [browsing, setBrowsing] = useState(false)

  if (browsing && selectedTargetId) {
    return (
      <>
        <DialogHeader>
          <DialogTitle>Browse remote filesystem</DialogTitle>
          <DialogDescription>
            Navigate to a directory and click Select to choose it.
          </DialogDescription>
        </DialogHeader>
        <RemoteFileBrowser
          targetId={selectedTargetId}
          initialPath={remotePath || '~'}
          onSelect={(path) => {
            onRemotePathChange(path)
            setBrowsing(false)
          }}
          onCancel={() => setBrowsing(false)}
        />
      </>
    )
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>Open remote project</DialogTitle>
        <DialogDescription>
          Choose a connected SSH target and enter the path to a Git repository.
        </DialogDescription>
      </DialogHeader>

      <div className="space-y-3 pt-1">
        <div className="space-y-1">
          <label className="text-[11px] font-medium text-muted-foreground">SSH target</label>
          {sshTargets.length === 0 ? (
            <div className="space-y-1.5 py-1">
              <p className="text-xs text-muted-foreground">No SSH targets configured.</p>
              <Button
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={onOpenSshSettings}
              >
                <Settings className="size-3.5" />
                Add in Settings
              </Button>
            </div>
          ) : (
            <div className="space-y-1.5 max-h-64 overflow-y-auto pr-1 scrollbar-sleek">
              {sshTargets.map((target) => (
                <SshTargetRow
                  key={target.id}
                  target={target}
                  isSelected={selectedTargetId === target.id}
                  onSelect={onSelectTarget}
                  onConnect={onConnectTarget}
                />
              ))}
            </div>
          )}
        </div>

        <div className="space-y-1">
          <label className="text-[11px] font-medium text-muted-foreground">Remote path</label>
          <div className="flex gap-2">
            <Input
              value={remotePath}
              onChange={(e) => onRemotePathChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
                  e.preventDefault()
                  if (selectedTargetId && remotePath.trim() && !isAddingRemote) {
                    onAdd()
                  }
                }
              }}
              placeholder="/home/user/project"
              className="h-8 text-xs flex-1"
              disabled={isAddingRemote || !selectedTargetId}
            />
            <Button
              variant="outline"
              size="sm"
              className="h-8 px-2 shrink-0"
              onClick={() => setBrowsing(true)}
              disabled={!selectedTargetId || isAddingRemote}
            >
              <FolderOpen className="size-3.5" />
            </Button>
          </div>
        </div>

        {remoteError && <p className="text-[11px] text-destructive">{remoteError}</p>}

        <Button
          onClick={onAdd}
          disabled={!selectedTargetId || !remotePath.trim() || isAddingRemote}
          className="w-full"
        >
          {isAddingRemote ? 'Adding...' : 'Add remote project'}
        </Button>
      </div>
    </>
  )
}

// ── Clone step ───────────────────────────────────────────────────────

type CloneStepProps = {
  cloneUrl: string
  cloneDestination: string
  cloneError: string | null
  cloneProgress: { phase: string; percent: number } | null
  isCloning: boolean
  disableDestinationPicker?: boolean
  onUrlChange: (value: string) => void
  onDestChange: (value: string) => void
  onPickDestination: () => void
  onClone: () => void
}

export function CloneStep({
  cloneUrl,
  cloneDestination,
  cloneError,
  cloneProgress,
  isCloning,
  disableDestinationPicker = false,
  onUrlChange,
  onDestChange,
  onPickDestination,
  onClone
}: CloneStepProps): React.JSX.Element {
  const canClone = !!cloneUrl.trim() && !!cloneDestination.trim() && !isCloning
  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>): void => {
    if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
      e.preventDefault()
      if (canClone) {
        onClone()
      }
    }
  }
  return (
    <>
      <DialogHeader>
        <DialogTitle>Clone from URL</DialogTitle>
        <DialogDescription>Enter the Git URL and choose where to clone it.</DialogDescription>
      </DialogHeader>

      <div className="space-y-3 pt-1">
        <div className="space-y-1">
          <label className="text-[11px] font-medium text-muted-foreground">Git URL</label>
          <Input
            value={cloneUrl}
            onChange={(e) => onUrlChange(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="https://github.com/user/repo.git"
            className="h-8 text-xs"
            disabled={isCloning}
            autoFocus
          />
        </div>

        <div className="space-y-1">
          <label className="text-[11px] font-medium text-muted-foreground">Clone location</label>
          <div className="flex gap-2">
            <Input
              value={cloneDestination}
              onChange={(e) => onDestChange(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="/path/to/destination"
              className="h-8 text-xs flex-1"
              disabled={isCloning}
            />
            <Button
              variant="outline"
              size="sm"
              className="h-8 px-2 shrink-0"
              onClick={onPickDestination}
              disabled={isCloning || disableDestinationPicker}
              title={disableDestinationPicker ? 'Enter a server path manually' : 'Choose folder'}
            >
              <Folder className="size-3.5" />
            </Button>
          </div>
        </div>

        {cloneError && <p className="text-[11px] text-destructive">{cloneError}</p>}

        <Button
          onClick={onClone}
          disabled={!cloneUrl.trim() || !cloneDestination.trim() || isCloning}
          className="w-full"
        >
          {isCloning ? 'Cloning...' : 'Clone'}
        </Button>

        {/* Why: progress bar lives below the button so it doesn't push the
           button down when it appears mid-clone. */}
        {isCloning && cloneProgress && (
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>{cloneProgress.phase}</span>
              <span>{cloneProgress.percent}%</span>
            </div>
            <div className="h-1.5 w-full rounded-full bg-secondary overflow-hidden">
              <div
                className="h-full rounded-full bg-foreground transition-[width] duration-300 ease-out"
                style={{ width: `${cloneProgress.percent}%` }}
              />
            </div>
          </div>
        )}
      </div>
    </>
  )
}
