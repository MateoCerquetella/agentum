import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { ArrowDown, ArrowUp, Check, Gauge, Monitor, Server } from 'lucide-react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger
} from '@/components/ui/dropdown-menu'
import { useAppStore } from '../../store'
import { useHostIoRate } from '@/hooks/use-host-io-rate'
import { formatRate, LOCAL_HOST_KEY, type HostKey } from '@/runtime/io-meter'

// Where the user's chosen host survives reload. localStorage (not the IPC UI
// store) keeps this purely renderer-local — the selection is a view preference,
// not shared app state.
const SELECTED_HOST_STORAGE_KEY = 'agentum.statusBar.ioHostKey'

function loadSelectedHostKey(): HostKey {
  try {
    return globalThis.localStorage?.getItem(SELECTED_HOST_STORAGE_KEY) || LOCAL_HOST_KEY
  } catch {
    return LOCAL_HOST_KEY
  }
}

function persistSelectedHostKey(key: HostKey): void {
  try {
    globalThis.localStorage?.setItem(SELECTED_HOST_STORAGE_KEY, key)
  } catch {
    // localStorage unavailable (headless) — selection just won't persist.
  }
}

type IoHostOption = {
  key: HostKey
  label: string
  kind: 'local' | 'ssh'
}

/**
 * Single status-bar chip showing live ↓ in / ↑ out WS throughput for one host,
 * with a dropdown to pick which host (local + each SSH host). Mirrors the TUI's
 * I/O meter; the byte rates come from per-host counters fed by the session WS
 * stream (see io-meter.ts), sampled on an interval by useHostIoRate.
 */
export function IoStatusSegment({
  compact,
  iconOnly
}: {
  compact: boolean
  iconOnly: boolean
}): React.JSX.Element {
  const sshTargetLabels = useAppStore((s) => s.sshTargetLabels)
  const hostMetaByKey = useAppStore((s) => s.hostMetaByKey)
  const recordFeatureInteraction = useAppStore((s) => s.recordFeatureInteraction)

  const [selectedHostKey, setSelectedHostKey] = useState<HostKey>(loadSelectedHostKey)

  // local first, then every configured SSH host. SSH labels are the source of
  // truth for which hosts exist (same source SshStatusSegment uses); host meta
  // refines the local label when readiness has resolved.
  const options = useMemo<IoHostOption[]>(() => {
    const localLabel = hostMetaByKey[LOCAL_HOST_KEY]?.label || 'This device'
    const out: IoHostOption[] = [{ key: LOCAL_HOST_KEY, label: localLabel, kind: 'local' }]
    for (const [connectionId, label] of sshTargetLabels.entries()) {
      out.push({ key: `ssh:${connectionId}`, label, kind: 'ssh' })
    }
    return out
  }, [hostMetaByKey, sshTargetLabels])

  // If the selected host disappears (SSH target removed), fall back to local so
  // the chip never points at a host that no longer exists.
  useEffect(() => {
    if (!options.some((o) => o.key === selectedHostKey)) {
      setSelectedHostKey(LOCAL_HOST_KEY)
      persistSelectedHostKey(LOCAL_HOST_KEY)
    }
  }, [options, selectedHostKey])

  const { inRate, outRate } = useHostIoRate(selectedHostKey)

  const selected = options.find((o) => o.key === selectedHostKey) ?? options[0]

  const handleSelect = useCallback((key: HostKey): void => {
    setSelectedHostKey(key)
    persistSelectedHostKey(key)
  }, [])

  const down = formatRate(inRate)
  const up = formatRate(outRate)

  return (
    <DropdownMenu
      onOpenChange={(open) => {
        if (open) {
          recordFeatureInteraction('ssh')
        }
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 cursor-pointer rounded px-1 py-0.5 hover:bg-accent/70"
          aria-label={`I/O speed for ${selected?.label ?? 'host'}`}
          title={`I/O speed · ${selected?.label ?? 'host'}`}
        >
          {iconOnly ? (
            <span className="inline-flex items-center gap-1 text-muted-foreground">
              <Gauge className="size-3" />
            </span>
          ) : (
            <span className="inline-flex items-center gap-1.5 tabular-nums">
              {!compact && selected?.kind === 'ssh' && (
                <Server className="size-3 text-muted-foreground" />
              )}
              <span className="inline-flex items-center gap-0.5 text-muted-foreground">
                <ArrowDown className="size-3" />
                <span>{down}</span>
              </span>
              <span className="inline-flex items-center gap-0.5 text-muted-foreground">
                <ArrowUp className="size-3" />
                <span>{up}</span>
              </span>
            </span>
          )}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="end" sideOffset={8} className="w-[min(16rem,calc(100vw-1rem))]">
        <DropdownMenuLabel className="flex items-center gap-1.5">
          <Gauge className="size-3.5 text-muted-foreground" />
          I/O Speed Host
        </DropdownMenuLabel>
        {options.map((option) => (
          <DropdownMenuItem
            key={option.key}
            onSelect={(event) => {
              event.preventDefault()
              handleSelect(option.key)
            }}
          >
            {option.kind === 'local' ? (
              <Monitor className="size-3.5 text-muted-foreground" />
            ) : (
              <Server className="size-3.5 text-muted-foreground" />
            )}
            <span className="min-w-0 flex-1 truncate">{option.label}</span>
            {option.key === selectedHostKey ? (
              <Check className="ml-auto size-3.5 text-foreground" />
            ) : null}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
