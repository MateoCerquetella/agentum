import React, { useCallback, useMemo, useState } from 'react'
import { ArrowDown, ArrowUp, Check, Gauge, Server } from 'lucide-react'
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
}

/**
 * Single status-bar chip showing live ↓ in / ↑ out WS throughput for one SSH
 * host, with a dropdown to pick which one. I/O throughput is only meaningful for
 * remote hosts — local sessions stream over loopback — so the local device is
 * not offered and the chip hides entirely when no SSH host is configured.
 * Mirrors the TUI's I/O meter; byte rates come from per-host counters fed by the
 * session WS stream (see io-meter.ts), sampled on an interval by useHostIoRate.
 */
export function IoStatusSegment({
  compact,
  iconOnly
}: {
  compact: boolean
  iconOnly: boolean
}): React.JSX.Element | null {
  const sshTargetLabels = useAppStore((s) => s.sshTargetLabels)
  const recordFeatureInteraction = useAppStore((s) => s.recordFeatureInteraction)

  const [selectedHostKey, setSelectedHostKey] = useState<HostKey>(loadSelectedHostKey)

  // Every configured SSH host (no local — I/O speed is a remote-only metric).
  // SSH labels are the source of truth for which hosts exist (same source
  // SshStatusSegment uses).
  const options = useMemo<IoHostOption[]>(() => {
    const out: IoHostOption[] = []
    for (const [connectionId, label] of sshTargetLabels.entries()) {
      out.push({ key: `ssh:${connectionId}`, label })
    }
    return out
  }, [sshTargetLabels])

  // Effective host for live rates + display: the saved choice when it's a known
  // SSH host, else the first one. We deliberately do NOT persist this fallback or
  // mutate the saved selection. On reload, SSH host labels hydrate asynchronously,
  // so a saved choice is briefly "unknown" — keeping the saved key intact lets
  // the chip snap back to it the moment its option appears (the "doesn't remember
  // my I/O host" bug).
  const effectiveHostKey = options.some((o) => o.key === selectedHostKey)
    ? selectedHostKey
    : (options[0]?.key ?? selectedHostKey)

  const { inRate, outRate } = useHostIoRate(effectiveHostKey)

  const selected = options.find((o) => o.key === effectiveHostKey) ?? options[0]

  const handleSelect = useCallback((key: HostKey): void => {
    setSelectedHostKey(key)
    persistSelectedHostKey(key)
  }, [])

  const down = formatRate(inRate)
  const up = formatRate(outRate)

  // I/O speed only applies to remote (SSH) hosts; with none configured there is
  // nothing to show, so the chip disappears rather than render an empty picker.
  if (options.length === 0) {
    return null
  }

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
              {!compact && <Server className="size-3 text-muted-foreground" />}
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
            <Server className="size-3.5 text-muted-foreground" />
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
