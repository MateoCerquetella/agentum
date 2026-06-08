import type React from 'react'
import { ChevronDown, Monitor, Server, SquareTerminal } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { SidebarHost } from './worktree-list-groups'

const STATUS_DOT: Record<NonNullable<SidebarHost['status']>, string> = {
  reachable: 'bg-emerald-500',
  connecting: 'bg-amber-500',
  down: 'bg-zinc-400',
  unknown: 'bg-zinc-300'
}

// Mask IPv4/IPv6 literals in the host detail so the address isn't exposed at a
// glance (screenshots / screenshares). The full detail (real IP) stays on the
// element's `title`, so hovering still reveals it.
const IPV4_RE = /\b\d{1,3}(?:\.\d{1,3}){3}\b/g
const IPV6_RE = /\b(?:[0-9a-fA-F]{1,4}:){2,7}[0-9a-fA-F]{0,4}\b/g
function maskIps(text: string): string {
  return text.replace(IPV4_RE, '•••.•••.•••.•••').replace(IPV6_RE, '••••')
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
      className="group flex h-8 w-full cursor-pointer items-center gap-1.5 px-1 text-left"
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
            // tmux-backed session. Understated by design — muted, small, and
            // sized to be easy to ignore (mock-up: "present but easy to ignore").
            <SquareTerminal
              className="size-3 shrink-0 text-muted-foreground/70"
              aria-label="Has tmux sessions"
            >
              <title>Has tmux sessions</title>
            </SquareTerminal>
          ) : null}
        </span>
        {host.detail ? (
          // IP masked by default; the real address is on `title`, so hovering
          // the line reveals it.
          <span
            className="truncate text-[11px] text-muted-foreground"
            title={host.detail}
          >
            {maskIps(host.detail)}
          </span>
        ) : null}
      </div>
      <span className="ml-auto inline-flex items-center gap-1 rounded-full bg-sidebar-accent px-1.5 py-0.5 text-[11px] text-muted-foreground">
        <span className={cn('size-1.5 rounded-full', STATUS_DOT[status])} />
        {count}
      </span>
    </div>
  )
}
