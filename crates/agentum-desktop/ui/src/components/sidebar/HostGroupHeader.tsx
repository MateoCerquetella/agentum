import type React from 'react'
import { ChevronDown, Monitor, Server, SquareTerminal } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import type { SidebarHost } from './worktree-list-groups'

const STATUS_DOT: Record<NonNullable<SidebarHost['status']>, string> = {
  reachable: 'bg-emerald-500',
  connecting: 'bg-amber-500',
  down: 'bg-zinc-400',
  unknown: 'bg-zinc-300'
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
  collapsed,
  onToggle
}: {
  host: SidebarHost
  count: number
  collapsed: boolean
  onToggle: () => void
}): React.JSX.Element {
  const Icon = host.kind === 'ssh' ? Server : Monitor
  const status = host.status ?? 'unknown'
  return (
    <div
      role="button"
      tabIndex={0}
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
      className="group flex min-h-8 w-full cursor-pointer items-center gap-1.5 px-1 py-0.5 text-left"
    >
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
        {host.detail ? (
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
      <span className="ml-auto inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground">
        <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
        {count}
      </span>
    </div>
  )
}
