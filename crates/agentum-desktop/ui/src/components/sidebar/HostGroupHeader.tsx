import type React from 'react'
import {
  ChevronDown,
  GripVertical,
  Loader2,
  Monitor,
  Server,
  ServerOff,
  SquareTerminal
} from 'lucide-react'
import { cn } from '@/lib/utils'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { STATUS_LABELS } from '@/components/settings/SshTargetCard'
import type { SidebarHost } from './worktree-list-groups'

const STATUS_DOT: Record<NonNullable<SidebarHost['status']>, string> = {
  reachable: 'bg-emerald-500',
  connecting: 'bg-amber-500',
  // Red, not zinc: a down host must read as an outage at a glance — the old
  // grey dot was indistinguishable from "unknown" while sessions looked dead.
  down: 'bg-red-500',
  unknown: 'bg-zinc-300'
}

/** Short wording for the down-line. 'disconnected' is a deliberate/idle state
 *  (muted), everything else is an outage (red) — mirrors settings statusColor. */
function downLabel(sshStatus: SidebarHost['sshStatus']): string {
  switch (sshStatus) {
    case 'disconnected':
      return 'Disconnected'
    case 'auth-failed':
      return 'Auth failed'
    default:
      return 'Host unreachable'
  }
}

// Drop IPv4/IPv6 literals from the host detail so the address isn't exposed at a
// glance (screenshots / screenshares), then tidy the leftover separators so the
// line reads cleanly (e.g. "ssh 172.30.66.4 · Linux 6.9" → "ssh · Linux 6.9").
// The full detail (real IP) is shown on hover via a tooltip.
const IPV4_RE = /\b\d{1,3}(?:\.\d{1,3}){3}\b/g
const IPV6_RE = /\b(?:[0-9a-fA-F]{1,4}:){2,7}[0-9a-fA-F]{0,4}\b/g
function redactDetail(text: string): string {
  const redacted = text.replace(IPV4_RE, '').replace(IPV6_RE, '')
  // Collapse the double space the removed token leaves, and drop a now-dangling
  // leading separator (e.g. "ssh  · Linux" → "ssh · Linux"; " · Linux" → "Linux").
  return redacted
    .replace(/\s{2,}/g, ' ')
    .replace(/\s+·/g, ' ·')
    .replace(/^\s*·\s*/, '')
    .trim()
}

/** Whether redaction actually removed an address (so we only offer the
 *  hover-to-reveal tooltip when there's something hidden). */
function hasRedactedIp(text: string): boolean {
  return redactDetail(text) !== text.trim()
}

export function HostGroupHeader({
  host,
  count,
  runningCount,
  collapsed,
  onToggle,
  onOpenTmuxSessions,
  onReconnect,
  onHeaderPointerDown,
  dragId,
  isDragging
}: {
  host: SidebarHost
  count: number
  /** Live working workspaces on this host. When present, the hierarchy header
   *  shows the approved working/idle pair instead of the legacy total pill. */
  runningCount?: number
  collapsed: boolean
  onToggle: () => void
  /** Open the tmux sessions modal for this host. Always shown for SSH hosts;
   *  shown for the local host when tmux is in use. */
  onOpenTmuxSessions?: () => void
  /** Reconnect the SSH transport for this host. Rendered as an inline action
   *  on the down-line; the store flips to 'connecting' as soon as the attempt
   *  starts, so this header re-renders into the spinner state on its own. */
  onReconnect?: () => void | Promise<void>
  /** When `dragId` is set (SSH hosts only), the header becomes a drag handle
   *  for reordering host groups (spec 383): the root stamps `data-host-header-id`
   *  and the pointer-down arms the shared reorder controller. The local host
   *  passes neither, so it is never draggable and stays pinned first. */
  onHeaderPointerDown?: (event: React.PointerEvent<HTMLElement>, dragId: string) => void
  dragId?: string
  isDragging?: boolean
}): React.JSX.Element {
  const Icon = host.kind === 'ssh' ? Server : Monitor
  const status = host.status ?? 'unknown'
  const isSshDown = host.kind === 'ssh' && status === 'down'
  const isSshConnecting = host.kind === 'ssh' && status === 'connecting'
  const downTone =
    host.sshStatus === 'disconnected' ? 'text-muted-foreground' : 'text-red-400'
  const idleCount = Math.max(0, count - (runningCount ?? 0))

  const handleReconnectClick = (e: React.MouseEvent): void => {
    // Reconnect must never toggle the section collapse. No local busy flag:
    // the store flips to 'connecting' as the attempt starts, which swaps this
    // button out for the spinner line (also keeps this component hook-free so
    // tests can invoke it directly).
    e.stopPropagation()
    void onReconnect?.()
  }
  return (
    <div
      role="button"
      tabIndex={0}
      data-host-header-id={dragId}
      onPointerDown={
        dragId && onHeaderPointerDown ? (e) => onHeaderPointerDown(e, dragId) : undefined
      }
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onToggle()
        }
      }}
      // Why: min-h (not fixed h-8) — SSH hosts render a second `detail` line, so
      // a one-line height clipped the two-line content and bled it onto the row
      // below. min-h keeps single-line hosts at 32px while letting SSH headers grow.
      className={cn(
        'group flex min-h-9 w-full items-center gap-1.5 rounded-md px-1 py-0.5 text-left transition-colors hover:bg-sidebar-foreground/[0.035]',
        // A draggable (SSH, 2+) host gets the grab cursor so hovering signals it
        // can be reordered; the local / single host stays a plain pointer.
        dragId ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer',
        isDragging && 'opacity-60'
      )}
    >
      {/* The recognised drag-handle affordance (spec 383 AC1): a grip that fades
          in on hover for reorderable hosts. The whole header is the drag target
          (bigger hit area); this just makes the capability discoverable. */}
      {dragId ? (
        <GripVertical
          className="-mr-0.5 size-3.5 shrink-0 text-muted-foreground/40 transition-colors group-hover:text-muted-foreground"
          aria-hidden
        />
      ) : null}
      <ChevronDown
        className={cn(
          'size-3.5 shrink-0 text-muted-foreground transition-transform',
          collapsed && '-rotate-90'
        )}
      />
      <Icon className="size-4 shrink-0 text-muted-foreground" />
      <div className="flex min-w-0 flex-1 flex-col leading-tight">
        <span className="flex min-w-0 items-center gap-1">
          <span className="truncate text-sm font-semibold text-foreground">{host.label}</span>
          {host.hasTmux ? (
            // Truthful per-host marker: this host has at least one live
            // tmux-backed session. Emerald + small; hover explains it.
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="inline-flex cursor-help">
                  <SquareTerminal
                    className="size-3.5 shrink-0 text-emerald-500"
                    aria-label="This machine is using tmux"
                  />
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4}>
                {host.kind === 'ssh'
                  ? 'This host is running sessions in tmux'
                  : 'This machine is running sessions in tmux'}
              </TooltipContent>
            </Tooltip>
          ) : null}
        </span>
        {isSshDown || isSshConnecting ? (
          // The transport line replaces the host detail while the connection
          // is anything but healthy: an unavailable host must say so where the
          // user is looking (the sidebar), not only in the status bar.
          <span className="flex min-w-0 items-center gap-1 text-[11px]">
            {isSshConnecting ? (
              <Loader2 className="size-3 shrink-0 animate-spin text-amber-500" />
            ) : (
              <ServerOff className={cn('size-3 shrink-0', downTone)} />
            )}
            {isSshDown && host.sshError ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <span className={cn('cursor-help truncate', downTone)}>
                    {downLabel(host.sshStatus)}
                  </span>
                </TooltipTrigger>
                <TooltipContent side="bottom" sideOffset={4}>
                  {host.sshError}
                </TooltipContent>
              </Tooltip>
            ) : (
              <span className={cn('truncate', isSshConnecting ? 'text-muted-foreground' : downTone)}>
                {isSshConnecting
                  ? STATUS_LABELS[host.sshStatus ?? 'connecting']
                  : downLabel(host.sshStatus)}
              </span>
            )}
            {isSshDown && onReconnect ? (
              <button
                type="button"
                onClick={handleReconnectClick}
                onKeyDown={(e) => {
                  // Keep Enter/Space from bubbling into the row's toggle handler.
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.stopPropagation()
                  }
                }}
                className="shrink-0 rounded px-1 py-px text-[11px] font-medium text-foreground hover:bg-sidebar-accent"
                aria-label={`Reconnect to ${host.label}`}
              >
                Reconnect
              </button>
            ) : null}
          </span>
        ) : host.detail ? (
          hasRedactedIp(host.detail) ? (
            // IP dropped from the line; hover reveals the real address.
            <Tooltip>
              <TooltipTrigger asChild>
                <span className="w-fit max-w-full cursor-help truncate text-[11px] text-muted-foreground">
                  {redactDetail(host.detail)}
                </span>
              </TooltipTrigger>
              <TooltipContent side="bottom" sideOffset={4}>
                {host.detail}
              </TooltipContent>
            </Tooltip>
          ) : (
            <span className="truncate text-[11px] text-muted-foreground">{host.detail}</span>
          )
        ) : null}
      </div>
      {onOpenTmuxSessions ? (
        // Hover-revealed affordance: open the tmux sessions browser for this
        // host. Stop propagation so clicking never toggles the section collapse.
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={(e) => {
                e.stopPropagation()
                onOpenTmuxSessions()
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.stopPropagation()
                }
              }}
              className={cn(
                'ml-auto inline-flex size-5 shrink-0 items-center justify-center rounded bg-transparent opacity-0 transition-colors transition-opacity',
                'group-hover:opacity-100 group-focus-within:opacity-100 focus-visible:opacity-100',
                'text-muted-foreground hover:bg-sidebar-accent hover:text-foreground focus-visible:bg-sidebar-accent focus-visible:text-foreground focus-visible:outline-none'
              )}
              aria-label={`Tmux sessions on ${host.label}`}
            >
              <SquareTerminal className="size-3.5" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            Tmux sessions
          </TooltipContent>
        </Tooltip>
      ) : null}
      {runningCount !== undefined && status === 'reachable' ? (
        <span
          className={cn(
            'inline-flex items-center gap-1.5 text-[9px] tabular-nums text-muted-foreground',
            onOpenTmuxSessions ? 'ml-1' : 'ml-auto'
          )}
          aria-label={`${runningCount} working, ${idleCount} idle`}
        >
          <span className="inline-flex items-center gap-1">
            <span className="size-1.5 rounded-full bg-emerald-500 shadow-[0_0_0_2px_color-mix(in_srgb,#10b981_9%,transparent)]" />
            {runningCount}
          </span>
          <span className="inline-flex items-center gap-1">
            <span className="size-1.5 rounded-full bg-muted-foreground/45" />
            {idleCount}
          </span>
        </span>
      ) : (
        <span
          className={cn(
            'inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground',
            // Push the count to the right edge only when the tmux button isn't
            // there to claim `ml-auto` itself.
            onOpenTmuxSessions ? 'ml-1' : 'ml-auto'
          )}
        >
          <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
          {count}
        </span>
      )}
    </div>
  )
}
